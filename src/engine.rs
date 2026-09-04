//! Query execution, backend selection, and fallback policy.

use crate::AnyResult;
use crate::graph::RelatedGraph;
use crate::graph::{build_graph_data, query_direct_from_commits};
use crate::history::{
    git_diff_tree_direct_for_target, git_log_direct_for_target,
    git_log_direct_for_target_remove_empty, git_log_for_target, git_log_for_target_batch,
    git_log_for_target_batch_parallel, git_log_for_target_diff_tree,
    git_log_for_target_diff_tree_parallel, git_log_for_target_remove_empty,
    git_log_for_target_rev_list, gix_log_for_git_selected_target, gix_log_for_target,
};
use crate::model::{
    Commit, GraphBuildConfig, GraphData, HistoryCoverage, OnDemandBackend, OnDemandConfig,
    ResultItem,
};
use crate::pack::{
    git_log_for_target_pack_fast, git_log_for_target_pack_scan, git_pack_fast_direct_for_target,
    git_pack_scan_direct_for_target,
};
use crate::repo::RepoContext;

pub(crate) fn query_on_demand(
    root: &str,
    target: &str,
    mode: &str,
    top: usize,
    config: &OnDemandConfig,
) -> AnyResult<Vec<ResultItem>> {
    if mode == "direct"
        && let Some(results) = query_direct_on_demand_fast(root, target, config, top)?
    {
        return Ok(results);
    }
    let commits = on_demand_commits(root, target, config)?;
    query_from_commits(root, target, &commits, mode, top, config)
}

pub(crate) fn query_from_commits(
    root: &str,
    target: &str,
    commits: &[Commit],
    mode: &str,
    top: usize,
    config: &OnDemandConfig,
) -> AnyResult<Vec<ResultItem>> {
    if mode == "direct" {
        return Ok(query_direct_from_commits(
            target,
            commits,
            config,
            top,
            config.evidence_limit as isize,
        ));
    }
    let data = build_graph_data(
        root,
        commits,
        GraphBuildConfig {
            max_files_per_commit: config.max_files_per_commit,
            half_life_days: config.half_life_days,
            evidence_limit: config.evidence_limit,
        },
    );
    let graph = RelatedGraph::new(&data);
    graph.query(target, mode, top, config.evidence_limit as isize)
}

fn query_direct_on_demand_fast(
    root: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Option<Vec<ResultItem>>> {
    let results = match config.backend {
        OnDemandBackend::GitCli => git_log_direct_for_target(root, target, config, top)?,
        OnDemandBackend::GitDiffTree => {
            git_diff_tree_direct_for_target(root, target, config, top, false)?
        }
        OnDemandBackend::GitRevList => {
            git_diff_tree_direct_for_target(root, target, config, top, true)?
        }
        OnDemandBackend::GitRemoveEmpty => {
            git_log_direct_for_target_remove_empty(root, target, config, top)?
        }
        OnDemandBackend::PackScan => git_pack_scan_direct_for_target(root, target, config, top)?,
        OnDemandBackend::PackFast => git_pack_fast_direct_for_target(root, target, config, top)?,
        _ => return Ok(None),
    };
    Ok(Some(results))
}

pub(crate) fn build_on_demand_graph_data(
    root: &str,
    target: &str,
    config: &OnDemandConfig,
) -> AnyResult<GraphData> {
    let commits = on_demand_commits(root, target, config)?;
    Ok(build_graph_data(
        root,
        &commits,
        GraphBuildConfig {
            max_files_per_commit: config.max_files_per_commit,
            half_life_days: config.half_life_days,
            evidence_limit: config.evidence_limit,
        },
    ))
}

fn on_demand_commits(root: &str, target: &str, config: &OnDemandConfig) -> AnyResult<Vec<Commit>> {
    match config.backend {
        OnDemandBackend::Hybrid => gix_log_for_git_selected_target(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
            config.jobs,
        ),
        OnDemandBackend::Gix => gix_log_for_target(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
            config.scan_commits,
            config.jobs,
        ),
        OnDemandBackend::GitCli => {
            git_log_for_target(root, target, config.max_commits, config.since.as_deref())
        }
        OnDemandBackend::GitBatch => {
            git_log_for_target_batch(root, target, config.max_commits, config.since.as_deref())
        }
        OnDemandBackend::GitBatchParallel => git_log_for_target_batch_parallel(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
            config.jobs,
        ),
        OnDemandBackend::GitDiffTree => {
            git_log_for_target_diff_tree(root, target, config.max_commits, config.since.as_deref())
        }
        OnDemandBackend::GitDiffTreeParallel => git_log_for_target_diff_tree_parallel(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
            config.jobs,
        ),
        OnDemandBackend::GitRevList => {
            git_log_for_target_rev_list(root, target, config.max_commits, config.since.as_deref())
        }
        OnDemandBackend::GitRemoveEmpty => git_log_for_target_remove_empty(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
        ),
        OnDemandBackend::PackScan => git_log_for_target_pack_scan(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
            config.scan_commits,
        ),
        OnDemandBackend::PackFast => git_log_for_target_pack_fast(
            root,
            target,
            config.max_commits,
            config.since.as_deref(),
            config.scan_commits,
        ),
    }
}

pub(crate) fn configure_backend_for_repo(
    repo: &RepoContext,
    config: &mut OnDemandConfig,
) -> AnyResult<Option<String>> {
    let object_format = repo.object_format()?;
    if object_format == "sha1"
        || matches!(
            config.backend,
            OnDemandBackend::GitCli | OnDemandBackend::GitRemoveEmpty
        )
    {
        return Ok(None);
    }
    if config.backend_explicit {
        return Err(format!(
            "history backend {:?} does not support Git object format {object_format:?}; use --history-backend git",
            config.backend
        )
        .into());
    }
    config.backend = OnDemandBackend::GitCli;
    Ok(Some(format!(
        "Git object format {object_format} is not supported by pack-fast; used the git backend instead."
    )))
}

pub(crate) fn with_default_pack_fallback<T>(
    config: &mut OnDemandConfig,
    operation: impl Fn(&OnDemandConfig) -> AnyResult<T>,
) -> AnyResult<(T, Option<String>)> {
    match operation(config) {
        Ok(value) => Ok((value, None)),
        Err(pack_error)
            if config.backend == OnDemandBackend::PackFast && !config.backend_explicit =>
        {
            config.backend = OnDemandBackend::GitCli;
            match operation(config) {
                Ok(value) => Ok((
                    value,
                    Some(
                        "pack-fast could not read this repository; used the git backend instead."
                            .to_string(),
                    ),
                )),
                Err(git_error) => Err(format!(
                    "pack-fast failed: {pack_error}; git fallback failed: {git_error}"
                )
                .into()),
            }
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn history_coverage(
    config: &OnDemandConfig,
    diff_rename_mapping: bool,
) -> HistoryCoverage {
    let (completeness, approximate) = match config.backend {
        OnDemandBackend::GitCli | OnDemandBackend::GitRemoveEmpty => ("target-window-exact", false),
        OnDemandBackend::PackScan if config.scan_commits == 0 => ("target-window-exact", false),
        OnDemandBackend::PackFast => ("latency-bounded", true),
        OnDemandBackend::PackScan => ("scan-bounded", true),
        _ => ("backend-dependent", true),
    };
    HistoryCoverage {
        backend: format!("{:?}", config.backend),
        completeness: completeness.to_string(),
        approximate,
        rename_tracking: if matches!(
            config.backend,
            OnDemandBackend::GitCli | OnDemandBackend::GitRemoveEmpty
        ) {
            if diff_rename_mapping {
                "git-follow+diff-renames".to_string()
            } else {
                "git-follow".to_string()
            }
        } else if matches!(
            config.backend,
            OnDemandBackend::PackFast | OnDemandBackend::PackScan
        ) {
            if diff_rename_mapping {
                "exact-blob-renames+diff-renames".to_string()
            } else {
                "exact-blob-renames".to_string()
            }
        } else if diff_rename_mapping {
            "diff-renames-only".to_string()
        } else {
            "current-path-only".to_string()
        },
        max_target_commits: config.max_commits,
        scan_commits: config.scan_commits,
    }
}
