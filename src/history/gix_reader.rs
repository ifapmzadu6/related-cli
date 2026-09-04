//! Gitoxide history traversal and tree diffs.

use super::git_cli::{git_show_commit_for_target, git_target_commit_seeds};
use crate::AnyResult;
use crate::model::{Commit, GixCommitSeed};
use crate::path_utils::normalize_git_path;
use gix::bstr::ByteSlice;
use rayon::prelude::*;
use rustc_hash::FxHashSet as HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

pub(crate) fn parse_gix_since(since: Option<&str>) -> AnyResult<Option<i64>> {
    since
        .map(|value| {
            gix::date::parse(value, Some(SystemTime::now()))
                .map(|time| time.seconds)
                .map_err(|err| format!("invalid --since value {value:?}: {err}").into())
        })
        .transpose()
}
