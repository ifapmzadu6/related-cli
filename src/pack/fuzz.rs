//! Fuzz entry points for pack decoders and indexes.

use super::objects::{
    apply_pack_delta, parse_raw_commit, parse_tree_entries, read_pack_object_from_bytes,
};
use super::store::PackIndex;
use std::path::PathBuf;

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_parse_bytes(data: &[u8]) {
    let _ = read_pack_object_from_bytes(data, 0);
    if !data.is_empty() {
        let offset = usize::from(data[0]) % data.len();
        let _ = read_pack_object_from_bytes(data, offset as u64);
    }
    let split = data
        .first()
        .map_or(0, |byte| usize::from(*byte) % data.len().saturating_add(1));
    let _ = apply_pack_delta(&data[..split], &data[split..]);
    let _ = parse_raw_commit(data);
    let _ = parse_tree_entries(data);
    let _ = PackIndex::from_data(PathBuf::from("fuzz.idx"), data.to_vec());
}
