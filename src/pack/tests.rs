//! Pack reader regression tests.

use super::limits::{MAX_GIT_OBJECT_BYTES, MAX_PACK_DELTA_DEPTH, MAX_TREE_DIFF_DEPTH};
use super::objects::{
    apply_pack_delta, read_delta_varint, read_ofs_delta_base_offset_from_bytes,
    read_pack_object_from_bytes, validate_git_object_size,
};
use super::store::{PackIndex, cache_needs_reset, next_pack_delta_depth};
use super::tree::{git_tree_name_cmp, validate_tree_diff_depth};
use std::io::Write;
use std::path::PathBuf;

#[test]
fn git_tree_name_comparator_matches_directory_sort_rule() {
    assert_eq!(
        git_tree_name_cmp(b"foo.bar", false, b"foo", true),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        git_tree_name_cmp(b"foo", true, b"foo.bar", false),
        std::cmp::Ordering::Greater
    );
    assert_eq!(
        git_tree_name_cmp(b"foo", false, b"foo", true),
        std::cmp::Ordering::Less
    );
    assert_eq!(
        git_tree_name_cmp(b"foo", false, b"foo", false),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn malformed_pack_varints_return_errors() {
    let oversized = vec![0xff; 32];
    assert!(read_pack_object_from_bytes(&oversized, 0).is_err());

    let mut pos = 0;
    assert!(read_ofs_delta_base_offset_from_bytes(&oversized, &mut pos, u64::MAX).is_err());

    let mut pos = 0;
    assert!(read_delta_varint(&oversized, &mut pos).is_err());
}

#[test]
fn pack_object_reader_rejects_declared_size_mismatches_and_large_objects() {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
    encoder.write_all(b"four").unwrap();
    let compressed = encoder.finish().unwrap();
    let mut pack = vec![(3 << 4) | 3];
    pack.extend(compressed);

    let error = read_pack_object_from_bytes(&pack, 0)
        .unwrap_err()
        .to_string();
    assert!(error.contains("pack object size mismatch"));
    assert!(validate_git_object_size(MAX_GIT_OBJECT_BYTES + 1, "test object").is_err());

    let oversized_delta_target = [0, 0x81, 0x80, 0x80, 0x80, 0x01];
    assert!(apply_pack_delta(&[], &oversized_delta_target).is_err());
}

#[test]
fn cache_budgets_reset_at_entry_and_byte_limits() {
    assert!(!cache_needs_reset(40, 60, 9, 10, 100));
    assert!(cache_needs_reset(40, 61, 9, 10, 100));
    assert!(cache_needs_reset(0, 1, 10, 10, 100));
}

#[test]
fn recursive_pack_operations_enforce_depth_limits() {
    let mut depth = 0;
    for _ in 0..MAX_PACK_DELTA_DEPTH {
        depth = next_pack_delta_depth(depth).unwrap();
    }
    assert!(next_pack_delta_depth(depth).is_err());
    assert!(validate_tree_diff_depth(MAX_TREE_DIFF_DEPTH).is_ok());
    assert!(validate_tree_diff_depth(MAX_TREE_DIFF_DEPTH + 1).is_err());
}

#[test]
fn pack_index_rejects_non_monotonic_fanout() {
    let mut data = vec![0; 8 + 256 * 4 + 40];
    data[..4].copy_from_slice(&[0xff, b't', b'O', b'c']);
    data[4..8].copy_from_slice(&2u32.to_be_bytes());
    data[8..12].copy_from_slice(&2u32.to_be_bytes());
    data[12..16].copy_from_slice(&1u32.to_be_bytes());
    let error = match PackIndex::from_data(PathBuf::from("test.idx"), data) {
        Ok(_) => panic!("non-monotonic fanout should fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("non-monotonic idx fanout"));
}
