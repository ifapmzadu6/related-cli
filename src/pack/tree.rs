//! Git tree ordering and bounded recursive diffs.

use super::limits::MAX_TREE_DIFF_DEPTH;
use super::store::RawGitStore;
use super::types::{RawCommit, RawNamedTreeEntry, RawObjectId, RawTreeEntry};
use crate::AnyResult;
use crate::path_utils::decode_git_path;
use std::cmp::Ordering;

pub(super) fn raw_tree_entry_is_tree(entry: &RawTreeEntry) -> bool {
    entry.mode == 40_000
}

fn git_tree_entry_name_cmp(
    left: &RawNamedTreeEntry,
    right_name: &[u8],
    right_is_tree: bool,
) -> Ordering {
    git_tree_name_cmp(
        &left.name,
        raw_tree_entry_is_tree(&left.entry),
        right_name,
        right_is_tree,
    )
}

pub(super) fn git_tree_name_cmp(
    left: &[u8],
    left_is_tree: bool,
    right: &[u8],
    right_is_tree: bool,
) -> Ordering {
    let shared = left.len().min(right.len());
    for idx in 0..shared {
        match left[idx].cmp(&right[idx]) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    let left_next = git_tree_name_next_byte(left, left_is_tree, shared);
    let right_next = git_tree_name_next_byte(right, right_is_tree, shared);
    left_next.cmp(&right_next)
}

fn git_tree_name_next_byte(name: &[u8], is_tree: bool, idx: usize) -> Option<u8> {
    if idx < name.len() {
        Some(name[idx])
    } else if is_tree {
        Some(b'/')
    } else {
        None
    }
}

pub(super) fn pack_changed_files_for_commit(
    store: &mut RawGitStore,
    commit: &RawCommit,
    file_limit: Option<usize>,
) -> AnyResult<Vec<String>> {
    let mut files = Vec::new();
    let mut prefix = Vec::new();
    pack_changed_files_for_commit_into(store, commit, file_limit, &mut files, &mut prefix)?;
    Ok(files)
}

pub(super) fn pack_changed_files_for_commit_into(
    store: &mut RawGitStore,
    commit: &RawCommit,
    file_limit: Option<usize>,
    files: &mut Vec<String>,
    prefix: &mut Vec<u8>,
) -> AnyResult<()> {
    files.clear();
    prefix.clear();
    if commit.parents.len() > 1 {
        // Match `git log --full-diff --name-only` without `-m`, which does not
        // emit a per-parent file list for merge commits.
        return Ok(());
    }
    let old_tree = commit
        .parents
        .first()
        .and_then(|parent| store.raw_commit(parent).ok())
        .map(|parent| parent.tree);
    pack_diff_trees(
        store,
        old_tree,
        Some(commit.tree),
        prefix,
        files,
        file_limit,
    )?;
    Ok(())
}

fn pack_diff_trees(
    store: &mut RawGitStore,
    old_tree: Option<RawObjectId>,
    new_tree: Option<RawObjectId>,
    prefix: &mut Vec<u8>,
    out: &mut Vec<String>,
    file_limit: Option<usize>,
) -> AnyResult<()> {
    pack_diff_trees_at_depth(store, old_tree, new_tree, prefix, out, file_limit, 0)
}

#[allow(clippy::too_many_arguments)]
fn pack_diff_trees_at_depth(
    store: &mut RawGitStore,
    old_tree: Option<RawObjectId>,
    new_tree: Option<RawObjectId>,
    prefix: &mut Vec<u8>,
    out: &mut Vec<String>,
    file_limit: Option<usize>,
    depth: usize,
) -> AnyResult<()> {
    validate_tree_diff_depth(depth)?;
    if file_limit.is_some_and(|limit| out.len() > limit) {
        return Ok(());
    }
    let Some(new_tree) = new_tree else {
        return Ok(());
    };
    if old_tree == Some(new_tree) {
        return Ok(());
    }
    let new_entries = store.tree_entries(new_tree)?;
    let old_entries_arc = if let Some(old_tree) = old_tree {
        Some(store.tree_entries(old_tree)?)
    } else {
        None
    };
    let old_entries: &[RawNamedTreeEntry] = old_entries_arc.as_deref().unwrap_or(&[]);
    let mut old_idx = 0usize;

    for RawNamedTreeEntry { name, entry } in new_entries.iter() {
        let name = name.as_slice();
        while old_idx < old_entries.len()
            && git_tree_entry_name_cmp(&old_entries[old_idx], name, raw_tree_entry_is_tree(entry))
                == Ordering::Less
        {
            old_idx += 1;
        }
        let old_entry = if old_idx < old_entries.len()
            && git_tree_entry_name_cmp(&old_entries[old_idx], name, raw_tree_entry_is_tree(entry))
                == Ordering::Equal
        {
            let entry = old_entries[old_idx].entry;
            old_idx += 1;
            Some(entry)
        } else {
            None
        };
        if old_entry == Some(*entry) {
            continue;
        }
        let prefix_len = prefix.len();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(name);
        if raw_tree_entry_is_tree(entry) {
            let old_child = old_entry
                .filter(raw_tree_entry_is_tree)
                .map(|entry| entry.id);
            pack_diff_trees_at_depth(
                store,
                old_child,
                Some(entry.id),
                prefix,
                out,
                file_limit,
                depth + 1,
            )?;
        } else {
            out.push(decode_git_path(prefix)?);
        }
        prefix.truncate(prefix_len);
        if file_limit.is_some_and(|limit| out.len() > limit) {
            return Ok(());
        }
    }
    Ok(())
}

pub(super) fn validate_tree_diff_depth(depth: usize) -> AnyResult<()> {
    if depth > MAX_TREE_DIFF_DEPTH {
        return Err(
            format!("Git tree depth exceeds the supported limit of {MAX_TREE_DIFF_DEPTH}").into(),
        );
    }
    Ok(())
}
