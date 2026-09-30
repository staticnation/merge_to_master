use crate::prelude::*;

pub trait RemapMasters: Sized {
    /// Remap the references of `plugin` to be compatible with `master`.
    ///
    /// Additionally update the masters list of `master` to include any \
    /// masters from `plugin` that were not already present.
    ///
    /// These ensure that `plugin` can now be safely merged with `master`.
    ///
    /// # Explanation
    ///
    /// Consider we're merging two plugins that have different master files:
    /// ```ignore
    /// "PluginA.esp" => ["Morrowind.esm", "Tamriel Data.esm"]
    /// "PluginB.esp" => ["Morrowind.esm", "OAAB_Data.esm", "Tamriel Data.esm"]
    /// ```
    ///
    /// Assume both plugins move the same object in a particular cell of `Tamriel Data.esm`. \
    /// Let's say the object was an "iron dagger" in the cell "Ebon Tower".
    ///
    /// Inside `PluginA.esp` this will be represented something like this:
    /// ```ignore
    /// Cell {
    ///     name: "Ebon Tower",
    ///     references: {
    ///         (2, 15): {
    ///             id: "iron dagger",
    ///             position: [ ... ],
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// Inside `PluginB.esp` this will be represented something like this:
    /// ```ignore
    /// Cell {
    ///     name: "Ebon Tower",
    ///     references: {
    ///         (3, 15): {
    ///             id: "iron dagger",
    ///             position: [ ... ],
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// Note that the *master indices* differ despite being the same object: \
    /// ```ignore
    /// (2, 15) != (3, 15)
    /// ```
    ///
    /// This is because plugins track references using the index of the master \
    /// that defined them. *The index in the plugin's own masters list.*
    ///
    /// In `PluginA.esp` the master `Tamriel Data.esm` is at index 2. \
    /// In `PluginB.esp` the master `Tamriel Data.esm` is at index 3. \
    /// *(indexing starts at 1; The 0 index is reserved and means the current plugin)*
    ///
    /// If we just naively merged `PluginA.esp` into `PluginB.esp` we would \
    /// end up with something very wrong:
    /// ```ignore
    /// Cell {
    ///     name: "Ebon Tower",
    ///     references: {
    ///         (2, 15): { ... }
    ///         (3, 15): { ... }
    ///     }
    /// }
    /// ```
    /// The edits no longer refer to the same object. The master at index `2` in \
    /// `PluginB.esp` is not `Tamriel Data.esm`, but `OAAB_Data.esm`. Which means \
    /// the edit is now being applied to some object of `OAAB_Data.esm`!
    ///
    /// The correct way to handle this merge would be to remap the indices of the plugin \
    /// to be consistent with the indices of those that it is being merged into. Which is \
    /// this function does.
    ///
    fn remap_masters(&mut self, master: &PluginData, master_name: &str);

    fn from_path_remap_masters(
        path: &Path,
        plugin_index: usize,
        master_remap: &HashMap<&UncasedStr, u32>,
    ) -> Result<Self>;
}

impl RemapMasters for PluginData {
    fn remap_masters(&mut self, master: &PluginData, master_name: &str) {
        let (new_masters, index_remap) = get_index_remap(&self.header.masters, &master.header.masters, master_name);

        // Copy author/description/etc from the master file to the plugin file.
        self.header = master.header.clone();

        if let Some(masters) = new_masters {
            self.header.masters = masters;
        }

        if let Some(indices) = index_remap {
            let start_index = next_reference_index(master);
            apply_index_remap(self, &indices, start_index);
        }
    }

    /// Load a plugin and remap its reference indices.
    ///
    fn from_path_remap_masters(
        path: &Path,
        plugin_index: usize,
        master_remap: &HashMap<&UncasedStr, u32>,
    ) -> Result<Self> {
        let mut plugin = PluginData::from_path(path)?;

        // References refer to their source file (mast_index) using the index
        // defined in the current plugin's header. We're making a merged file
        // that has *all* plugins as masters, so we need to remap the indices.
        let mut local_remap = SmallVec::<[u32; 8]>::new();

        // Index 0 always refers to the plugin itself.
        local_remap.push((plugin_index + 1) as u32);

        // Indices 1..N refer to the plugin's masters, we need to remap them.
        for (name, _) in &plugin.header.masters {
            local_remap.push(match master_remap.get(name.as_uncased()) {
                Some(i) => *i,
                None => {
                    bail!("Master '{}' not found. ({})", name, path.display())
                }
            });
        }

        // Now apply the index remap to all references in the plugin.
        for cell in plugin.cells.iter_mut() {
            cell.references = std::mem::take(&mut cell.references)
                .into_iter()
                .filter_map(|(_, mut reference)| {
                    reference.mast_index = *local_remap.get(reference.mast_index as usize)?;
                    Some(((reference.mast_index, reference.refr_index), reference))
                })
                .collect();
        }

        // And OpenMW's per-reference Lua script entries, which name references the same way,
        // and the references serialized inside Lua initialization data.
        if let Some(lua) = plugin.lua.as_mut() {
            remap_lua_data(lua, |file, _| {
                if let Some(m) = usize::try_from(*file).ok().and_then(|i| local_remap.get(i).copied()) {
                    *file = m as i32;
                }
            });
            for script in &mut lua.scripts {
                script.instances.retain_mut(|instance| {
                    let remapped = usize::try_from(instance.mast_idx)
                        .ok()
                        .and_then(|i| local_remap.get(i).copied());
                    remapped.is_some_and(|m| {
                        instance.mast_idx = m as i32;
                        true
                    })
                });
            }
        }

        Ok(plugin)
    }
}

type Masters = Vec<(String, u64)>; // (name, size)
type Indices = Vec<u32>;

fn get_index_remap(
    plugin_masters: &Masters,
    master_masters: &Masters,
    target_master: &str,
) -> (Option<Masters>, Option<Indices>) {
    let mut new_masters = Vec::with_capacity(10);
    let mut index_remap = Vec::with_capacity(10);

    new_masters.extend(master_masters.iter().cloned());
    index_remap.push(0); // Index 0 is reserved for only local references.

    for master in plugin_masters {
        index_remap.push({
            // If it matches target_master then remap references to local.
            if master.0.eq_ignore_ascii_case(target_master) {
                0
            }
            // Otherwise remap it to the master position in `new_masters`.
            else {
                new_masters
                    .iter()
                    .position(|(name, _)| master.0.eq_ignore_ascii_case(name))
                    .map_or_else(
                        || {
                            new_masters.push(master.clone());
                            new_masters.len().try_into().unwrap()
                        },
                        |i| (i + 1).try_into().unwrap(),
                    )
            }
        });
    }

    let masters_changed = master_masters != &new_masters;
    let indices_changed = (0..).zip(&index_remap).any(|(i, &j)| i != j);
    (
        masters_changed.then_some(new_masters),
        indices_changed.then_some(index_remap),
    )
}

/// Returns the next available reference index for a plugin.
///
/// It's the highest reference index in the plugin plus one.
///
fn next_reference_index(plugin: &PluginData) -> u32 {
    plugin
        .cells
        .par_iter()
        .filter_map(|cell| {
            cell.references
                .values()
                .filter_map(|reference| (reference.mast_index == 0).then_some(reference.refr_index))
                .max()
        })
        .max()
        .map_or(1, |i| i + 1)
}

fn apply_index_remap(plugin: &mut PluginData, index_remap: &[u32], start_index: u32) {
    let mut next_index = start_index;

    // The plugin's own references are numbered afresh; remembered, old to new, for the
    // Lua script entries that name them.
    let mut renumbered: HashMap<u32, u32> = HashMap::new();

    let cells = plugin.cells.iter_mut();

    for cell in cells {
        let references = std::mem::take(&mut cell.references).into_iter();

        cell.references = references
            .into_iter()
            .map(|((mut mast_index, mut refr_index), mut reference)| {
                if mast_index == 0 {
                    renumbered.insert(refr_index, next_index);
                    refr_index = next_index;
                    next_index += 1;
                } else {
                    mast_index = index_remap[mast_index as usize];
                }
                reference.mast_index = mast_index;
                reference.refr_index = refr_index;
                ((mast_index, refr_index), reference)
            })
            .collect();
    }

    // OpenMW's per-reference Lua script entries follow their references, and so do the
    // references serialized inside Lua initialization data.
    if let Some(lua) = plugin.lua.as_mut() {
        remap_lua_data(lua, |file, index| {
            if *file == 0 {
                if let Some(&new) = renumbered.get(&*index) {
                    *index = new;
                }
            } else if let Some(&m) = usize::try_from(*file).ok().and_then(|i| index_remap.get(i)) {
                *file = m as i32;
            }
        });
        for script in &mut lua.scripts {
            for instance in &mut script.instances {
                if instance.mast_idx == 0 {
                    if let Some(&new) = renumbered.get(&instance.ref_idx) {
                        instance.ref_idx = new;
                    }
                } else if let Some(&m) = usize::try_from(instance.mast_idx)
                    .ok()
                    .and_then(|i| index_remap.get(i))
                {
                    instance.mast_idx = m as i32;
                }
            }
        }
    }
}

/// Runs `f` over every object reference serialized in a Lua configuration's
/// initialization data (`types::luad`). Data that does not parse is left as it was.
fn remap_lua_data(lua: &mut tes3::esp::ScriptConfigList, mut f: impl FnMut(&mut i32, &mut u32)) {
    for script in &mut lua.scripts {
        let datas = std::iter::once(&mut script.init_data)
            .chain(script.records.iter_mut().map(|r| &mut r.data))
            .chain(script.instances.iter_mut().map(|i| &mut i.data));
        for data in datas {
            if let Err(e) = remap_refnums(data, &mut f) {
                warn!("Lua initialization data of {} left as it was: {e}", script.path);
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn masters_vec(master_names: &[&str]) -> Masters {
        master_names.iter().map(|&name| (name.into(), 0)).collect()
    }

    #[test]
    fn no_masters() {
        let plugin_masters = masters_vec(&[]);
        let master_masters = masters_vec(&[]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, None);
        assert_eq!(indices, None);
    }

    #[test]
    fn one_identical_master() {
        let plugin_masters = masters_vec(&["A"]);
        let master_masters = masters_vec(&["A"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, None);
        assert_eq!(indices, None);
    }

    #[test]
    fn many_identical_masters() {
        let plugin_masters = masters_vec(&["A", "B", "C"]);
        let master_masters = masters_vec(&["A", "B", "C"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, None);
        assert_eq!(indices, None);
    }

    #[test]
    fn one_mismatched_master() {
        let plugin_masters = masters_vec(&["A"]);
        let master_masters = masters_vec(&["B"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");

        // The new master gets appended onto the initial master list.
        assert_eq!(masters, Some(masters_vec(&["B", "A"])));

        // Plugin references at index 1 should get shifted to index 2.
        assert_eq!(indices, Some(vec![0, 2]));
    }

    #[test]
    fn many_mismatched_masters() {
        let plugin_masters = masters_vec(&["A", "B", "C"]);
        let master_masters = masters_vec(&["D", "E", "F"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");

        // The new masters get appended onto the initial master list.
        assert_eq!(masters, Some(masters_vec(&["D", "E", "F", "A", "B", "C"])));

        // The reference indices get shifted over to stay consistent.
        assert_eq!(indices, Some(vec![0, 4, 5, 6]));
    }

    #[test]
    fn some_mismatched_masters() {
        let plugin_masters = masters_vec(&["A", "B", "C", "D"]);
        let master_masters = masters_vec(&["A", "E", "C", "F"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");

        // The new masters ("B", "D") get appended onto the initial master list.
        assert_eq!(masters, Some(masters_vec(&["A", "E", "C", "F", "B", "D"])));

        // 0 = reserved
        // 1 = "A" was unchanged, it was at position 1 of both lists
        // 5 = "B" has moved, it's now at position 5 in the new list
        // 3 = "C" has moved, it's now at position 3 in the new list
        // 6 = "D" has moved, it's now at position 6 in the new list
        assert_eq!(indices, Some(vec![0, 1, 5, 3, 6]));
    }

    #[test]
    fn plugin_merging_into_master() {
        let plugin_masters = masters_vec(&["A", "B", "C"]);
        let master_masters = masters_vec(&["A", "D"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "B");

        // Target is omitted from the master list. It cannot be a master of itself.
        assert_eq!(masters, Some(masters_vec(&["A", "D", "C"])));

        // 0 = reserved
        // 1 = "A" was unchanged, it was at position 1 of both lists
        // 0 = "B" is the target master so it is moved to position 0
        // 3 = "C" was unchanged, it was at position 3 of both lists
        assert_eq!(indices, Some(vec![0, 1, 0, 3]));
    }

    #[test]
    fn mismatched_masters_of_consistent_order() {
        let plugin_masters = masters_vec(&["A", "B"]);
        let master_masters = masters_vec(&["A", "B", "C"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, None);
        assert_eq!(indices, None);
    }

    #[test]
    fn mismatched_masters_of_consistent_order_inv() {
        let plugin_masters = masters_vec(&["A", "B", "C"]);
        let master_masters = masters_vec(&["A", "B"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, Some(masters_vec(&["A", "B", "C"])));
        assert_eq!(indices, None);
    }

    #[test]
    fn same_masters_in_different_order() {
        let plugin_masters = masters_vec(&["C", "A", "B"]);
        let master_masters = masters_vec(&["A", "C", "B"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, None);
        assert_eq!(indices, Some(vec![0, 2, 1, 3]));
    }

    #[test]
    fn either_side_empty() {
        let plugin_masters = masters_vec(&[]);
        let master_masters = masters_vec(&["A"]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, None);
        assert_eq!(indices, None);

        let plugin_masters = masters_vec(&["A"]);
        let master_masters = masters_vec(&[]);
        let (masters, indices) = get_index_remap(&plugin_masters, &master_masters, "");
        assert_eq!(masters, Some(masters_vec(&["A"])));
        assert_eq!(indices, None);
    }
}
