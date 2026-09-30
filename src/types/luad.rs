//! OpenMW's serialized Lua data (`LUAD`) - walked, not decoded, to renumber the object
//! references inside it.
//!
//! LUAL's initialization data is a Lua value serialized by OpenMW's
//! `LuaUtil::serialize` (components/lua/serialization.cpp): a format-version byte (0),
//! then one value. Tables nest; an object reference is a custom value of type name `o`
//! holding an `ESM::RefNum` - `u32` index, `i32` content file, little-endian - which in a
//! content file counts 0 for the file itself and 1.. for its masters, like LUAI and FRMR
//! (`LuaScriptsCfg::adjustRefNums`). When masters are remapped those numbers have to
//! follow, or a script's saved reference points at another master's object.
//!
//! OpenMW deserializes the value and serializes it again; here the bytes are walked and
//! each reference is rewritten in place. A reference is always eight bytes, so nothing
//! else moves and the rest of the data is left byte for byte.

/// The serialization format version OpenMW writes and accepts.
const FORMAT_VERSION: u8 = 0;

const SHORT_STRING_FLAG: u8 = 0x20;
const CUSTOM_FULL_FLAG: u8 = 0x40;
const CUSTOM_COMPACT_FLAG: u8 = 0x80;

/// The custom type name of an object reference (`sRefNumTypeName`).
const REFNUM_TYPE_NAME: &[u8] = b"o";

/// Nested tables OpenMW will write at most (it refuses to serialize deeper).
const MAX_DEPTH: usize = 64;

/// Calls `f(content_file, index)` for every object reference serialized in `data`, and
/// writes back what it leaves them as. Empty data is no value at all and is fine.
pub fn remap_refnums(data: &mut [u8], f: &mut impl FnMut(&mut i32, &mut u32)) -> Result<(), String> {
    if data.is_empty() {
        return Ok(());
    }
    if data[0] != FORMAT_VERSION {
        return Err(format!("unknown Lua serialization format {}", data[0]));
    }
    let mut pos = 1;
    value(data, &mut pos, f, 0)?;
    if pos != data.len() {
        return Err("data after the serialized value".into());
    }
    Ok(())
}

fn take(data: &[u8], pos: &mut usize, n: usize) -> Result<usize, String> {
    let at = *pos;
    let end = at.checked_add(n).filter(|&e| e <= data.len()).ok_or("unexpected end of serialized data")?;
    *pos = end;
    Ok(at)
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn value(data: &mut [u8], pos: &mut usize, f: &mut impl FnMut(&mut i32, &mut u32), depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("tables nested too deep".into());
    }
    let t = data[take(data, pos, 1)?];
    if t & (CUSTOM_COMPACT_FLAG | CUSTOM_FULL_FLAG) != 0 {
        let (name_len, size) = if t & CUSTOM_COMPACT_FLAG != 0 {
            ((t & 7) as usize + 1, ((t >> 3) & 15) as usize)
        } else {
            let at = take(data, pos, 4)?;
            ((t & 63) as usize + 1, u32_at(data, at) as usize)
        };
        let name_at = take(data, pos, name_len)?;
        let at = take(data, pos, size)?;
        if &data[name_at..name_at + name_len] == REFNUM_TYPE_NAME {
            if size != 8 {
                return Err("an object reference that is not eight bytes".into());
            }
            let mut index = u32_at(data, at);
            let mut file = u32_at(data, at + 4) as i32;
            f(&mut file, &mut index);
            data[at..at + 4].copy_from_slice(&index.to_le_bytes());
            data[at + 4..at + 8].copy_from_slice(&file.to_le_bytes());
        }
        return Ok(());
    }
    if t & SHORT_STRING_FLAG != 0 {
        take(data, pos, (t & 0x1f) as usize)?;
        return Ok(());
    }
    match t {
        0x00 => {
            take(data, pos, 8)?; // number (f64)
        }
        0x01 => {
            let at = take(data, pos, 4)?; // long string
            let n = u32_at(data, at) as usize;
            take(data, pos, n)?;
        }
        0x02 => {
            take(data, pos, 1)?; // boolean
        }
        0x03 => {
            // table: key, value pairs until TABLE_END
            loop {
                match data.get(*pos) {
                    None => return Err("unexpected end of serialized data".into()),
                    Some(0x04) => {
                        *pos += 1;
                        break;
                    }
                    Some(_) => {
                        value(data, pos, f, depth + 1)?;
                        value(data, pos, f, depth + 1)?;
                    }
                }
            }
        }
        0x10 => {
            take(data, pos, 16)?; // vec2: 2 f64
        }
        0x11 => {
            take(data, pos, 24)?; // vec3: 3 f64
        }
        0x12 => {
            take(data, pos, 128)?; // transform (matrix): 16 f64
        }
        0x13 | 0x14 => {
            take(data, pos, 32)?; // transform (quaternion), vec4: 4 f64
        }
        0x15 => {
            take(data, pos, 16)?; // colour: 4 f32
        }
        _ => return Err(format!("unknown type {t:#x} in serialized data")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refnum(file: i32, index: u32) -> Vec<u8> {
        // Compact custom form: 0b1SSSSTTT, S = 8 bytes of data, T = name length - 1 = 0.
        let mut v = vec![CUSTOM_COMPACT_FLAG | (8 << 3), b'o'];
        v.extend_from_slice(&index.to_le_bytes());
        v.extend_from_slice(&file.to_le_bytes());
        v
    }

    #[test]
    fn references_are_renumbered_and_nothing_else_moves() {
        // { target = <ref 1:42>, count = 3, name = "guard", list = { <ref 0:7>, true } }
        let mut data = vec![FORMAT_VERSION, 0x03];
        data.extend([SHORT_STRING_FLAG | 6]);
        data.extend(b"target");
        data.extend(refnum(1, 42));
        data.extend([SHORT_STRING_FLAG | 5]);
        data.extend(b"count");
        data.push(0x00);
        data.extend(3.0f64.to_le_bytes());
        data.extend([SHORT_STRING_FLAG | 4]);
        data.extend(b"name");
        data.extend([SHORT_STRING_FLAG | 5]);
        data.extend(b"guard");
        data.extend([SHORT_STRING_FLAG | 4]);
        data.extend(b"list");
        data.push(0x03);
        data.push(0x00);
        data.extend(1.0f64.to_le_bytes());
        data.extend(refnum(0, 7));
        data.push(0x00);
        data.extend(2.0f64.to_le_bytes());
        data.extend([0x02, 1]);
        data.push(0x04);
        data.push(0x04);

        let before = data.clone();
        let mut seen = Vec::new();
        remap_refnums(&mut data, &mut |file, index| {
            seen.push((*file, *index));
            if *file == 0 {
                *index += 100;
            } else {
                *file = 5;
            }
        })
        .unwrap();
        assert_eq!(seen, vec![(1, 42), (0, 7)]);
        assert_eq!(data.len(), before.len());
        let changed: Vec<usize> = (0..data.len()).filter(|&i| data[i] != before[i]).collect();
        assert_eq!(changed.len(), 2, "only the file of the first and the index of the second");
    }

    #[test]
    fn nothing_and_nonsense() {
        assert!(remap_refnums(&mut [], &mut |_, _| {}).is_ok());
        assert!(remap_refnums(&mut [1, 0x02, 1], &mut |_, _| {}).is_err(), "unknown version");
        assert!(remap_refnums(&mut [0, 0x03, 0x02], &mut |_, _| {}).is_err(), "truncated");
        assert!(remap_refnums(&mut [0, 0x02, 1, 0x02], &mut |_, _| {}).is_err(), "trailing data");
    }
}
