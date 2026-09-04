//! Target-path history traversal and exact-blob rename following.

use super::limits::{
    DEFAULT_PACK_FAST_SCAN_COMMITS, MAX_EXACT_RENAME_TREE_ENTRIES, MAX_PATH_CACHE_ENTRIES,
    PACK_FAST_MIN_SCAN_COMMITS, PACK_FAST_MIN_TARGET_COMMITS, PACK_FAST_STALL_COMMITS,
};
use super::store::RawGitStore;
use super::tree::{raw_tree_entry_is_tree, validate_tree_diff_depth};
use super::types::{RawCommit, RawObjectId, RawTreeEntry};
use crate::AnyResult;
use crate::path_utils::decode_git_path;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

pub(super) fn pack_visit_target_commits(
    store: &mut RawGitStore,
    target: &str,
    max_commits: usize,
    since_seconds: Option<i64>,
    scan_commits: usize,
    latency_bounded_scan: bool,
    mut visitor: impl FnMut(&mut RawGitStore, RawObjectId, &RawCommit, &str) -> AnyResult<()>,
) -> AnyResult<()> {
    let mut target_path = PackTargetPath::new(target);
    let head = store.head_id()?;
    let head_commit = store.raw_commit(head)?;
    let mut heap = BinaryHeap::new();
    let mut seen = HashSet::default();
    let mut sequence = 0usize;
    seen.insert(head);
    heap.push(PackWalkItem {
        time: head_commit.time,
        sequence,
        id: head,
    });

    let mut selected = 0usize;
    let mut scanned = 0usize;
    let mut last_hit_scan = 0usize;
    let scan_limit = if latency_bounded_scan && scan_commits == 0 {
        DEFAULT_PACK_FAST_SCAN_COMMITS
    } else {
        scan_commits
    };
    while let Some(item) = heap.pop() {
        let commit = store.raw_commit(item.id)?;
        scanned += 1;
        if scan_limit > 0 && scanned > scan_limit {
            break;
        }
        if since_seconds.is_some_and(|since| commit.time < since) {
            continue;
        }

        let decision = pack_path_history_decision(store, &mut target_path, &commit)?;
        if decision.include {
            let selected_path = decision
                .selected_path
                .as_deref()
                .ok_or("pack path decision omitted the selected target path")?;
            visitor(store, item.id, &commit, selected_path)?;
            selected += 1;
            last_hit_scan = scanned;
            if max_commits > 0 && selected >= max_commits {
                break;
            }
        }
        if latency_bounded_scan
            && scanned >= PACK_FAST_MIN_SCAN_COMMITS
            && (selected >= PACK_FAST_MIN_TARGET_COMMITS
                || (selected > 0
                    && scanned.saturating_sub(last_hit_scan) >= PACK_FAST_STALL_COMMITS))
        {
            break;
        }

        if let Some(parent) = decision.first_parent {
            pack_push_walk_parent(&mut seen, &mut heap, &mut sequence, parent);
        }
        for parent in decision.extra_parents {
            pack_push_walk_parent(&mut seen, &mut heap, &mut sequence, parent);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct PackWalkParent {
    id: RawObjectId,
    time: i64,
}

struct PackPathDecision {
    include: bool,
    selected_path: Option<String>,
    first_parent: Option<PackWalkParent>,
    extra_parents: Vec<PackWalkParent>,
}

impl PackPathDecision {
    fn new(include: bool) -> Self {
        Self {
            include,
            selected_path: None,
            first_parent: None,
            extra_parents: Vec::new(),
        }
    }

    fn one_parent(include: bool, selected_path: Option<String>, parent: PackWalkParent) -> Self {
        Self {
            include,
            selected_path,
            first_parent: Some(parent),
            extra_parents: Vec::new(),
        }
    }

    fn push_parent(&mut self, parent: PackWalkParent) {
        if self.first_parent.is_none() {
            self.first_parent = Some(parent);
        } else {
            self.extra_parents.push(parent);
        }
    }
}

fn pack_push_walk_parent(
    seen: &mut HashSet<RawObjectId>,
    heap: &mut BinaryHeap<PackWalkItem>,
    sequence: &mut usize,
    parent: PackWalkParent,
) {
    if !seen.insert(parent.id) {
        return;
    }
    *sequence += 1;
    heap.push(PackWalkItem {
        time: parent.time,
        sequence: *sequence,
        id: parent.id,
    });
}

struct PackTargetPath {
    path: String,
    components: Vec<Vec<u8>>,
    entry_cache: HashMap<RawObjectId, Option<RawTreeEntry>>,
    child_cache: HashMap<(RawObjectId, usize), Option<RawTreeEntry>>,
}

impl PackTargetPath {
    fn new(target: &str) -> Self {
        Self {
            path: target.to_string(),
            components: target
                .as_bytes()
                .split(|byte| *byte == b'/')
                .filter(|component| !component.is_empty())
                .map(|component| component.to_vec())
                .collect(),
            entry_cache: HashMap::default(),
            child_cache: HashMap::default(),
        }
    }

    fn set_path(&mut self, target: String) {
        self.path = target;
        self.components = self
            .path
            .as_bytes()
            .split(|byte| *byte == b'/')
            .filter(|component| !component.is_empty())
            .map(|component| component.to_vec())
            .collect();
        self.entry_cache.clear();
        self.child_cache.clear();
    }

    fn entry_at_path(
        &mut self,
        store: &mut RawGitStore,
        tree_id: RawObjectId,
    ) -> AnyResult<Option<RawTreeEntry>> {
        if let Some(entry) = self.entry_cache.get(&tree_id) {
            return Ok(*entry);
        }
        let mut current = tree_id;
        let mut found = None;
        for idx in 0..self.components.len() {
            let Some(entry) = self.child_entry(store, current, idx)? else {
                reset_entry_cache_if_full(&mut self.entry_cache);
                self.entry_cache.insert(tree_id, None);
                return Ok(None);
            };
            current = entry.id;
            found = Some(entry);
        }
        reset_entry_cache_if_full(&mut self.entry_cache);
        self.entry_cache.insert(tree_id, found);
        Ok(found)
    }

    fn child_entry(
        &mut self,
        store: &mut RawGitStore,
        tree_id: RawObjectId,
        component_idx: usize,
    ) -> AnyResult<Option<RawTreeEntry>> {
        let key = (tree_id, component_idx);
        if let Some(entry) = self.child_cache.get(&key) {
            return Ok(*entry);
        }
        let entry = store.find_tree_child_entry(tree_id, &self.components[component_idx])?;
        if self.child_cache.len() >= MAX_PATH_CACHE_ENTRIES {
            self.child_cache.clear();
        }
        self.child_cache.insert(key, entry);
        Ok(entry)
    }

    fn treesame_and_new_exists(
        &mut self,
        store: &mut RawGitStore,
        old_tree: RawObjectId,
        new_tree: RawObjectId,
    ) -> AnyResult<(bool, bool)> {
        if old_tree == new_tree {
            return Ok((true, self.entry_at_path(store, new_tree)?.is_some()));
        }
        let mut old_tree = Some(old_tree);
        let mut new_tree = Some(new_tree);
        for idx in 0..self.components.len() {
            let old_entry = old_tree
                .map(|tree| self.child_entry(store, tree, idx))
                .transpose()?
                .flatten();
            let new_entry = new_tree
                .map(|tree| self.child_entry(store, tree, idx))
                .transpose()?
                .flatten();
            if old_entry == new_entry {
                return Ok((true, new_entry.is_some()));
            }
            if idx + 1 == self.components.len() {
                return Ok((false, new_entry.is_some()));
            }
            old_tree = old_entry
                .filter(raw_tree_entry_is_tree)
                .map(|entry| entry.id);
            new_tree = new_entry
                .filter(raw_tree_entry_is_tree)
                .map(|entry| entry.id);
            if old_tree.is_none() && new_tree.is_none() {
                return Ok((true, false));
            }
        }
        Ok((true, false))
    }
}

fn reset_entry_cache_if_full(cache: &mut HashMap<RawObjectId, Option<RawTreeEntry>>) {
    if cache.len() >= MAX_PATH_CACHE_ENTRIES {
        cache.clear();
    }
}

fn pack_path_history_decision(
    store: &mut RawGitStore,
    target: &mut PackTargetPath,
    commit: &RawCommit,
) -> AnyResult<PackPathDecision> {
    if commit.parents.is_empty() {
        let include = target.entry_at_path(store, commit.tree)?.is_some();
        return Ok(PackPathDecision {
            include,
            selected_path: include.then(|| target.path.clone()),
            first_parent: None,
            extra_parents: Vec::new(),
        });
    }

    let mut decision = PackPathDecision::new(false);
    let mut saw_parent = false;
    for parent in commit.parents.iter() {
        if let Ok(parent_commit) = store.raw_commit(parent) {
            saw_parent = true;
            let (treesame, new_exists) =
                target.treesame_and_new_exists(store, parent_commit.tree, commit.tree)?;
            let walk_parent = PackWalkParent {
                id: parent,
                time: parent_commit.time,
            };
            if treesame {
                return Ok(PackPathDecision::one_parent(false, None, walk_parent));
            } else {
                decision.include |= new_exists;
                decision.push_parent(walk_parent);
            }
        }
    }

    if !saw_parent {
        decision.include = target.entry_at_path(store, commit.tree)?.is_some();
    }
    if decision.include {
        decision.selected_path = Some(target.path.clone());
        if commit.parents.len() == 1 {
            let parent = commit.parents.first().ok_or("missing first parent")?;
            let parent_commit = store.raw_commit(parent)?;
            let old_entry = target.entry_at_path(store, parent_commit.tree)?;
            let new_entry = target.entry_at_path(store, commit.tree)?;
            if old_entry.is_none()
                && let Some(new_entry) = new_entry.filter(|entry| !raw_tree_entry_is_tree(entry))
                && let Some(source) = pack_find_exact_rename_source(
                    store,
                    parent_commit.tree,
                    commit.tree,
                    new_entry,
                )?
            {
                target.set_path(source);
            }
        }
    }
    Ok(decision)
}

fn pack_find_exact_rename_source(
    store: &mut RawGitStore,
    old_tree: RawObjectId,
    new_tree: RawObjectId,
    target_entry: RawTreeEntry,
) -> AnyResult<Option<String>> {
    let mut state = ExactRenameSearch {
        target_entry,
        new_tree,
        visited_entries: 0,
        source: None,
        ambiguous_or_bounded: false,
    };
    let mut prefix = Vec::new();
    pack_search_exact_rename_source(store, old_tree, &mut prefix, &mut state, 0)?;
    if state.ambiguous_or_bounded {
        Ok(None)
    } else {
        Ok(state.source)
    }
}

struct ExactRenameSearch {
    target_entry: RawTreeEntry,
    new_tree: RawObjectId,
    visited_entries: usize,
    source: Option<String>,
    ambiguous_or_bounded: bool,
}

fn pack_search_exact_rename_source(
    store: &mut RawGitStore,
    tree: RawObjectId,
    prefix: &mut Vec<u8>,
    state: &mut ExactRenameSearch,
    depth: usize,
) -> AnyResult<()> {
    validate_tree_diff_depth(depth)?;
    if state.ambiguous_or_bounded {
        return Ok(());
    }
    let entries = store.tree_entries(tree)?;
    for entry in entries.iter() {
        state.visited_entries += 1;
        if state.visited_entries > MAX_EXACT_RENAME_TREE_ENTRIES {
            state.ambiguous_or_bounded = true;
            return Ok(());
        }
        let prefix_len = prefix.len();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(&entry.name);
        if raw_tree_entry_is_tree(&entry.entry) {
            pack_search_exact_rename_source(store, entry.entry.id, prefix, state, depth + 1)?;
        } else if entry.entry == state.target_entry {
            let path = decode_git_path(prefix)?;
            let mut candidate = PackTargetPath::new(&path);
            if candidate.entry_at_path(store, state.new_tree)?.is_none() {
                if state.source.is_some() {
                    state.ambiguous_or_bounded = true;
                } else {
                    state.source = Some(path);
                }
            }
        }
        prefix.truncate(prefix_len);
        if state.ambiguous_or_bounded {
            return Ok(());
        }
    }
    Ok(())
}

#[derive(Eq, PartialEq)]
struct PackWalkItem {
    time: i64,
    sequence: usize,
    id: RawObjectId,
}

impl Ord for PackWalkItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time
            .cmp(&other.time)
            .then_with(|| other.sequence.cmp(&self.sequence))
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for PackWalkItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
