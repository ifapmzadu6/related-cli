use crate::graph::time_decay;
use crate::history::{format_gix_time, parse_gix_since};
use crate::model::*;
use crate::path_utils::{decode_git_path, normalize_git_path};
use crate::{AnyResult, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_FILES};
use rayon::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_PACK_FAST_SCAN_COMMITS: usize = 17_500;
const PACK_FAST_MIN_SCAN_COMMITS: usize = 1_000;
const PACK_FAST_MIN_TARGET_COMMITS: usize = 256;
const PACK_FAST_STALL_COMMITS: usize = 5_000;
const PACK_DIRECT_PARALLEL_MIN_COMMITS: usize = 256;
const MAX_GIT_OBJECT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PACK_DELTA_DEPTH: usize = 128;
const MAX_TREE_DIFF_DEPTH: usize = 256;
const MAX_OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_OBJECT_CACHE_ENTRIES: usize = 16_384;
const MAX_COMMIT_CACHE_ENTRIES: usize = 32_768;
const MAX_TREE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TREE_CACHE_ENTRIES: usize = 8_192;
const MAX_PATH_CACHE_ENTRIES: usize = 65_536;
const MAX_EXACT_RENAME_TREE_ENTRIES: usize = 20_000;

pub(crate) fn git_log_for_target_pack_scan(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
) -> AnyResult<Vec<Commit>> {
    pack_log_for_target(repo, target, max_commits, since, scan_commits)
}

pub(crate) fn git_log_for_target_pack_fast(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
) -> AnyResult<Vec<Commit>> {
    pack_log_for_target_inner(
        repo,
        target,
        max_commits,
        since,
        scan_commits,
        true,
        None,
        true,
    )
}

pub(crate) fn git_pack_scan_direct_for_target(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    pack_direct_for_target_inner(
        repo,
        target,
        config.max_commits,
        config.since.as_deref(),
        config.scan_commits,
        config,
        top,
        false,
    )
}

pub(crate) fn git_pack_fast_direct_for_target(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    pack_direct_for_target_inner(
        repo,
        target,
        config.max_commits,
        config.since.as_deref(),
        config.scan_commits,
        config,
        top,
        true,
    )
}

fn pack_log_for_target(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
) -> AnyResult<Vec<Commit>> {
    pack_log_for_target_inner(
        repo,
        target,
        max_commits,
        since,
        scan_commits,
        true,
        None,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn pack_log_for_target_inner(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
    include_subject: bool,
    diff_file_limit: Option<usize>,
    latency_bounded_scan: bool,
) -> AnyResult<Vec<Commit>> {
    let target = normalize_git_path(target);
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let since_seconds = parse_gix_since(since)?;
    let mut store = RawGitStore::open(repo)?;
    let mut commits = Vec::with_capacity(max_commits.min(1024));
    pack_visit_target_commits(
        &mut store,
        &target,
        max_commits,
        since_seconds,
        scan_commits,
        latency_bounded_scan,
        |store, id, raw, selected_path| {
            let mut files = pack_changed_files_for_commit(store, raw, diff_file_limit)?;
            canonicalize_pack_target_path(&mut files, selected_path, &target);
            commits.push(Commit {
                hash: id.to_hex(),
                unix_time: raw.time,
                date: format_gix_time(gix::date::Time {
                    seconds: raw.time,
                    offset: raw.offset,
                }),
                subject: if include_subject {
                    store.raw_commit_subject(id)?
                } else {
                    String::new()
                },
                files,
            });
            Ok(())
        },
    )?;
    Ok(commits)
}

#[allow(clippy::too_many_arguments)]
fn pack_direct_for_target_inner(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
    config: &OnDemandConfig,
    top: usize,
    latency_bounded_scan: bool,
) -> AnyResult<Vec<ResultItem>> {
    let target = normalize_git_path(target);
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let since_seconds = parse_gix_since(since)?;
    let max_files = effective_max_files(config);
    let half_life = if config.half_life_days <= 0.0 {
        DEFAULT_HALF_LIFE_DAYS
    } else {
        config.half_life_days
    };
    let mut store = RawGitStore::open(repo)?;
    if !latency_bounded_scan
        && config.jobs_explicit
        && config.jobs > 1
        && config.evidence_limit == 0
    {
        let selected = pack_collect_target_commits(
            &mut store,
            &target,
            max_commits,
            since_seconds,
            scan_commits,
            latency_bounded_scan,
        )?;
        if selected.len() >= PACK_DIRECT_PARALLEL_MIN_COMMITS {
            return pack_direct_for_selected_parallel(
                &store,
                &selected,
                max_files,
                half_life,
                top,
                config.jobs,
            );
        }
        return pack_direct_for_selected_serial(&mut store, &selected, max_files, half_life, top);
    }

    let mut latest = None;
    let mut target_weight = 0.0;
    let mut pairs: HashMap<String, PackDirectPairStat> =
        HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default());
    let diff_file_limit = Some(max_files.saturating_add(1));
    let mut files = Vec::with_capacity(max_files.saturating_add(1));
    let mut prefix = Vec::new();

    pack_visit_target_commits(
        &mut store,
        &target,
        max_commits,
        since_seconds,
        scan_commits,
        latency_bounded_scan,
        |store, id, raw, selected_path| {
            pack_changed_files_for_commit_into(
                store,
                raw,
                diff_file_limit,
                &mut files,
                &mut prefix,
            )?;
            if files.is_empty() || files.len() > max_files {
                return Ok(());
            }

            let latest = *latest.get_or_insert(raw.time);
            let decay = time_decay(latest, raw.time, half_life);
            target_weight += decay;
            let file_count = files.len();
            let pair_weight = decay / ((file_count + 1) as f64).log2();
            let mut evidence = None;

            for other in &files {
                if other == selected_path {
                    continue;
                }
                let pair = pairs.entry(other.clone()).or_default();
                pair.cochanges += 1;
                pair.weight += pair_weight;
                pair.other_weight += decay;
                if pair
                    .last_seen_time
                    .is_none_or(|last_seen| raw.time > last_seen)
                {
                    pair.last_seen_time = Some(raw.time);
                    pair.last_seen_offset = raw.offset;
                }
                if pair.evidence.len() < config.evidence_limit {
                    if evidence.is_none() {
                        evidence = Some(Evidence {
                            hash: id.to_hex(),
                            date: format_gix_time(gix::date::Time {
                                seconds: raw.time,
                                offset: raw.offset,
                            }),
                            subject: store.raw_commit_subject(id)?,
                            file_count,
                            weight: pair_weight,
                        });
                    }
                    if let Some(evidence) = &evidence {
                        pair.evidence.push(evidence.clone());
                    }
                }
            }
            Ok(())
        },
    )?;

    let mut scored = Vec::with_capacity(pairs.len());
    for (path, pair) in pairs {
        let score = if target_weight <= 0.0 || pair.other_weight <= 0.0 {
            pair.weight
        } else {
            pair.weight / (target_weight * pair.other_weight).sqrt()
        };
        scored.push(PackDirectScoredPair { path, pair, score });
    }
    truncate_top_pack_direct_pairs(&mut scored, top);
    Ok(scored
        .into_iter()
        .map(|item| pack_direct_pair_result(item.pair, item.path, item.score, "direct_cochange"))
        .collect())
}

fn pack_collect_target_commits(
    store: &mut RawGitStore,
    target: &str,
    max_commits: usize,
    since_seconds: Option<i64>,
    scan_commits: usize,
    latency_bounded_scan: bool,
) -> AnyResult<Vec<(RawObjectId, RawCommit, String)>> {
    let mut selected = Vec::with_capacity(max_commits.min(1024));
    pack_visit_target_commits(
        store,
        target,
        max_commits,
        since_seconds,
        scan_commits,
        latency_bounded_scan,
        |_, id, raw, selected_path| {
            selected.push((id, raw.clone(), selected_path.to_string()));
            Ok(())
        },
    )?;
    Ok(selected)
}

fn pack_direct_for_selected_serial(
    store: &mut RawGitStore,
    selected: &[(RawObjectId, RawCommit, String)],
    max_files: usize,
    half_life: f64,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    let Some((_, latest_commit, _)) = selected.first() else {
        return Ok(Vec::new());
    };
    let latest = latest_commit.time;
    let diff_file_limit = Some(max_files.saturating_add(1));
    let mut partial = PackDirectPartial::new(top);
    let mut files = Vec::with_capacity(max_files.saturating_add(1));
    let mut prefix = Vec::new();
    for (_, raw, selected_path) in selected {
        pack_direct_add_commit_no_evidence(
            store,
            selected_path,
            raw,
            max_files,
            half_life,
            latest,
            diff_file_limit,
            &mut partial,
            &mut files,
            &mut prefix,
        )?;
    }
    Ok(pack_direct_results_from_parts(
        partial.pairs,
        partial.target_weight,
        top,
    ))
}

fn pack_direct_for_selected_parallel(
    template: &RawGitStore,
    selected: &[(RawObjectId, RawCommit, String)],
    max_files: usize,
    half_life: f64,
    top: usize,
    jobs: usize,
) -> AnyResult<Vec<ResultItem>> {
    let Some((_, latest_commit, _)) = selected.first() else {
        return Ok(Vec::new());
    };
    let latest = latest_commit.time;
    let jobs = jobs.min(selected.len()).max(1);
    let chunk_size = selected.len().div_ceil(jobs).max(1);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
    let partials: Vec<PackDirectPartial> = pool.install(|| -> Result<_, String> {
        selected
            .par_chunks(chunk_size)
            .map(|chunk| {
                let mut store = template.fork_empty();
                let diff_file_limit = Some(max_files.saturating_add(1));
                let mut partial = PackDirectPartial::new(top);
                let mut files = Vec::with_capacity(max_files.saturating_add(1));
                let mut prefix = Vec::new();
                for (_, raw, selected_path) in chunk {
                    pack_direct_add_commit_no_evidence(
                        &mut store,
                        selected_path,
                        raw,
                        max_files,
                        half_life,
                        latest,
                        diff_file_limit,
                        &mut partial,
                        &mut files,
                        &mut prefix,
                    )
                    .map_err(|err| err.to_string())?;
                }
                Ok(partial)
            })
            .collect::<Result<Vec<_>, _>>()
    })?;

    let mut merged = PackDirectPartial::new(top);
    for partial in partials {
        pack_direct_merge_partial(&mut merged, partial);
    }
    Ok(pack_direct_results_from_parts(
        merged.pairs,
        merged.target_weight,
        top,
    ))
}

#[allow(clippy::too_many_arguments)]
fn pack_direct_add_commit_no_evidence(
    store: &mut RawGitStore,
    target: &str,
    raw: &RawCommit,
    max_files: usize,
    half_life: f64,
    latest: i64,
    diff_file_limit: Option<usize>,
    partial: &mut PackDirectPartial,
    files: &mut Vec<String>,
    prefix: &mut Vec<u8>,
) -> AnyResult<()> {
    pack_changed_files_for_commit_into(store, raw, diff_file_limit, files, prefix)?;
    if files.is_empty() || files.len() > max_files {
        return Ok(());
    }
    let decay = time_decay(latest, raw.time, half_life);
    partial.target_weight += decay;
    let pair_weight = decay / ((files.len() + 1) as f64).log2();
    for other in files.iter() {
        if other == target {
            continue;
        }
        let pair = partial.pairs.entry(other.clone()).or_default();
        pair.cochanges += 1;
        pair.weight += pair_weight;
        pair.other_weight += decay;
        if pair
            .last_seen_time
            .is_none_or(|last_seen| raw.time > last_seen)
        {
            pair.last_seen_time = Some(raw.time);
            pair.last_seen_offset = raw.offset;
        }
    }
    Ok(())
}

fn canonicalize_pack_target_path(files: &mut [String], selected_path: &str, target: &str) {
    if selected_path == target {
        return;
    }
    for file in files {
        if file == selected_path {
            target.clone_into(file);
        }
    }
}

fn pack_direct_merge_partial(target: &mut PackDirectPartial, source: PackDirectPartial) {
    target.target_weight += source.target_weight;
    for (path, source_pair) in source.pairs {
        let pair = target.pairs.entry(path).or_default();
        pair.cochanges += source_pair.cochanges;
        pair.weight += source_pair.weight;
        pair.other_weight += source_pair.other_weight;
        match (source_pair.last_seen_time, pair.last_seen_time) {
            (Some(source_seen), Some(target_seen)) if source_seen > target_seen => {
                pair.last_seen_time = Some(source_seen);
                pair.last_seen_offset = source_pair.last_seen_offset;
            }
            (Some(source_seen), None) => {
                pair.last_seen_time = Some(source_seen);
                pair.last_seen_offset = source_pair.last_seen_offset;
            }
            _ => {}
        }
    }
}

fn pack_direct_results_from_parts(
    pairs: HashMap<String, PackDirectPairStat>,
    target_weight: f64,
    top: usize,
) -> Vec<ResultItem> {
    let mut scored = Vec::with_capacity(pairs.len());
    for (path, pair) in pairs {
        let score = if target_weight <= 0.0 || pair.other_weight <= 0.0 {
            pair.weight
        } else {
            pair.weight / (target_weight * pair.other_weight).sqrt()
        };
        scored.push(PackDirectScoredPair { path, pair, score });
    }
    truncate_top_pack_direct_pairs(&mut scored, top);
    scored
        .into_iter()
        .map(|item| pack_direct_pair_result(item.pair, item.path, item.score, "direct_cochange"))
        .collect()
}

fn pack_visit_target_commits(
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

fn raw_tree_entry_is_tree(entry: &RawTreeEntry) -> bool {
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

fn git_tree_name_cmp(
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

fn pack_changed_files_for_commit(
    store: &mut RawGitStore,
    commit: &RawCommit,
    file_limit: Option<usize>,
) -> AnyResult<Vec<String>> {
    let mut files = Vec::new();
    let mut prefix = Vec::new();
    pack_changed_files_for_commit_into(store, commit, file_limit, &mut files, &mut prefix)?;
    Ok(files)
}

fn pack_changed_files_for_commit_into(
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct RawObjectId([u8; 20]);

impl RawObjectId {
    fn from_hex_str(value: &str) -> AnyResult<Self> {
        Self::from_hex(value.as_bytes())
    }

    fn from_hex(value: &[u8]) -> AnyResult<Self> {
        if value.len() != 40 {
            return Err(format!("expected 40 hex bytes, got {}", value.len()).into());
        }
        let mut out = [0u8; 20];
        for (idx, slot) in out.iter_mut().enumerate() {
            *slot = (hex_nibble(value[idx * 2])? << 4) | hex_nibble(value[idx * 2 + 1])?;
        }
        Ok(Self(out))
    }

    fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(40);
        for byte in self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

fn hex_nibble(byte: u8) -> AnyResult<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex byte {:?}", byte as char).into()),
    }
}

fn parse_hex_byte(value: &[u8]) -> AnyResult<u8> {
    if value.len() != 2 {
        return Err(format!("expected 2 hex bytes, got {}", value.len()).into());
    }
    Ok((hex_nibble(value[0])? << 4) | hex_nibble(value[1])?)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RawObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

#[derive(Clone, Debug)]
struct RawGitObject {
    kind: RawObjectKind,
    data: Arc<[u8]>,
}

#[derive(Clone, Debug)]
struct RawCommit {
    tree: RawObjectId,
    parents: RawParents,
    time: i64,
    offset: i32,
}

#[derive(Clone, Debug, Default)]
struct RawParents {
    first: Option<RawObjectId>,
    extra: Vec<RawObjectId>,
}

impl RawParents {
    fn push(&mut self, parent: RawObjectId) {
        if self.first.is_none() {
            self.first = Some(parent);
        } else {
            self.extra.push(parent);
        }
    }

    fn is_empty(&self) -> bool {
        self.first.is_none()
    }

    fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.extra.len()
    }

    fn first(&self) -> Option<RawObjectId> {
        self.first
    }

    fn iter(&self) -> RawParentsIter<'_> {
        RawParentsIter {
            first: self.first,
            extra: self.extra.iter(),
        }
    }
}

struct RawParentsIter<'a> {
    first: Option<RawObjectId>,
    extra: std::slice::Iter<'a, RawObjectId>,
}

impl Iterator for RawParentsIter<'_> {
    type Item = RawObjectId;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(first) = self.first.take() {
            return Some(first);
        }
        self.extra.next().copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RawTreeEntry {
    mode: u32,
    id: RawObjectId,
}

#[derive(Clone, Debug)]
struct RawNamedTreeEntry {
    name: Vec<u8>,
    entry: RawTreeEntry,
}

#[derive(Clone, Debug)]
struct PackedRawObject {
    type_code: u8,
    base: Option<PackedBase>,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
enum PackedBase {
    Offset(u64),
    Id(RawObjectId),
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

struct RawGitStore {
    git_dir: PathBuf,
    common_dir: PathBuf,
    object_dir: PathBuf,
    indexes: Arc<[PackIndex]>,
    loose_prefixes: [bool; 256],
    object_cache: HashMap<RawObjectId, RawGitObject>,
    object_cache_bytes: usize,
    commit_cache: HashMap<RawObjectId, RawCommit>,
    tree_entries_cache: HashMap<RawObjectId, Arc<[RawNamedTreeEntry]>>,
    tree_entries_cache_bytes: usize,
    offset_cache: HashMap<(usize, u64), RawGitObject>,
    offset_cache_bytes: usize,
    pack_maps: Vec<Option<Arc<memmap2::Mmap>>>,
}

impl RawGitStore {
    fn open(repo: &str) -> AnyResult<Self> {
        let repo_root = Path::new(repo);
        let git_dir = resolve_git_dir(repo_root)?;
        let common_dir = resolve_common_dir(&git_dir)?;
        let object_dir = common_dir.join("objects");
        let indexes = load_pack_indexes(&object_dir)?;
        let loose_prefixes = load_loose_prefixes(&object_dir)?;
        Ok(Self {
            git_dir,
            common_dir,
            object_dir,
            pack_maps: std::iter::repeat_with(|| None)
                .take(indexes.len())
                .collect(),
            indexes,
            loose_prefixes,
            object_cache: HashMap::default(),
            object_cache_bytes: 0,
            commit_cache: HashMap::default(),
            tree_entries_cache: HashMap::default(),
            tree_entries_cache_bytes: 0,
            offset_cache: HashMap::default(),
            offset_cache_bytes: 0,
        })
    }

    fn fork_empty(&self) -> Self {
        Self {
            git_dir: self.git_dir.clone(),
            common_dir: self.common_dir.clone(),
            object_dir: self.object_dir.clone(),
            pack_maps: self.pack_maps.clone(),
            indexes: Arc::clone(&self.indexes),
            loose_prefixes: self.loose_prefixes,
            object_cache: HashMap::default(),
            object_cache_bytes: 0,
            commit_cache: HashMap::default(),
            tree_entries_cache: HashMap::default(),
            tree_entries_cache_bytes: 0,
            offset_cache: HashMap::default(),
            offset_cache_bytes: 0,
        }
    }

    fn head_id(&self) -> AnyResult<RawObjectId> {
        let head = fs::read_to_string(self.git_dir.join("HEAD"))?;
        let head = head.trim();
        if let Some(reference) = head.strip_prefix("ref: ") {
            return self.resolve_ref(reference.trim());
        }
        RawObjectId::from_hex_str(head)
    }

    fn resolve_ref(&self, reference: &str) -> AnyResult<RawObjectId> {
        for base in [&self.git_dir, &self.common_dir] {
            let path = base.join(reference);
            if path.is_file() {
                return RawObjectId::from_hex_str(fs::read_to_string(path)?.trim());
            }
        }
        for base in [&self.git_dir, &self.common_dir] {
            let path = base.join("packed-refs");
            if let Some(id) = packed_ref_id(&path, reference)? {
                return Ok(id);
            }
        }
        Err(format!("could not resolve ref {reference:?}").into())
    }

    fn raw_commit(&mut self, id: RawObjectId) -> AnyResult<RawCommit> {
        if let Some(commit) = self.commit_cache.get(&id) {
            return Ok(commit.clone());
        }
        let commit = {
            let object = self.find_object_ref(id)?;
            if object.kind != RawObjectKind::Commit {
                return Err(format!("object {} is not a commit", id.to_hex()).into());
            }
            parse_raw_commit(&object.data)?
        };
        if self.commit_cache.len() >= MAX_COMMIT_CACHE_ENTRIES {
            self.commit_cache.clear();
        }
        self.commit_cache.insert(id, commit.clone());
        Ok(commit)
    }

    fn raw_commit_subject(&mut self, id: RawObjectId) -> AnyResult<String> {
        let object = self.find_object_ref(id)?;
        if object.kind != RawObjectKind::Commit {
            return Err(format!("object {} is not a commit", id.to_hex()).into());
        }
        Ok(parse_raw_commit_subject(&object.data))
    }

    fn find_tree_child_entry(
        &mut self,
        tree_id: RawObjectId,
        name: &[u8],
    ) -> AnyResult<Option<RawTreeEntry>> {
        let object = self.find_object_ref(tree_id)?;
        if object.kind != RawObjectKind::Tree {
            return Ok(None);
        }
        find_tree_entry(&object.data, name)
    }

    fn tree_entries(&mut self, tree_id: RawObjectId) -> AnyResult<Arc<[RawNamedTreeEntry]>> {
        if let Some(entries) = self.tree_entries_cache.get(&tree_id) {
            return Ok(Arc::clone(entries));
        }
        let entries = {
            let object = self.find_object_ref(tree_id)?;
            if object.kind != RawObjectKind::Tree {
                return Err(format!("object {} is not a tree", tree_id.to_hex()).into());
            }
            parse_tree_entries(&object.data)?
        };
        let entries: Arc<[RawNamedTreeEntry]> = entries.into();
        let entries_bytes = entries
            .iter()
            .fold(std::mem::size_of_val(entries.as_ref()), |total, entry| {
                total.saturating_add(entry.name.len())
            });
        if cache_needs_reset(
            self.tree_entries_cache_bytes,
            entries_bytes,
            self.tree_entries_cache.len(),
            MAX_TREE_CACHE_ENTRIES,
            MAX_TREE_CACHE_BYTES,
        ) {
            self.tree_entries_cache.clear();
            self.tree_entries_cache_bytes = 0;
        }
        self.tree_entries_cache_bytes = self.tree_entries_cache_bytes.saturating_add(entries_bytes);
        self.tree_entries_cache
            .insert(tree_id, Arc::clone(&entries));
        Ok(entries)
    }

    fn find_object_ref(&mut self, id: RawObjectId) -> AnyResult<&RawGitObject> {
        if !self.object_cache.contains_key(&id) {
            let object = self.load_object_at_delta_depth(id, 0)?;
            self.cache_object(id, object);
        }
        match self.object_cache.get(&id) {
            Some(object) => Ok(object),
            None => Err(format!("object {} not found after load", id.to_hex()).into()),
        }
    }

    fn find_object_at_delta_depth(
        &mut self,
        id: RawObjectId,
        depth: usize,
    ) -> AnyResult<RawGitObject> {
        if let Some(object) = self.object_cache.get(&id) {
            return Ok(object.clone());
        }
        let object = self.load_object_at_delta_depth(id, depth)?;
        self.cache_object(id, object.clone());
        Ok(object)
    }

    fn cache_object(&mut self, id: RawObjectId, object: RawGitObject) {
        if cache_needs_reset(
            self.object_cache_bytes,
            object.data.len(),
            self.object_cache.len(),
            MAX_OBJECT_CACHE_ENTRIES,
            MAX_OBJECT_CACHE_BYTES,
        ) {
            self.object_cache.clear();
            self.object_cache_bytes = 0;
        }
        self.object_cache_bytes = self.object_cache_bytes.saturating_add(object.data.len());
        self.object_cache.insert(id, object);
    }

    fn load_object_at_delta_depth(
        &mut self,
        id: RawObjectId,
        depth: usize,
    ) -> AnyResult<RawGitObject> {
        Ok(if let Some(object) = self.find_loose_object(id)? {
            object
        } else if let Some((pack_index, offset)) = self.find_pack_offset(id) {
            self.find_pack_object_at_depth(pack_index, offset, depth)?
        } else {
            return Err(format!("object {} not found", id.to_hex()).into());
        })
    }

    fn find_pack_offset(&self, id: RawObjectId) -> Option<(usize, u64)> {
        self.indexes
            .iter()
            .enumerate()
            .find_map(|(pack_index, index)| {
                index.lookup_offset(id).map(|offset| (pack_index, offset))
            })
    }

    fn find_loose_object(&self, id: RawObjectId) -> AnyResult<Option<RawGitObject>> {
        if !self.loose_prefixes[id.0[0] as usize] {
            return Ok(None);
        }
        let hex = id.to_hex();
        let path = self.object_dir.join(&hex[..2]).join(&hex[2..]);
        if !path.is_file() {
            return Ok(None);
        }
        let file = File::open(path)?;
        let decoder = flate2::read::ZlibDecoder::new(file);
        let mut inflated = Vec::new();
        decoder
            .take(MAX_GIT_OBJECT_BYTES.saturating_add(1024).saturating_add(1))
            .read_to_end(&mut inflated)?;
        if inflated.len() as u64 > MAX_GIT_OBJECT_BYTES.saturating_add(1024) {
            return Err(format!(
                "loose object {} exceeds the supported size limit of {MAX_GIT_OBJECT_BYTES} bytes",
                id.to_hex()
            )
            .into());
        }
        let Some(header_end) = inflated.iter().position(|byte| *byte == 0) else {
            return Err("loose object missing header delimiter".into());
        };
        let header = std::str::from_utf8(&inflated[..header_end])?;
        let (kind, declared_size) = header
            .split_once(' ')
            .ok_or("loose object header must contain kind and size")?;
        let kind = raw_kind_from_name(kind)?;
        let declared_size: u64 = declared_size.parse()?;
        validate_git_object_size(declared_size, "loose object")?;
        let data_start = header_end + 1;
        let data_len = inflated.len() - data_start;
        if data_len as u64 != declared_size {
            return Err(format!(
                "loose object size mismatch: expected {declared_size}, got {}",
                data_len
            )
            .into());
        }
        inflated.drain(..data_start);
        Ok(Some(RawGitObject {
            kind,
            data: inflated.into(),
        }))
    }

    fn find_pack_object_at_depth(
        &mut self,
        pack_index: usize,
        offset: u64,
        depth: usize,
    ) -> AnyResult<RawGitObject> {
        let cache_key = (pack_index, offset);
        if let Some(object) = self.offset_cache.get(&cache_key) {
            return Ok(object.clone());
        }
        let raw = self.read_pack_object(pack_index, offset)?;
        let object = match raw.type_code {
            1 => RawGitObject {
                kind: RawObjectKind::Commit,
                data: raw.data.into(),
            },
            2 => RawGitObject {
                kind: RawObjectKind::Tree,
                data: raw.data.into(),
            },
            3 => RawGitObject {
                kind: RawObjectKind::Blob,
                data: raw.data.into(),
            },
            4 => RawGitObject {
                kind: RawObjectKind::Tag,
                data: raw.data.into(),
            },
            6 | 7 => {
                let Some(base) = raw.base else {
                    return Err("delta object missing base".into());
                };
                let base_depth = next_pack_delta_depth(depth)?;
                let base = match base {
                    PackedBase::Offset(base_offset) => {
                        self.find_pack_object_at_depth(pack_index, base_offset, base_depth)?
                    }
                    PackedBase::Id(base_id) => {
                        self.find_object_at_delta_depth(base_id, base_depth)?
                    }
                };
                RawGitObject {
                    kind: base.kind,
                    data: apply_pack_delta(&base.data, &raw.data)?.into(),
                }
            }
            other => return Err(format!("unsupported pack object type {other}").into()),
        };
        if cache_needs_reset(
            self.offset_cache_bytes,
            object.data.len(),
            self.offset_cache.len(),
            MAX_OBJECT_CACHE_ENTRIES,
            MAX_OBJECT_CACHE_BYTES,
        ) {
            self.offset_cache.clear();
            self.offset_cache_bytes = 0;
        }
        self.offset_cache_bytes = self.offset_cache_bytes.saturating_add(object.data.len());
        self.offset_cache.insert(cache_key, object.clone());
        Ok(object)
    }

    fn read_pack_object(&mut self, pack_index: usize, offset: u64) -> AnyResult<PackedRawObject> {
        if pack_index >= self.indexes.len() {
            return Err(format!("pack index {pack_index} out of range").into());
        }
        if self.pack_maps[pack_index].is_none() {
            let pack_path = &self.indexes[pack_index].pack_path;
            let file = File::open(pack_path)?;
            let mmap = unsafe { memmap2::Mmap::map(&file)? };
            self.pack_maps[pack_index] = Some(Arc::new(mmap));
        }
        let mmap = self
            .pack_maps
            .get(pack_index)
            .and_then(Option::as_ref)
            .ok_or("failed to open cached pack map")?;
        read_pack_object_from_bytes(mmap, offset)
    }
}

fn cache_needs_reset(
    current_bytes: usize,
    incoming_bytes: usize,
    entries: usize,
    max_entries: usize,
    max_bytes: usize,
) -> bool {
    entries >= max_entries || current_bytes.saturating_add(incoming_bytes) > max_bytes
}

fn next_pack_delta_depth(depth: usize) -> AnyResult<usize> {
    let next = depth.checked_add(1).ok_or("pack delta depth overflow")?;
    if next > MAX_PACK_DELTA_DEPTH {
        return Err(format!(
            "pack delta chain exceeds the supported depth of {MAX_PACK_DELTA_DEPTH}"
        )
        .into());
    }
    Ok(next)
}

fn validate_tree_diff_depth(depth: usize) -> AnyResult<()> {
    if depth > MAX_TREE_DIFF_DEPTH {
        return Err(
            format!("Git tree depth exceeds the supported limit of {MAX_TREE_DIFF_DEPTH}").into(),
        );
    }
    Ok(())
}

struct PackIndex {
    pack_path: PathBuf,
    data: Vec<u8>,
    fanout: [u32; 256],
    count: usize,
    names_start: usize,
    offsets_start: usize,
    large_offsets_start: usize,
}

impl PackIndex {
    fn open(idx_path: PathBuf) -> AnyResult<Self> {
        let data = fs::read(&idx_path)?;
        Self::from_data(idx_path, data)
    }

    fn from_data(idx_path: PathBuf, data: Vec<u8>) -> AnyResult<Self> {
        if data.len() < 8 + 256 * 4 {
            return Err(format!("idx file too small: {}", idx_path.display()).into());
        }
        if data.get(0..4) != Some(&[0xff, b't', b'O', b'c']) {
            return Err(format!("unsupported idx magic: {}", idx_path.display()).into());
        }
        if read_be_u32(&data, 4)? != 2 {
            return Err(format!("unsupported idx version: {}", idx_path.display()).into());
        }
        let mut fanout = [0u32; 256];
        for (idx, slot) in fanout.iter_mut().enumerate() {
            *slot = read_be_u32(&data, 8 + idx * 4)?;
        }
        if fanout.windows(2).any(|pair| pair[0] > pair[1]) {
            return Err(format!("non-monotonic idx fanout: {}", idx_path.display()).into());
        }
        let count = fanout[255] as usize;
        let names_start: usize = 8 + 256 * 4;
        let names_bytes = count
            .checked_mul(20)
            .ok_or("idx name table size overflow")?;
        let crc_bytes = count.checked_mul(4).ok_or("idx CRC table size overflow")?;
        let offsets_bytes = count
            .checked_mul(4)
            .ok_or("idx offset table size overflow")?;
        let crc_start = names_start
            .checked_add(names_bytes)
            .ok_or("idx name table offset overflow")?;
        let offsets_start = crc_start
            .checked_add(crc_bytes)
            .ok_or("idx CRC table offset overflow")?;
        let large_offsets_start = offsets_start
            .checked_add(offsets_bytes)
            .ok_or("idx offset table offset overflow")?;
        let minimum_size = large_offsets_start
            .checked_add(40)
            .ok_or("idx trailer offset overflow")?;
        if data.len() < minimum_size {
            return Err(format!("truncated idx file: {}", idx_path.display()).into());
        }
        let pack_path = idx_path.with_extension("pack");
        Ok(Self {
            pack_path,
            data,
            fanout,
            count,
            names_start,
            offsets_start,
            large_offsets_start,
        })
    }

    fn lookup_offset(&self, id: RawObjectId) -> Option<u64> {
        let bucket = id.0[0] as usize;
        let start = if bucket == 0 {
            0
        } else {
            self.fanout[bucket - 1] as usize
        };
        let end = self.fanout[bucket] as usize;
        let mut left = start;
        let mut right = end;
        while left < right {
            let mid = (left + right) / 2;
            let raw = self.object_id_slice(mid)?;
            match raw.cmp(id.0.as_slice()) {
                Ordering::Less => left = mid + 1,
                Ordering::Greater => right = mid,
                Ordering::Equal => return self.offset_at(mid).ok(),
            }
        }
        None
    }

    fn object_id_slice(&self, index: usize) -> Option<&[u8]> {
        if index >= self.count {
            return None;
        }
        let start = self.names_start + index * 20;
        self.data.get(start..start + 20)
    }

    fn offset_at(&self, index: usize) -> AnyResult<u64> {
        if index >= self.count {
            return Err("pack index offset out of range".into());
        }
        let raw = read_be_u32(&self.data, self.offsets_start + index * 4)?;
        if raw & 0x8000_0000 == 0 {
            return Ok(raw as u64);
        }
        let large_index = (raw & 0x7fff_ffff) as usize;
        read_be_u64(&self.data, self.large_offsets_start + large_index * 8)
    }
}

fn resolve_git_dir(repo_root: &Path) -> AnyResult<PathBuf> {
    let dot_git = repo_root.join(".git");
    if dot_git.is_dir() {
        return Ok(dot_git);
    }
    let text = fs::read_to_string(&dot_git)?;
    let Some(path) = text.trim().strip_prefix("gitdir:") else {
        return Err(format!("unsupported .git file: {}", dot_git.display()).into());
    };
    Ok(resolve_relative_path(repo_root, path.trim()))
}

fn resolve_common_dir(git_dir: &Path) -> AnyResult<PathBuf> {
    let common_dir = git_dir.join("commondir");
    if !common_dir.is_file() {
        return Ok(git_dir.to_path_buf());
    }
    Ok(resolve_relative_path(
        git_dir,
        fs::read_to_string(common_dir)?.trim(),
    ))
}

fn resolve_relative_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn load_pack_indexes(object_dir: &Path) -> AnyResult<Arc<[PackIndex]>> {
    let pack_dir = object_dir.join("pack");
    if !pack_dir.is_dir() {
        return Ok(Vec::new().into());
    }
    let mut paths = fs::read_dir(pack_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "idx"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut indexes = paths
        .into_iter()
        .map(PackIndex::open)
        .collect::<AnyResult<Vec<_>>>()?;
    indexes.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.pack_path.cmp(&right.pack_path))
    });
    Ok(indexes.into())
}

fn load_loose_prefixes(object_dir: &Path) -> AnyResult<[bool; 256]> {
    let mut prefixes = [false; 256];
    if !object_dir.is_dir() {
        return Ok(prefixes);
    }
    for entry in fs::read_dir(object_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() != 2 {
            continue;
        }
        if let Ok(prefix) = parse_hex_byte(name.as_bytes()) {
            prefixes[prefix as usize] = true;
        }
    }
    Ok(prefixes)
}

fn packed_ref_id(path: &Path, reference: &str) -> AnyResult<Option<RawObjectId>> {
    if !path.is_file() {
        return Ok(None);
    }
    for line in fs::read_to_string(path)?.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((id, name)) = line.split_once(' ') else {
            continue;
        };
        if name == reference {
            return Ok(Some(RawObjectId::from_hex_str(id)?));
        }
    }
    Ok(None)
}

fn parse_raw_commit(data: &[u8]) -> AnyResult<RawCommit> {
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

fn parse_raw_commit_subject(data: &[u8]) -> String {
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

fn find_tree_entry(data: &[u8], component: &[u8]) -> AnyResult<Option<RawTreeEntry>> {
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

fn parse_tree_entries(data: &[u8]) -> AnyResult<Vec<RawNamedTreeEntry>> {
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

fn raw_kind_from_name(name: &str) -> AnyResult<RawObjectKind> {
    match name {
        "commit" => Ok(RawObjectKind::Commit),
        "tree" => Ok(RawObjectKind::Tree),
        "blob" => Ok(RawObjectKind::Blob),
        "tag" => Ok(RawObjectKind::Tag),
        other => Err(format!("unsupported loose object kind {other:?}").into()),
    }
}

fn read_pack_object_from_bytes(pack: &[u8], offset: u64) -> AnyResult<PackedRawObject> {
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

fn validate_git_object_size(size: u64, context: &str) -> AnyResult<()> {
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

fn read_ofs_delta_base_offset_from_bytes(
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

fn apply_pack_delta(base: &[u8], delta: &[u8]) -> AnyResult<Vec<u8>> {
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

fn read_delta_varint(data: &[u8], pos: &mut usize) -> AnyResult<usize> {
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

fn read_be_u32(data: &[u8], offset: usize) -> AnyResult<u32> {
    let bytes: [u8; 4] = data
        .get(offset..offset + 4)
        .ok_or("truncated u32")?
        .try_into()?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_be_u64(data: &[u8], offset: usize) -> AnyResult<u64> {
    let bytes: [u8; 8] = data
        .get(offset..offset + 8)
        .ok_or("truncated u64")?
        .try_into()?;
    Ok(u64::from_be_bytes(bytes))
}

fn pack_direct_pair_result(
    pair: PackDirectPairStat,
    path: String,
    score: f64,
    reason: &str,
) -> ResultItem {
    let last_seen = pair
        .last_seen_time
        .map(|seconds| {
            format_gix_time(gix::date::Time {
                seconds,
                offset: pair.last_seen_offset,
            })
        })
        .unwrap_or_default();
    ResultItem {
        path,
        score,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen,
        reason: reason.to_string(),
        evidence: pair.evidence,
    }
}

fn truncate_top_pack_direct_pairs(results: &mut Vec<PackDirectScoredPair>, top: usize) {
    if top == 0 {
        results.clear();
        return;
    }
    if results.len() > top {
        results.select_nth_unstable_by(top, pack_direct_scored_pair_cmp);
        results.truncate(top);
    }
    results.sort_unstable_by(pack_direct_scored_pair_cmp);
}

fn effective_max_files(config: &OnDemandConfig) -> usize {
    if config.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        config.max_files_per_commit
    }
}

fn pack_direct_scored_pair_cmp(
    left: &PackDirectScoredPair,
    right: &PackDirectScoredPair,
) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then(left.path.cmp(&right.path))
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
}
