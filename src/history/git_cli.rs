//! Git subprocess history selection and expansion.

use super::parsers::{
    GitFollowHistory, canonicalize_followed_target_paths, parse_commit_seeds,
    parse_git_follow_history, parse_git_log, parse_git_log_direct, parse_git_log_rename_aware,
};
use crate::AnyResult;
use crate::git_utils::{run_git, run_git_with_stdin};
use crate::model::{Commit, GixCommitSeed, OnDemandConfig, RenameAwareCommit, ResultItem};
use crate::path_utils::{literal_pathspec, normalize_git_path};
use rayon::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::path::Path;

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

pub(crate) fn git_log_rename_aware(
    repo: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<RenameAwareCommit>> {
    let mut args = vec![
        "log".to_string(),
        "--find-renames".to_string(),
        "--name-status".to_string(),
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
    parse_git_log_rename_aware(&out)
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

pub(super) fn git_show_commit_for_target(
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
