//! Git CLI and gitoxide history readers.

mod git_cli;
mod gix_reader;
mod parsers;

pub(crate) use git_cli::{
    git_diff_tree_direct_for_target, git_followed_commits_for_targets, git_log,
    git_log_direct_for_target, git_log_direct_for_target_remove_empty, git_log_for_target,
    git_log_for_target_batch, git_log_for_target_batch_parallel, git_log_for_target_diff_tree,
    git_log_for_target_diff_tree_parallel, git_log_for_target_remove_empty,
    git_log_for_target_rev_list, git_log_rename_aware,
};
pub(crate) use gix_reader::{
    format_gix_time, gix_log_for_git_selected_target, gix_log_for_target, parse_gix_since,
};
#[cfg(feature = "fuzzing")]
pub(crate) use parsers::fuzz_parse_bytes;

#[cfg(test)]
pub(crate) use git_cli::{
    git_diff_tree_direct_from_hash_input, git_diff_tree_selected_commits,
    git_target_commit_hash_input, git_target_commit_seeds,
};
