//! Pack-native history backend. Storage and decoding are independent of ranking.

mod limits;
mod objects;
mod query;
mod store;
mod tree;
mod types;
mod walk;

pub(crate) use query::{
    git_log_for_target_pack_fast, git_log_for_target_pack_scan, git_pack_fast_direct_for_target,
    git_pack_scan_direct_for_target,
};

#[cfg(feature = "fuzzing")]
mod fuzz;
#[cfg(feature = "fuzzing")]
pub(crate) use fuzz::fuzz_parse_bytes;
#[cfg(test)]
mod tests;
