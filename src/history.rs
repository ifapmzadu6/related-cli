use crate::AnyResult;
use crate::git_utils::{run_git, run_git_with_stdin};
use crate::graph::{direct_pair_result, time_decay, truncate_top_direct_pairs};
use crate::model::*;
use crate::path_utils::{decode_git_path, literal_pathspec, normalize_git_path};
use gix::bstr::ByteSlice;
use rayon::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(crate) fn git_log(
    repo: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let mut args = vec![
        "log".to_string(),
        "--name-only".to_string(),
        "-z".to_string(),
        "--diff-filter=ACMRT".to_string(),
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s".to_string(),
    ];
    if max_commits > 0 {
        args.push(format!("--max-count={max_commits}"));
    }
    if let Some(since) = since {
        args.push("--since".to_string());
        args.push(since.to_string());
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_git(Path::new(repo), &arg_refs)?;
    parse_git_log(&out)
}

pub(crate) fn git_log_for_target(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    git_log_for_target_git(repo, target, max_commits, since, false)
}

pub(crate) fn git_log_for_target_remove_empty(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    git_log_for_target_git(repo, target, max_commits, since, true)
}

fn git_log_for_target_git(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    remove_empty: bool,
) -> AnyResult<Vec<Commit>> {
    let history = git_follow_target_history(repo, target, max_commits, since, remove_empty)?;
    git_show_followed_commits(repo, target, &history)
}

struct GitFollowHistory {
    hash_input: Vec<u8>,
    hashes: Vec<String>,
    target_paths_by_hash: HashMap<String, HashSet<String>>,
}

pub(crate) fn git_followed_commits_for_targets(
    repo: &str,
    targets: &[String],
    config: &OnDemandConfig,
) -> AnyResult<Vec<(String, Vec<Commit>)>> {
    let jobs = config.jobs.min(targets.len()).max(1);
    let mut ordered_histories = if jobs > 1 && targets.len() > 1 {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
        pool.install(|| -> Result<Vec<_>, String> {
            targets
                .par_iter()
                .enumerate()
                .map(|(index, target)| {
                    git_follow_target_history(
                        repo,
                        target,
                        config.max_commits,
                        config.since.as_deref(),
                        false,
                    )
                    .map(|history| (index, target.clone(), history))
                    .map_err(|err| err.to_string())
                })
                .collect::<Result<Vec<_>, _>>()
        })?
    } else {
        targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                git_follow_target_history(
                    repo,
                    target,
                    config.max_commits,
                    config.since.as_deref(),
                    false,
                )
                .map(|history| (index, target.clone(), history))
            })
            .collect::<AnyResult<Vec<_>>>()?
    };
    ordered_histories.sort_by_key(|(index, _, _)| *index);

    let mut histories = Vec::with_capacity(ordered_histories.len());
    let mut unique_hashes = HashSet::default();
    let mut hashes = Vec::new();
    for (_, target, history) in ordered_histories {
        for hash in &history.hashes {
            if unique_hashes.insert(hash.clone()) {
                hashes.push(hash.clone());
            }
        }
        histories.push((target, history));
    }

    let commits_by_hash = git_show_hashes(repo, &hashes, config.evidence_limit > 0)?;
    let mut results = Vec::with_capacity(histories.len());
    for (target, history) in histories {
        let mut commits = Vec::with_capacity(history.hashes.len());
        for hash in &history.hashes {
            let mut commit = commits_by_hash
                .get(hash)
                .cloned()
                .ok_or_else(|| format!("missing expanded followed commit {hash}"))?;
            canonicalize_followed_target_paths(
                std::slice::from_mut(&mut commit),
                &target,
                &history.target_paths_by_hash,
            );
            commits.push(commit);
        }
        results.push((target, commits));
    }
    Ok(results)
}

fn git_follow_target_history(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    remove_empty: bool,
) -> AnyResult<GitFollowHistory> {
    let mut args = vec![
        "log".to_string(),
        "--follow".to_string(),
        "--find-renames".to_string(),
        "--name-status".to_string(),
        "-z".to_string(),
        "--diff-filter=ACMRT".to_string(),
        "--pretty=format:%x1e%H".to_string(),
    ];
    if remove_empty {
        args.push("--remove-empty".to_string());
    }
    if max_commits > 0 {
        args.push(format!("--max-count={max_commits}"));
    }
    if let Some(since) = since {
        args.push("--since".to_string());
        args.push(since.to_string());
    }
    args.push("--".to_string());
    args.push(literal_pathspec(target));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_git(Path::new(repo), &arg_refs)?;
    parse_git_follow_history(&out)
}

fn git_show_followed_commits(
    repo: &str,
    target: &str,
    history: &GitFollowHistory,
) -> AnyResult<Vec<Commit>> {
    if history.hash_input.is_empty() {
        return Ok(Vec::new());
    }
    let commits_by_hash = git_show_hashes(repo, &history.hashes, true)?;
    let mut commits = Vec::with_capacity(history.hashes.len());
    for hash in &history.hashes {
        commits.push(
            commits_by_hash
                .get(hash)
                .cloned()
                .ok_or_else(|| format!("missing expanded followed commit {hash}"))?,
        );
    }
    canonicalize_followed_target_paths(&mut commits, target, &history.target_paths_by_hash);
    Ok(commits)
}

fn git_show_hashes(
    repo: &str,
    hashes: &[String],
    include_subject: bool,
) -> AnyResult<HashMap<String, Commit>> {
    const HASH_CHUNK_SIZE: usize = 512;
    let pretty = if include_subject {
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s"
    } else {
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI"
    };
    let args = [
        "show",
        "--stdin",
        "--no-renames",
        "--full-diff",
        "--name-only",
        "-z",
        "--diff-filter=ACMRT",
        pretty,
    ];
    let mut commits_by_hash = HashMap::default();
    for chunk in hashes.chunks(HASH_CHUNK_SIZE) {
        let mut input = Vec::with_capacity(chunk.len().saturating_mul(41));
        for hash in chunk {
            input.extend_from_slice(hash.as_bytes());
            input.push(b'\n');
        }
        let out = run_git_with_stdin(Path::new(repo), &args, &input)?;
        for commit in parse_git_log(&out)? {
            commits_by_hash.insert(commit.hash.clone(), commit);
        }
    }
    Ok(commits_by_hash)
}

pub(crate) fn git_log_for_target_batch(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_commits, since)?;
    git_show_selected_commits(repo, &seeds)
}

pub(crate) fn git_log_for_target_batch_parallel(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    jobs: usize,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_commits, since)?;
    if jobs <= 1 || seeds.len() <= 1 {
        return git_show_selected_commits(repo, &seeds);
    }

    let chunk_size = seeds.len().div_ceil(jobs).max(1);
    let chunks: Vec<(usize, Vec<GixCommitSeed>)> = seeds
        .chunks(chunk_size)
        .enumerate()
        .map(|(chunk_order, chunk)| (chunk_order, chunk.to_vec()))
        .collect();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
    let mut results: Vec<(usize, Vec<Commit>)> = pool.install(|| -> Result<_, String> {
        chunks
            .par_iter()
            .map(|(chunk_order, chunk)| {
                git_show_selected_commits(repo, chunk)
                    .map(|commits| (*chunk_order, commits))
                    .map_err(|err| err.to_string())
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    results.sort_by_key(|(chunk_order, _)| *chunk_order);

    let mut commits = Vec::new();
    for (_, mut chunk) in results {
        commits.append(&mut chunk);
    }
    Ok(commits)
}

pub(crate) fn git_log_for_target_diff_tree(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_commits, since)?;
    git_diff_tree_selected_commits(repo, &seeds)
}

pub(crate) fn git_log_for_target_diff_tree_parallel(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    jobs: usize,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_commits, since)?;
    if jobs <= 1 || seeds.len() <= 1 {
        return git_diff_tree_selected_commits(repo, &seeds);
    }

    let chunk_size = seeds.len().div_ceil(jobs).max(1);
    let chunks: Vec<(usize, Vec<GixCommitSeed>)> = seeds
        .chunks(chunk_size)
        .enumerate()
        .map(|(chunk_order, chunk)| (chunk_order, chunk.to_vec()))
        .collect();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
    let mut results: Vec<(usize, Vec<Commit>)> = pool.install(|| -> Result<_, String> {
        chunks
            .par_iter()
            .map(|(chunk_order, chunk)| {
                git_diff_tree_selected_commits(repo, chunk)
                    .map(|commits| (*chunk_order, commits))
                    .map_err(|err| err.to_string())
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    results.sort_by_key(|(chunk_order, _)| *chunk_order);

    let mut commits = Vec::new();
    for (_, mut chunk) in results {
        commits.append(&mut chunk);
    }
    Ok(commits)
}

pub(crate) fn git_log_for_target_rev_list(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_rev_list_target_commit_seeds(repo, target, max_commits, since)?;
    git_diff_tree_selected_commits(repo, &seeds)
}

pub(crate) fn git_log_direct_for_target(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    git_log_direct_for_target_git(repo, target, config, top, false)
}

pub(crate) fn git_log_direct_for_target_remove_empty(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    git_log_direct_for_target_git(repo, target, config, top, true)
}

fn git_log_direct_for_target_git(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
    remove_empty: bool,
) -> AnyResult<Vec<ResultItem>> {
    let commits = git_log_for_target_git(
        repo,
        target,
        config.max_commits,
        config.since.as_deref(),
        remove_empty,
    )?;
    Ok(crate::graph::query_direct_from_commits(
        target,
        &commits,
        config,
        top,
        config.evidence_limit as isize,
    ))
}

pub(crate) fn git_diff_tree_direct_for_target(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
    use_rev_list: bool,
) -> AnyResult<Vec<ResultItem>> {
    let input = if use_rev_list {
        git_rev_list_target_commit_hash_input(
            repo,
            target,
            config.max_commits,
            config.since.as_deref(),
        )?
    } else {
        git_target_commit_hash_input(repo, target, config.max_commits, config.since.as_deref())?
    };
    git_diff_tree_direct_from_hash_input(repo, &input, target, config, top)
}

fn git_show_selected_commits(repo: &str, seeds: &[GixCommitSeed]) -> AnyResult<Vec<Commit>> {
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let mut input = String::new();
    for seed in seeds {
        input.push_str(&seed.id.to_string());
        input.push('\n');
    }
    let args = [
        "show",
        "--stdin",
        "--full-diff",
        "--name-only",
        "-z",
        "--diff-filter=ACMRT",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
    ];
    let out = run_git_with_stdin(Path::new(repo), &args, input.as_bytes())?;
    parse_git_log(&out)
}

pub(crate) fn git_diff_tree_selected_commits(
    repo: &str,
    seeds: &[GixCommitSeed],
) -> AnyResult<Vec<Commit>> {
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let mut input = String::new();
    for seed in seeds {
        input.push_str(&seed.id.to_string());
        input.push('\n');
    }
    let args = [
        "diff-tree",
        "--stdin",
        "--root",
        "-r",
        "--name-only",
        "-z",
        "--diff-filter=ACMRT",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
    ];
    let out = run_git_with_stdin(Path::new(repo), &args, input.as_bytes())?;
    parse_git_log(&out)
}

pub(crate) fn git_diff_tree_direct_from_hash_input(
    repo: &str,
    input: &[u8],
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let pretty = if config.evidence_limit == 0 {
        "--pretty=format:%x1e%ct%x1f%cI"
    } else {
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s"
    };
    let args = [
        "diff-tree",
        "--stdin",
        "--root",
        "-r",
        "--name-only",
        "-z",
        "--diff-filter=ACMRT",
        pretty,
    ];
    let out = run_git_with_stdin(Path::new(repo), &args, input)?;
    parse_git_log_direct(&out, target, config, top, config.evidence_limit as isize)
}

pub(crate) fn gix_log_for_target(
    repo: &str,
    target: &str,
    max_target_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
    jobs: usize,
) -> AnyResult<Vec<Commit>> {
    let thread_safe = gix::ThreadSafeRepository::open(repo)?;
    let mut local = thread_safe.to_thread_local();
    local.object_cache_size_if_unset(16 * 1024 * 1024);
    let since_seconds = parse_gix_since(since)?;
    let walk = local
        .head_id()?
        .ancestors()
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()?;

    let batch_size = (jobs.max(1) * 64).max(256);
    let target_path = PathBuf::from(target);
    let mut commits = Vec::new();
    let mut batch = Vec::with_capacity(batch_size);
    let mut scanned = 0usize;

    for info in walk {
        let info = info?;
        scanned += 1;
        if let Some(since_seconds) = since_seconds
            && info.commit_time() < since_seconds
        {
            break;
        }
        batch.push(GixCommitSeed {
            id: info.id,
            first_parent: info.parent_ids().next().map(|id| id.detach()),
        });

        let scan_limit_reached = scan_commits > 0 && scanned >= scan_commits;
        if batch.len() >= batch_size || scan_limit_reached {
            append_gix_target_batch(&thread_safe, &target_path, &batch, jobs, &mut commits)?;
            batch.clear();
            if (max_target_commits > 0 && commits.len() >= max_target_commits) || scan_limit_reached
            {
                break;
            }
        }
    }

    if !batch.is_empty() && (max_target_commits == 0 || commits.len() < max_target_commits) {
        append_gix_target_batch(&thread_safe, &target_path, &batch, jobs, &mut commits)?;
    }
    if max_target_commits > 0 {
        commits.truncate(max_target_commits);
    }
    Ok(commits)
}

pub(crate) fn gix_log_for_git_selected_target(
    repo: &str,
    target: &str,
    max_target_commits: usize,
    since: Option<&str>,
    jobs: usize,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_target_commits, since)?;
    let thread_safe = gix::ThreadSafeRepository::open(repo)?;
    let target_path = PathBuf::from(target);
    let mut commits = Vec::new();
    append_gix_selected_batch(repo, &thread_safe, &target_path, &seeds, jobs, &mut commits)?;
    Ok(commits)
}

pub(crate) fn git_target_commit_seeds(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<GixCommitSeed>> {
    let out = git_target_commit_hash_input(repo, target, max_commits, since)?;
    parse_commit_seeds(&out)
}

pub(crate) fn git_target_commit_hash_input(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<u8>> {
    let mut args = vec![
        "log".to_string(),
        "--diff-filter=ACMRT".to_string(),
        "--format=%H".to_string(),
    ];
    if max_commits > 0 {
        args.push(format!("--max-count={max_commits}"));
    }
    if let Some(since) = since {
        args.push("--since".to_string());
        args.push(since.to_string());
    }
    args.push("--".to_string());
    args.push(literal_pathspec(target));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git_hash_stdin_input(run_git(Path::new(repo), &arg_refs)?)
}

fn git_rev_list_target_commit_seeds(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<GixCommitSeed>> {
    let out = git_rev_list_target_commit_hash_input(repo, target, max_commits, since)?;
    parse_commit_seeds(&out)
}

fn git_rev_list_target_commit_hash_input(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<u8>> {
    let mut args = vec!["rev-list".to_string()];
    if max_commits > 0 {
        args.push(format!("--max-count={max_commits}"));
    }
    if let Some(since) = since {
        args.push("--since".to_string());
        args.push(since.to_string());
    }
    args.push("HEAD".to_string());
    args.push("--".to_string());
    args.push(literal_pathspec(target));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git_hash_stdin_input(run_git(Path::new(repo), &arg_refs)?)
}

fn git_hash_stdin_input(mut out: Vec<u8>) -> AnyResult<Vec<u8>> {
    std::str::from_utf8(&out)?;
    if out.last().is_some_and(|byte| !byte.is_ascii_whitespace()) {
        out.push(b'\n');
    }
    Ok(out)
}

fn parse_commit_seeds(input: &[u8]) -> AnyResult<Vec<GixCommitSeed>> {
    std::str::from_utf8(input)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            Ok(GixCommitSeed {
                id: gix::hash::ObjectId::from_hex(line.trim().as_bytes())?,
                first_parent: None,
            })
        })
        .collect()
}

pub(crate) fn parse_gix_since(since: Option<&str>) -> AnyResult<Option<i64>> {
    since
        .map(|value| {
            gix::date::parse(value, Some(SystemTime::now()))
                .map(|time| time.seconds)
                .map_err(|err| format!("invalid --since value {value:?}: {err}").into())
        })
        .transpose()
}

fn append_gix_target_batch(
    thread_safe: &gix::ThreadSafeRepository,
    target: &Path,
    batch: &[GixCommitSeed],
    jobs: usize,
    out: &mut Vec<Commit>,
) -> AnyResult<()> {
    append_gix_batch(None, thread_safe, target, batch, jobs, false, out)
}

fn append_gix_selected_batch(
    repo_root: &str,
    thread_safe: &gix::ThreadSafeRepository,
    target: &Path,
    batch: &[GixCommitSeed],
    jobs: usize,
    out: &mut Vec<Commit>,
) -> AnyResult<()> {
    append_gix_batch(Some(repo_root), thread_safe, target, batch, jobs, true, out)
}

fn append_gix_batch(
    fallback_repo: Option<&str>,
    thread_safe: &gix::ThreadSafeRepository,
    target: &Path,
    batch: &[GixCommitSeed],
    jobs: usize,
    trust_target_changed: bool,
    out: &mut Vec<Commit>,
) -> AnyResult<()> {
    if jobs <= 1 {
        let mut repo = thread_safe.to_thread_local();
        repo.object_cache_size_if_unset(16 * 1024 * 1024);
        for seed in batch {
            match gix_commit_for_target(&mut repo, seed, target, trust_target_changed) {
                Ok(Some(commit)) => out.push(commit),
                Ok(None) => {}
                Err(err) => {
                    if let Some(repo) = fallback_repo {
                        if let Some(commit) = git_show_commit_for_target(repo, target, seed.id)? {
                            out.push(commit);
                        }
                    } else {
                        return Err(err);
                    }
                }
            }
        }
        return Ok(());
    }

    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
    let results: Vec<(gix::hash::ObjectId, Result<Option<Commit>, String>)> = pool.install(|| {
        batch
            .par_iter()
            .map(|seed| {
                let mut repo = thread_safe.to_thread_local();
                repo.object_cache_size_if_unset(16 * 1024 * 1024);
                (
                    seed.id,
                    gix_commit_for_target(&mut repo, seed, target, trust_target_changed)
                        .map_err(|err| err.to_string()),
                )
            })
            .collect()
    });
    for (id, result) in results {
        match result {
            Ok(Some(commit)) => out.push(commit),
            Ok(None) => {}
            Err(err) => {
                if let Some(repo) = fallback_repo {
                    if let Some(commit) = git_show_commit_for_target(repo, target, id)? {
                        out.push(commit);
                    }
                } else {
                    return Err(format!("gix on-demand failed: {err}").into());
                }
            }
        }
    }
    Ok(())
}

fn git_show_commit_for_target(
    repo: &str,
    target: &Path,
    commit_id: gix::hash::ObjectId,
) -> AnyResult<Option<Commit>> {
    let commit_id = commit_id.to_string();
    let target = normalize_git_path(&target.display().to_string());
    let args = [
        "show",
        "--full-diff",
        "--name-only",
        "-z",
        "--diff-filter=ACMRT",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
        commit_id.as_str(),
        "--",
        target.as_str(),
    ];
    let out = run_git(Path::new(repo), &args)?;
    Ok(parse_git_log(&out)?.into_iter().next())
}

fn gix_commit_for_target(
    repo: &mut gix::Repository,
    seed: &GixCommitSeed,
    target: &Path,
    trust_target_changed: bool,
) -> AnyResult<Option<Commit>> {
    let commit = repo.find_commit(seed.id)?;
    let first_parent = match seed.first_parent {
        Some(parent) => Some(parent),
        None => commit.parent_ids().next().map(|id| id.detach()),
    };
    let time = commit.time()?;
    let new_tree = commit.tree()?;
    let old_tree = match first_parent {
        Some(parent) => Some(repo.find_commit(parent)?.tree()?),
        None => None,
    };

    if !trust_target_changed && !gix_target_changed(old_tree.as_ref(), &new_tree, target)? {
        return Ok(None);
    }

    let mut options = gix::diff::Options::default();
    options.track_path().track_rewrites(None);
    let changes = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(options))?;
    let mut seen = HashSet::default();
    let mut files = Vec::new();
    for change in changes {
        if let Some(path) = gix_current_change_path(&change) {
            let path = normalize_git_path(&path);
            if !path.is_empty() && seen.insert(path.clone()) {
                files.push(path);
            }
        }
    }
    let target = normalize_git_path(&target.display().to_string());
    if trust_target_changed && !files.contains(&target) {
        files.push(target.clone());
    }
    if !files.contains(&target) {
        return Ok(None);
    }

    let subject = commit
        .message_raw_sloppy()
        .to_str_lossy()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    Ok(Some(Commit {
        hash: seed.id.to_string(),
        unix_time: time.seconds,
        date: format_gix_time(time),
        subject,
        files,
    }))
}

fn gix_target_changed(
    old_tree: Option<&gix::Tree<'_>>,
    new_tree: &gix::Tree<'_>,
    target: &Path,
) -> AnyResult<bool> {
    let new_entry = new_tree
        .lookup_entry_by_path(target)?
        .map(|entry| (entry.mode(), entry.object_id()));
    let Some(new_entry) = new_entry else {
        return Ok(false);
    };
    let old_entry = old_tree
        .map(|tree| {
            tree.lookup_entry_by_path(target)
                .map(|entry| entry.map(|entry| (entry.mode(), entry.object_id())))
        })
        .transpose()?
        .flatten();
    Ok(old_entry != Some(new_entry))
}

fn gix_current_change_path(change: &gix::object::tree::diff::ChangeDetached) -> Option<String> {
    use gix::diff::tree_with_rewrites::Change;
    match change {
        Change::Addition {
            location,
            entry_mode,
            ..
        }
        | Change::Modification {
            location,
            entry_mode,
            ..
        }
        | Change::Rewrite {
            location,
            entry_mode,
            ..
        } => {
            if entry_mode.is_tree() {
                None
            } else {
                Some(location.to_str_lossy().into_owned())
            }
        }
        Change::Deletion { .. } => None,
    }
}

pub(crate) fn format_gix_time(time: gix::date::Time) -> String {
    time.format_or_unix(gix::date::time::format::ISO8601_STRICT)
}

fn normalize_git_iso8601_date(date: &str) -> String {
    date.strip_suffix('Z')
        .map_or_else(|| date.to_string(), |prefix| format!("{prefix}+00:00"))
}

struct GitLogRecord<'a> {
    header: &'a str,
    files: Vec<String>,
}

fn parse_git_log_record(raw_record: &[u8]) -> AnyResult<Option<GitLogRecord<'_>>> {
    let raw_record = raw_record
        .iter()
        .position(|byte| *byte != b'\n' && *byte != 0)
        .map_or(&[][..], |start| &raw_record[start..]);
    if raw_record.is_empty() {
        return Ok(None);
    }

    let header_end = raw_record
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or_else(|| {
            raw_record
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(raw_record.len())
        });
    let header = std::str::from_utf8(&raw_record[..header_end])?;
    if header.is_empty() {
        return Ok(None);
    }

    let file_bytes = match raw_record.get(header_end) {
        Some(b'\n' | 0) => &raw_record[header_end + 1..],
        _ => &[],
    };
    let separator = if file_bytes.contains(&0) { 0 } else { b'\n' };
    let mut files = Vec::new();
    for raw_path in file_bytes
        .split(|byte| *byte == separator)
        .filter(|path| !path.is_empty())
    {
        let path = decode_git_path(raw_path)?;
        if !path.is_empty() {
            files.push(path);
        }
    }
    Ok(Some(GitLogRecord { header, files }))
}

fn parse_git_follow_history(out: &[u8]) -> AnyResult<GitFollowHistory> {
    let mut hash_input = Vec::new();
    let mut hashes = Vec::new();
    let mut target_paths_by_hash = HashMap::default();
    for raw_record in out.split(|byte| *byte == 0x1e) {
        let raw_record = raw_record
            .iter()
            .position(|byte| !matches!(*byte, 0 | b'\n' | b'\r'))
            .map_or(&[][..], |start| &raw_record[start..]);
        if raw_record.is_empty() {
            continue;
        }
        let header_end = raw_record
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(raw_record.len());
        let hash = std::str::from_utf8(&raw_record[..header_end])?.trim();
        if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid followed commit hash {hash:?}").into());
        }
        hash_input.extend_from_slice(hash.as_bytes());
        hash_input.push(b'\n');
        hashes.push(hash.to_string());

        let file_bytes = raw_record.get(header_end + 1..).unwrap_or_default();
        let tokens: Vec<&[u8]> = file_bytes
            .split(|byte| *byte == 0)
            .filter(|token| !token.is_empty())
            .collect();
        let mut idx = 0usize;
        let mut target_paths = HashSet::default();
        while idx < tokens.len() {
            let status = tokens[idx];
            idx += 1;
            let path_count = if matches!(status.first(), Some(b'R' | b'C')) {
                2
            } else if matches!(status.first(), Some(b'A' | b'M' | b'T')) {
                1
            } else {
                return Err(format!(
                    "unsupported followed name-status token {:?}",
                    String::from_utf8_lossy(status)
                )
                .into());
            };
            if idx.saturating_add(path_count) > tokens.len() {
                return Err("truncated followed name-status record".into());
            }
            for raw_path in &tokens[idx..idx + path_count] {
                let path = decode_git_path(raw_path)?;
                if !path.is_empty() {
                    target_paths.insert(path);
                }
            }
            idx += path_count;
        }
        target_paths_by_hash.insert(hash.to_string(), target_paths);
    }
    Ok(GitFollowHistory {
        hash_input,
        hashes,
        target_paths_by_hash,
    })
}

fn canonicalize_followed_target_paths(
    commits: &mut [Commit],
    target: &str,
    target_paths_by_hash: &HashMap<String, HashSet<String>>,
) {
    for commit in commits {
        let Some(target_paths) = target_paths_by_hash.get(&commit.hash) else {
            continue;
        };
        let mut seen = HashSet::default();
        let mut canonical = Vec::with_capacity(commit.files.len());
        for file in commit.files.drain(..) {
            let file = if target_paths.contains(&file) {
                target.to_string()
            } else {
                file
            };
            if seen.insert(file.clone()) {
                canonical.push(file);
            }
        }
        commit.files = canonical;
    }
}

fn parse_git_log(out: &[u8]) -> AnyResult<Vec<Commit>> {
    let mut commits = Vec::new();
    for raw_record in out.split(|byte| *byte == 0x1e) {
        let Some(record) = parse_git_log_record(raw_record)? else {
            continue;
        };
        let mut fields = record.header.splitn(4, '\x1f');
        let hash = fields.next().ok_or("missing commit hash")?.to_string();
        let unix_time: i64 = fields
            .next()
            .ok_or("missing commit unix time")?
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let date = normalize_git_iso8601_date(fields.next().ok_or("missing commit date")?);
        let subject = fields.next().unwrap_or_default().to_string();
        let mut seen = HashSet::default();
        let mut files = Vec::new();
        for file in record.files {
            if file.is_empty() || !seen.insert(file.clone()) {
                continue;
            }
            files.push(file);
        }
        commits.push(Commit {
            hash,
            unix_time,
            date,
            subject,
            files,
        });
    }
    Ok(commits)
}

fn parse_git_log_direct(
    out: &[u8],
    target: &str,
    config: &OnDemandConfig,
    top: usize,
    evidence_limit: isize,
) -> AnyResult<Vec<ResultItem>> {
    let max_files = config.max_files_per_commit;
    let half_life = config.half_life_days;
    let mut latest = None;
    let mut target_weight = 0.0;
    let mut pairs: HashMap<String, DirectPairStat> =
        HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default());
    for raw_record in out.split(|byte| *byte == 0x1e) {
        let Some(record) = parse_git_log_record(raw_record)? else {
            continue;
        };
        let (hash, unix_time_raw, date, subject) = if config.evidence_limit == 0 {
            let (unix_time_raw, date) = record
                .header
                .split_once('\x1f')
                .ok_or("missing compact commit header field")?;
            ("", unix_time_raw, date, "")
        } else {
            let mut fields = record.header.splitn(4, '\x1f');
            let hash = fields.next().ok_or("missing commit hash")?;
            let unix_time_raw = fields.next().ok_or("missing commit unix time")?;
            let date = fields.next().ok_or("missing commit date")?;
            let subject = fields.next().unwrap_or_default();
            (hash, unix_time_raw, date, subject)
        };
        let unix_time: i64 = unix_time_raw
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let date = normalize_git_iso8601_date(date);
        let file_count = record.files.len();
        let has_target = record.files.iter().any(|file| file == target);
        if file_count == 0 || file_count > max_files || !has_target {
            continue;
        }

        let latest = *latest.get_or_insert(unix_time);
        let decay = time_decay(latest, unix_time, half_life);
        target_weight += decay;

        let pair_weight = decay / ((file_count + 1) as f64).log2();
        let mut evidence = None;
        for other in record.files.iter().filter(|file| file.as_str() != target) {
            let pair = pairs.entry(other.clone()).or_default();
            pair.cochanges += 1;
            pair.weight += pair_weight;
            pair.other_weight += decay;
            if pair.last_seen.is_empty() || date.as_str() > pair.last_seen.as_str() {
                pair.last_seen = date.clone();
            }
            if pair.evidence.len() < config.evidence_limit {
                let evidence = evidence.get_or_insert_with(|| Evidence {
                    hash: hash.to_string(),
                    date: date.clone(),
                    subject: subject.to_string(),
                    file_count,
                    weight: pair_weight,
                });
                pair.evidence.push(evidence.clone());
            }
        }
    }

    let mut scored = Vec::with_capacity(pairs.len());
    for (path, pair) in pairs {
        let score = if target_weight <= 0.0 || pair.other_weight <= 0.0 {
            pair.weight
        } else {
            pair.weight / (target_weight * pair.other_weight).sqrt()
        };
        scored.push(DirectScoredPair { path, pair, score });
    }
    truncate_top_direct_pairs(&mut scored, top);
    Ok(scored
        .into_iter()
        .map(|item| {
            direct_pair_result(
                item.pair,
                item.path,
                item.score,
                "direct_cochange",
                evidence_limit,
            )
        })
        .collect())
}

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_parse_bytes(data: &[u8]) {
    let _ = parse_git_log(data);
    let _ = parse_git_log_record(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_git_log_parser_preserves_newlines_in_paths() {
        let raw = b"hash\x1f1\x1f2026-01-01T00:00:00Z\x1fsubject\n\0line\nbreak.md\0other.md\0";
        let record = parse_git_log_record(raw).unwrap().unwrap();
        assert_eq!(record.files, vec!["line\nbreak.md", "other.md"]);
    }

    #[test]
    fn git_utc_dates_use_the_pack_backend_offset_format() {
        assert_eq!(
            normalize_git_iso8601_date("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00+00:00"
        );
        assert_eq!(
            normalize_git_iso8601_date("2026-01-01T09:00:00+09:00"),
            "2026-01-01T09:00:00+09:00"
        );
    }
}
