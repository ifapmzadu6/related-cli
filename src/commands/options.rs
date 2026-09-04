use crate::cli::{
    ParsedArgs, flag_optional_string, flag_positive_f64, flag_positive_usize, flag_string,
    flag_usize,
};
use crate::model::{Confidence, OnDemandBackend, OnDemandConfig};
use crate::{
    AnyResult, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_COMMITS, DEFAULT_MAX_FILES,
    DEFAULT_ON_DEMAND_BACKEND,
};

pub(super) fn parse_on_demand_config(
    parsed: &ParsedArgs,
    default_evidence: usize,
) -> AnyResult<OnDemandConfig> {
    let history_backend_explicit = parsed.flags.contains_key("history-backend");
    let accuracy_explicit = parsed.flags.contains_key("accuracy");
    if history_backend_explicit && accuracy_explicit {
        return Err("--accuracy and --history-backend cannot be used together".into());
    }
    let backend = if let Some(accuracy) = flag_optional_string(parsed, "accuracy") {
        match accuracy.as_str() {
            "fast" => OnDemandBackend::PackFast,
            "exact" => OnDemandBackend::GitCli,
            other => return Err(format!("unknown accuracy {other:?}; use fast or exact").into()),
        }
    } else {
        parse_on_demand_backend(&flag_string(
            parsed,
            "history-backend",
            DEFAULT_ON_DEMAND_BACKEND,
        ))?
    };
    Ok(OnDemandConfig {
        backend,
        // Public accuracy levels describe behavior, not a specific implementation.
        // Keep the normal pack-fast -> Git fallback for `--accuracy fast`.
        backend_explicit: history_backend_explicit,
        max_commits: flag_usize(parsed, "max-commits", DEFAULT_MAX_COMMITS)?,
        since: flag_optional_string(parsed, "since"),
        max_files_per_commit: flag_positive_usize(
            parsed,
            "max-files-per-commit",
            DEFAULT_MAX_FILES,
        )?,
        half_life_days: flag_positive_f64(parsed, "half-life-days", DEFAULT_HALF_LIFE_DAYS)?,
        evidence_limit: flag_usize(parsed, "evidence", default_evidence)?,
        jobs: flag_positive_usize(parsed, "jobs", default_jobs())?,
        jobs_explicit: parsed.flags.contains_key("jobs"),
        scan_commits: flag_usize(parsed, "scan-commits", 0)?,
    })
}

fn parse_on_demand_backend(value: &str) -> AnyResult<OnDemandBackend> {
    match value {
        "hybrid" | "git-gix" | "gix-git" => Ok(OnDemandBackend::Hybrid),
        "gix" | "gitoxide" => Ok(OnDemandBackend::Gix),
        "git" | "git-cli" | "cli" => Ok(OnDemandBackend::GitCli),
        "git-batch" | "batch" => Ok(OnDemandBackend::GitBatch),
        "git-batch-parallel" | "batch-parallel" => Ok(OnDemandBackend::GitBatchParallel),
        "git-diff-tree" | "diff-tree" => Ok(OnDemandBackend::GitDiffTree),
        "git-diff-tree-parallel" | "diff-tree-parallel" => {
            Ok(OnDemandBackend::GitDiffTreeParallel)
        }
        "git-rev-list" | "rev-list" => Ok(OnDemandBackend::GitRevList),
        "git-remove-empty" | "remove-empty" => Ok(OnDemandBackend::GitRemoveEmpty),
        "pack-scan" | "pack-full" | "pack-walk" => Ok(OnDemandBackend::PackScan),
        "pack" | "git-pack" | "pack-fast" | "pack-auto" => Ok(OnDemandBackend::PackFast),
        other => Err(format!(
            "unknown history backend {other:?}; use hybrid, gix, git, git-remove-empty, git-batch, git-batch-parallel, git-diff-tree, git-diff-tree-parallel, git-rev-list, pack-fast, or pack-scan"
        )
        .into()),
    }
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

pub(super) fn validate_query_mode(mode: &str) -> AnyResult<()> {
    match mode {
        "direct" | "pagerank" | "path" | "hot" => Ok(()),
        other => Err(format!("unknown mode {other:?}; use direct, pagerank, path, or hot").into()),
    }
}

pub(super) fn parse_confidence(value: &str) -> AnyResult<Confidence> {
    match value {
        "low" => Ok(Confidence::Low),
        "medium" => Ok(Confidence::Medium),
        "high" => Ok(Confidence::High),
        other => Err(format!("unknown confidence {other:?}; use low, medium, or high").into()),
    }
}

const HISTORY_VALUE_FLAGS: &[&str] = &[
    "repo",
    "format",
    "evidence",
    "accuracy",
    "history-backend",
    "max-commits",
    "since",
    "max-files-per-commit",
    "half-life-days",
    "jobs",
    "scan-commits",
];

pub(super) fn parse_history_args(
    args: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> AnyResult<ParsedArgs> {
    let value_flags: Vec<_> = HISTORY_VALUE_FLAGS
        .iter()
        .chain(value_flags)
        .copied()
        .collect();
    crate::cli::parse_args(args, &value_flags, bool_flags)
}
