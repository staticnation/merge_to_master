//! OpenMW's Lua script configuration (`LUAL`, tes3's `ScriptConfigList`).
//!
//! A content file lists the Lua scripts it attaches, by path. OpenMW gathers every
//! file's lists in load order and resolves them in `LuaUtil::ScriptsConfiguration::init`
//! (components/lua/configuration.cpp), which this follows:
//!
//! * A later configuration for a path (compared case-blind) **replaces** the earlier
//!   one, and takes the later one's place in the order - OpenMW keeps the later entry
//!   and skips the earlier.
//! * Unless its flags carry `MERGE` (`sMerge`, 0x08): then it is folded into the earlier
//!   one - its flags OR'd in (the earlier one's own `MERGE` bit left as it was), its
//!   initialization data taken when it has any, and its attach-to types, per-record and
//!   per-reference entries added. OpenMW appends those entries and, per record id or
//!   reference, the last one decides (attach or detach, and its data); here a later entry
//!   for the same record or reference takes the earlier one's place, which leaves the
//!   same last word.
//!
//! One case differs on purpose: OpenMW does not re-point a path's index when a plain
//! replacement skips the earlier entry, so a `MERGE` entry after a replacement folds into
//! the skipped one and is lost. Here it folds into the replacement.
//!
//! Object references follow the masters when they are remapped (`remap_masters.rs`):
//! the per-reference entries (LUAI), whose content file is 0 for the file itself and 1..
//! for its masters as FRMR's, and the references serialized inside the initialization
//! data (LUAD, `types::luad`), which OpenMW renumbers the same way on load
//! (`LuaScriptsCfg::adjustRefNums`).

use tes3::esp::{OMWScriptAttachFlag, PerInstanceConfig, PerRecordConfig, ScriptConfig, ScriptConfigList};

/// A script path as OpenMW compares them: forward slashes, lower case.
fn path_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

/// Folds `list`'s scripts into `target`, as a later content file's configuration.
pub fn merge_script_lists(list: ScriptConfigList, target: &mut Option<ScriptConfigList>) {
    let target = target.get_or_insert_with(ScriptConfigList::default);
    target.flags = list.flags;
    for script in list.scripts {
        let key = path_key(&script.path);
        let at = target.scripts.iter().position(|s| path_key(&s.path) == key);
        match at {
            Some(i) if script.flags.contains(OMWScriptAttachFlag::MERGE) => {
                merge_script(script, &mut target.scripts[i]);
            }
            Some(i) => {
                // Replaced: the later one stands, where the later one comes.
                target.scripts.remove(i);
                target.scripts.push(script);
            }
            None => target.scripts.push(script),
        }
    }
}

/// A `MERGE` configuration folded into an earlier one for the same path.
fn merge_script(script: ScriptConfig, existing: &mut ScriptConfig) {
    // Whether the result still merges onto something earlier is the earlier one's say.
    let keep_merge = existing.flags & OMWScriptAttachFlag::MERGE;
    existing.flags = keep_merge | ((existing.flags | script.flags) - OMWScriptAttachFlag::MERGE);
    if !script.init_data.is_empty() {
        existing.init_data = script.init_data;
    }
    for t in script.types {
        if !existing.types.contains(&t) {
            existing.types.push(t);
        }
    }
    for record in script.records {
        merge_record(record, &mut existing.records);
    }
    for instance in script.instances {
        merge_instance(instance, &mut existing.instances);
    }
}

fn merge_record(record: PerRecordConfig, records: &mut Vec<PerRecordConfig>) {
    match records.iter_mut().find(|r| r.id.eq_ignore_ascii_case(&record.id)) {
        Some(r) => *r = record,
        None => records.push(record),
    }
}

fn merge_instance(instance: PerInstanceConfig, instances: &mut Vec<PerInstanceConfig>) {
    match instances
        .iter_mut()
        .find(|i| i.mast_idx == instance.mast_idx && i.ref_idx == instance.ref_idx)
    {
        Some(i) => *i = instance,
        None => instances.push(instance),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(path: &str, flags: OMWScriptAttachFlag, types: &[&str], records: &[(&str, bool)]) -> ScriptConfig {
        ScriptConfig {
            path: path.into(),
            flags,
            types: types.iter().map(|t| (*t).to_string()).collect(),
            records: records
                .iter()
                .map(|(id, attach)| PerRecordConfig { attach: *attach, id: (*id).to_string(), data: Vec::new() })
                .collect(),
            ..Default::default()
        }
    }

    fn list(scripts: Vec<ScriptConfig>) -> ScriptConfigList {
        ScriptConfigList { scripts, ..Default::default() }
    }

    #[test]
    fn a_later_config_replaces_an_earlier_one() {
        let mut target = Some(list(vec![script("scripts/a.lua", OMWScriptAttachFlag::CUSTOM, &["NPC_"], &[("fargoth", true)])]));
        merge_script_lists(list(vec![script("Scripts\\A.lua", OMWScriptAttachFlag::GLOBAL, &[], &[])]), &mut target);
        let t = target.unwrap();
        assert_eq!(t.scripts.len(), 1);
        assert_eq!(t.scripts[0].flags, OMWScriptAttachFlag::GLOBAL);
        assert!(t.scripts[0].types.is_empty() && t.scripts[0].records.is_empty());
    }

    #[test]
    fn a_merging_config_combines_with_the_earlier_one() {
        let mut target = Some(list(vec![script(
            "scripts/a.lua",
            OMWScriptAttachFlag::CUSTOM,
            &["NPC_"],
            &[("fargoth", true), ("caius", true)],
        )]));
        merge_script_lists(
            list(vec![
                script(
                    "scripts/a.lua",
                    OMWScriptAttachFlag::MERGE | OMWScriptAttachFlag::PLAYER,
                    &["CREA", "NPC_"],
                    &[("Fargoth", false)],
                ),
                script("scripts/b.lua", OMWScriptAttachFlag::GLOBAL, &[], &[]),
            ]),
            &mut target,
        );
        let t = target.unwrap();
        assert_eq!(t.scripts.len(), 2);
        let a = &t.scripts[0];
        assert_eq!(a.flags, OMWScriptAttachFlag::CUSTOM | OMWScriptAttachFlag::PLAYER);
        assert_eq!(a.types, vec!["NPC_".to_string(), "CREA".to_string()]);
        assert_eq!(a.records.len(), 2);
        assert!(!a.records[0].attach, "the later entry for fargoth wins");
        assert_eq!(t.scripts[1].path, "scripts/b.lua");
    }

    #[test]
    fn a_replacement_takes_the_later_place() {
        // OpenMW keeps the later entry and skips the earlier, so b now comes first.
        let mut target = Some(list(vec![
            script("scripts/a.lua", OMWScriptAttachFlag::GLOBAL, &[], &[]),
            script("scripts/b.lua", OMWScriptAttachFlag::GLOBAL, &[], &[]),
        ]));
        merge_script_lists(list(vec![script("scripts/a.lua", OMWScriptAttachFlag::GLOBAL, &[], &[])]), &mut target);
        let paths: Vec<String> = target.unwrap().scripts.into_iter().map(|s| s.path).collect();
        assert_eq!(paths, vec!["scripts/b.lua".to_string(), "scripts/a.lua".to_string()]);
    }

    #[test]
    fn a_new_merging_config_keeps_its_merge_flag() {
        let mut target = None;
        merge_script_lists(list(vec![script("scripts/a.lua", OMWScriptAttachFlag::MERGE, &[], &[])]), &mut target);
        assert!(target.unwrap().scripts[0].flags.contains(OMWScriptAttachFlag::MERGE));
    }
}
