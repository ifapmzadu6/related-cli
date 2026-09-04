//! Byte-level Git object and delta decoding.

use super::limits::MAX_GIT_OBJECT_BYTES;
use super::tree::git_tree_name_cmp;
use super::types::{
    PackedBase, PackedRawObject, RawCommit, RawNamedTreeEntry, RawObjectId, RawObjectKind,
    RawParents, RawTreeEntry,
};
use crate::AnyResult;
use std::cmp::Ordering;
use std::io::Read;

pub(super) fn parse_raw_commit(data: &[u8]) -> AnyResult<RawCommit> {
    let mut tree = None;
    let mut parents = RawParents::default();
    let mut time = None;
    let mut offset = 0;
    for line in data.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            break;
        }
        if let Some(raw_tree) = line.strip_prefix(b"tree ") {
            tree = Some(RawObjectId::from_hex(raw_tree)?);
        } else if let Some(parent) = line.strip_prefix(b"parent ") {
            parents.push(RawObjectId::from_hex(parent)?);
        } else if let Some(committer) = line.strip_prefix(b"committer ") {
            if let Some((seconds, parsed_offset)) = parse_raw_commit_time(committer) {
                time = Some(seconds);
                offset = parsed_offset;
                break;
            }
        } else if time.is_none()
            && let Some(author) = line.strip_prefix(b"author ")
            && let Some((seconds, parsed_offset)) = parse_raw_commit_time(author)
        {
            time = Some(seconds);
            offset = parsed_offset;
        }
    }
    let time = time.ok_or("commit missing timestamp")?;
    Ok(RawCommit {
        tree: tree.ok_or("commit missing tree")?,
        parents,
        time,
        offset,
    })
}

fn parse_raw_commit_time(line: &[u8]) -> Option<(i64, i32)> {
    let mut parts = line.rsplit(|byte| *byte == b' ');
    let timezone = parts.next()?;
    let timestamp = parts.next()?;
    Some((parse_decimal_i64(timestamp)?, parse_raw_timezone(timezone)?))
}

fn parse_raw_timezone(raw: &[u8]) -> Option<i32> {
    if raw.len() != 5 {
        return None;
    }
    let sign = match raw[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours = parse_two_decimal_digits(&raw[1..3])?;
    let minutes = parse_two_decimal_digits(&raw[3..5])?;
    Some(sign * (hours * 3600 + minutes * 60))
}

fn parse_decimal_i64(raw: &[u8]) -> Option<i64> {
    if raw.is_empty() {
        return None;
    }
    let mut value = 0i64;
    for byte in raw {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as i64)?;
    }
    Some(value)
}

fn parse_two_decimal_digits(raw: &[u8]) -> Option<i32> {
    if raw.len() != 2 || !raw[0].is_ascii_digit() || !raw[1].is_ascii_digit() {
        return None;
    }
    Some(((raw[0] - b'0') as i32) * 10 + (raw[1] - b'0') as i32)
}

pub(super) fn parse_raw_commit_subject(data: &[u8]) -> String {
    let Some(message_start) = data.windows(2).position(|window| window == b"\n\n") else {
        return String::new();
    };
    let message = &data[message_start + 2..];
    let subject = message
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    String::from_utf8_lossy(subject).into_owned()
}

pub(super) fn find_tree_entry(data: &[u8], component: &[u8]) -> AnyResult<Option<RawTreeEntry>> {
    let mut pos = 0usize;
    while pos < data.len() {
        let mode_start = pos;
        while pos < data.len() && data[pos] != b' ' {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("truncated tree mode".into());
        }
        let mode = parse_tree_mode(&data[mode_start..pos])?;
        let mode_is_tree = mode == 40_000;
        pos += 1;
        let name_start = pos;
        while pos < data.len() && data[pos] != 0 {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("truncated tree name".into());
        }
        let name = &data[name_start..pos];
        pos += 1;
        if pos + 20 > data.len() {
            return Err("truncated tree object id".into());
        }
        let id_start = pos;
        pos += 20;
        if name == component {
            let mut id = [0u8; 20];
            id.copy_from_slice(&data[id_start..id_start + 20]);
            return Ok(Some(RawTreeEntry {
                mode,
                id: RawObjectId(id),
            }));
        }
        if git_tree_name_cmp(name, mode_is_tree, component, true) == Ordering::Greater {
            return Ok(None);
        }
    }
    Ok(None)
}

pub(super) fn parse_tree_entries(data: &[u8]) -> AnyResult<Vec<RawNamedTreeEntry>> {
    let mut pos = 0usize;
    let mut entries = Vec::new();
    while pos < data.len() {
        let mode_start = pos;
        while pos < data.len() && data[pos] != b' ' {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("truncated tree mode".into());
        }
        let mode = parse_tree_mode(&data[mode_start..pos])?;
        pos += 1;
        let name_start = pos;
        while pos < data.len() && data[pos] != 0 {
            pos += 1;
        }
        if pos >= data.len() {
            return Err("truncated tree name".into());
        }
        let name = data[name_start..pos].to_vec();
        pos += 1;
        if pos + 20 > data.len() {
            return Err("truncated tree object id".into());
        }
        let mut id = [0u8; 20];
        id.copy_from_slice(&data[pos..pos + 20]);
        pos += 20;
        entries.push(RawNamedTreeEntry {
            name,
            entry: RawTreeEntry {
                mode,
                id: RawObjectId(id),
            },
        });
    }
    Ok(entries)
}

fn parse_tree_mode(raw: &[u8]) -> AnyResult<u32> {
    match raw {
        b"40000" => return Ok(40_000),
        b"100644" => return Ok(100_644),
        b"100755" => return Ok(100_755),
        b"120000" => return Ok(120_000),
        b"160000" => return Ok(160_000),
        _ => {}
    }
    let mut mode = 0u32;
    for byte in raw {
        if !byte.is_ascii_digit() {
            return Err("invalid tree mode".into());
        }
        mode = mode
            .checked_mul(10)
            .and_then(|value| value.checked_add((byte - b'0') as u32))
            .ok_or("tree mode overflow")?;
    }
    Ok(mode)
}

pub(super) fn raw_kind_from_name(name: &str) -> AnyResult<RawObjectKind> {
    match name {
        "commit" => Ok(RawObjectKind::Commit),
        "tree" => Ok(RawObjectKind::Tree),
        "blob" => Ok(RawObjectKind::Blob),
        "tag" => Ok(RawObjectKind::Tag),
        other => Err(format!("unsupported loose object kind {other:?}").into()),
    }
}

pub(super) fn read_pack_object_from_bytes(pack: &[u8], offset: u64) -> AnyResult<PackedRawObject> {
    let mut pos = usize::try_from(offset)?;
    let first = read_pack_byte(pack, &mut pos)?;
    let type_code = (first >> 4) & 0x07;
    let mut size = (first & 0x0f) as u64;
    let mut shift = 4u32;
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = read_pack_byte(pack, &mut pos)?;
        let factor = 1u64.checked_shl(shift).ok_or("pack object size overflow")?;
        let part = u64::from(byte & 0x7f)
            .checked_mul(factor)
            .ok_or("pack object size overflow")?;
        size = size.checked_add(part).ok_or("pack object size overflow")?;
        shift = shift.checked_add(7).ok_or("pack object size overflow")?;
    }
    validate_git_object_size(size, "pack object")?;
    let base = match type_code {
        6 => Some(PackedBase::Offset(read_ofs_delta_base_offset_from_bytes(
            pack, &mut pos, offset,
        )?)),
        7 => {
            let id = read_pack_slice(pack, &mut pos, 20)?;
            let mut out = [0u8; 20];
            out.copy_from_slice(id);
            Some(PackedBase::Id(RawObjectId(out)))
        }
        _ => None,
    };
    let decoder =
        flate2::bufread::ZlibDecoder::new(pack.get(pos..).ok_or("truncated pack object")?);
    let mut data = Vec::with_capacity(size.min(1024 * 1024) as usize);
    decoder
        .take(size.saturating_add(1))
        .read_to_end(&mut data)?;
    if data.len() as u64 != size {
        return Err(format!(
            "pack object size mismatch: expected {size}, got {}",
            data.len()
        )
        .into());
    }
    Ok(PackedRawObject {
        type_code,
        base,
        data,
    })
}

pub(super) fn validate_git_object_size(size: u64, context: &str) -> AnyResult<()> {
    if size > MAX_GIT_OBJECT_BYTES {
        return Err(format!(
            "{context} declares {size} bytes, exceeding the supported limit of {MAX_GIT_OBJECT_BYTES} bytes"
        )
        .into());
    }
    Ok(())
}

fn read_pack_byte(pack: &[u8], pos: &mut usize) -> AnyResult<u8> {
    let Some(byte) = pack.get(*pos) else {
        return Err("truncated pack byte".into());
    };
    *pos += 1;
    Ok(*byte)
}

fn read_pack_slice<'a>(pack: &'a [u8], pos: &mut usize, len: usize) -> AnyResult<&'a [u8]> {
    let end = pos.checked_add(len).ok_or("pack slice overflow")?;
    let Some(slice) = pack.get(*pos..end) else {
        return Err("truncated pack slice".into());
    };
    *pos = end;
    Ok(slice)
}

pub(super) fn read_ofs_delta_base_offset_from_bytes(
    pack: &[u8],
    pos: &mut usize,
    object_offset: u64,
) -> AnyResult<u64> {
    let mut byte = read_pack_byte(pack, pos)?;
    let mut distance = (byte & 0x7f) as u64;
    while byte & 0x80 != 0 {
        byte = read_pack_byte(pack, pos)?;
        distance = distance
            .checked_add(1)
            .and_then(|value| value.checked_mul(128))
            .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
            .ok_or("ofs-delta distance overflow")?;
    }
    object_offset
        .checked_sub(distance)
        .ok_or_else(|| "invalid ofs-delta base offset".into())
}

pub(super) fn apply_pack_delta(base: &[u8], delta: &[u8]) -> AnyResult<Vec<u8>> {
    let mut pos = 0usize;
    let source_size = read_delta_varint(delta, &mut pos)?;
    let target_size = read_delta_varint(delta, &mut pos)?;
    validate_git_object_size(u64::try_from(target_size)?, "delta target")?;
    if source_size != base.len() {
        return Err(format!(
            "delta source size mismatch: expected {source_size}, got {}",
            base.len()
        )
        .into());
    }
    let mut out = Vec::new();
    out.try_reserve_exact(target_size)
        .map_err(|err| format!("delta target size is too large: {err}"))?;
    while pos < delta.len() {
        let opcode = delta[pos];
        pos += 1;
        if opcode & 0x80 != 0 {
            let mut copy_offset = 0usize;
            let mut copy_size = 0usize;
            for idx in 0..4 {
                if opcode & (1 << idx) != 0 {
                    copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << (idx * 8);
                }
            }
            for idx in 0..3 {
                if opcode & (1 << (4 + idx)) != 0 {
                    copy_size |= (read_delta_byte(delta, &mut pos)? as usize) << (idx * 8);
                }
            }
            if copy_size == 0 {
                copy_size = 0x10000;
            }
            let end = copy_offset
                .checked_add(copy_size)
                .ok_or("delta copy range overflow")?;
            if end > base.len() {
                return Err("delta copy range out of bounds".into());
            }
            out.extend_from_slice(&base[copy_offset..end]);
        } else if opcode != 0 {
            let insert_size = opcode as usize;
            let end = pos
                .checked_add(insert_size)
                .ok_or("delta insert range overflow")?;
            if end > delta.len() {
                return Err("delta insert range out of bounds".into());
            }
            out.extend_from_slice(&delta[pos..end]);
            pos = end;
        } else {
            return Err("invalid zero delta opcode".into());
        }
    }
    if out.len() != target_size {
        return Err(format!(
            "delta target size mismatch: expected {target_size}, got {}",
            out.len()
        )
        .into());
    }
    Ok(out)
}

pub(super) fn read_delta_varint(data: &[u8], pos: &mut usize) -> AnyResult<usize> {
    let mut shift = 0u32;
    let mut out = 0usize;
    loop {
        let byte = read_delta_byte(data, pos)?;
        let factor = 1usize.checked_shl(shift).ok_or("delta varint overflow")?;
        let part = usize::from(byte & 0x7f)
            .checked_mul(factor)
            .ok_or("delta varint overflow")?;
        out = out.checked_add(part).ok_or("delta varint overflow")?;
        if byte & 0x80 == 0 {
            return Ok(out);
        }
        shift = shift.checked_add(7).ok_or("delta varint overflow")?;
    }
}

fn read_delta_byte(data: &[u8], pos: &mut usize) -> AnyResult<u8> {
    let Some(byte) = data.get(*pos) else {
        return Err("truncated delta".into());
    };
    *pos += 1;
    Ok(*byte)
}

pub(super) fn read_be_u32(data: &[u8], offset: usize) -> AnyResult<u32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or("truncated u32")?
        .try_into()?;
    Ok(u32::from_be_bytes(bytes))
}

pub(super) fn read_be_u64(data: &[u8], offset: usize) -> AnyResult<u64> {
    let bytes: [u8; 8] = data
        .get(offset..offset + 8)
        .ok_or("truncated u64")?
        .try_into()?;
    Ok(u64::from_be_bytes(bytes))
}
