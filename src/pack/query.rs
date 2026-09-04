//! Pack-native history queries and direct co-change scoring.

use super::limits::PACK_DIRECT_PARALLEL_MIN_COMMITS;
use super::store::RawGitStore;
use super::tree::{pack_changed_files_for_commit, pack_changed_files_for_commit_into};
use super::types::{RawCommit, RawObjectId};
use super::walk::pack_visit_target_commits;
use crate::graph::time_decay;
use crate::history::{format_gix_time, parse_gix_since};
use crate::model::{Commit, Evidence, OnDemandConfig, ResultItem, direct_pair_capacity};
use crate::path_utils::normalize_git_path;
use crate::ranking::truncate_top_by;
use crate::{AnyResult, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_FILES};
use rayon::prelude::*;
use rustc_hash::FxHashMap as HashMap;
use std::cmp::Ordering;

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
    truncate_top_by(results, top, pack_direct_scored_pair_cmp);
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

#[derive(Clone, Debug, Default)]
struct PackDirectPairStat {
    cochanges: usize,
    weight: f64,
    other_weight: f64,
    last_seen_time: Option<i64>,
    last_seen_offset: i32,
    evidence: Vec<Evidence>,
}

struct PackDirectScoredPair {
    path: String,
    pair: PackDirectPairStat,
    score: f64,
}

struct PackDirectPartial {
    target_weight: f64,
    pairs: HashMap<String, PackDirectPairStat>,
}

impl PackDirectPartial {
    fn new(top: usize) -> Self {
        Self {
            target_weight: 0.0,
            pairs: HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default()),
        }
    }
}
