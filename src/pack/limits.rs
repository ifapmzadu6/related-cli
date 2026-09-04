//! Resource bounds for pack readers and history traversal.

pub(super) const DEFAULT_PACK_FAST_SCAN_COMMITS: usize = 17_500;
pub(super) const PACK_FAST_MIN_SCAN_COMMITS: usize = 1_000;
pub(super) const PACK_FAST_MIN_TARGET_COMMITS: usize = 256;
pub(super) const PACK_FAST_STALL_COMMITS: usize = 5_000;
pub(super) const PACK_DIRECT_PARALLEL_MIN_COMMITS: usize = 256;
pub(super) const MAX_GIT_OBJECT_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const MAX_PACK_DELTA_DEPTH: usize = 128;
pub(super) const MAX_TREE_DIFF_DEPTH: usize = 256;
pub(super) const MAX_OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_OBJECT_CACHE_ENTRIES: usize = 16_384;
pub(super) const MAX_COMMIT_CACHE_ENTRIES: usize = 32_768;
pub(super) const MAX_TREE_CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_TREE_CACHE_ENTRIES: usize = 8_192;
pub(super) const MAX_PATH_CACHE_ENTRIES: usize = 65_536;
pub(super) const MAX_EXACT_RENAME_TREE_ENTRIES: usize = 20_000;
