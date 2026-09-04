use crate::audit::{aggregate_audit_results, confidence_thresholds};
use crate::cli::{
    ParsedArgs, flag_bool, flag_optional_string, flag_positive_f64, flag_positive_usize,
    flag_string, flag_usize, parse_args, parse_modes,
};
use crate::evaluation::{
    evaluate_audit_on_demand, evaluate_global, evaluate_on_demand,
    prepare_rename_aware_audit_history,
};
use crate::filters::{
    broad_change_hints, filter_related_results, filtered_query_top, parse_exclude_patterns,
    path_matches_any_pattern, query_hints,
};
use crate::git_utils::{
    git_audit_candidate_paths, git_diff_audit_paths, git_diff_audit_paths_for_range,
    git_diff_names, git_path_is_tracked, git_worktree_audit_paths,
};
use crate::graph::{build_graph_data, query_direct_from_commits};
use crate::history::{
    git_diff_tree_direct_for_target, git_followed_commits_for_targets, git_log,
    git_log_direct_for_target, git_log_direct_for_target_remove_empty, git_log_for_target,
    git_log_for_target_batch, git_log_for_target_batch_parallel, git_log_for_target_diff_tree,
    git_log_for_target_diff_tree_parallel, git_log_for_target_remove_empty,
    git_log_for_target_rev_list, git_log_rename_aware, gix_log_for_git_selected_target,
    gix_log_for_target,
};
use crate::model::*;
use crate::output::{
    OutputFormat, confidence_name, escape_text, parse_output_format, print_audit, print_audit_eval,
    print_eval, print_json, print_query, short_hash,
};
use crate::pack::{
    git_log_for_target_pack_fast, git_log_for_target_pack_scan, git_pack_fast_direct_for_target,
    git_pack_scan_direct_for_target,
};
use crate::path_utils::{normalize_input_path, pair_key};
use crate::repo::RepoContext;
use crate::{
    AUDIT_JSON_SCHEMA_VERSION, AnyResult, AuditFindingsError, DEFAULT_AUDIT_TOP, DEFAULT_EVIDENCE,
    DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_COMMITS, DEFAULT_MAX_FILES, DEFAULT_ON_DEMAND_BACKEND,
    DEFAULT_TOP, EXIT_AUDIT_FINDINGS, JSON_SCHEMA_VERSION,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn run(args: Vec<String>) -> AnyResult<()> {
    let mut stdout = io::stdout();
    run_with_writer(args, &mut stdout)
}

pub(crate) fn run_with_writer<W: Write>(args: Vec<String>, out: &mut W) -> AnyResult<()> {
    let Some(command) = args.first().map(String::as_str) else {
        print_usage(out)?;
        return Ok(());
    };

    match command {
        "query" => cmd_query(&args[1..], out),
        "explain" => cmd_explain(&args[1..], out),
        "audit" => cmd_audit(&args[1..], out),
        "diff" => cmd_diff(&args[1..], out),
        "eval" => cmd_eval(&args[1..], out),
        "version" | "-V" | "--version" => {
            writeln!(out, "related {}", env!("CARGO_PKG_VERSION"))?;
            Ok(())
        }
        "help" => print_command_usage(args.get(1).map(String::as_str), out),
        "-h" | "--help" => {
            print_usage(out)?;
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
}

fn print_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"related: changed-set omission audits from Git co-change history

Usage:
  related audit [--staged | --range REVISION_RANGE] [--top N] [--min-confidence LEVEL] [--fail-on-confidence LEVEL]
  related eval [--repo PATH] [--test-commits N] [--train-commits N]

Audit checks the current changed set for historically coupled files that may
have been omitted. It uses Git history without reading source contents.
Eval defaults to chronological changed-set omission evaluation.
Run related <command> --help for command-specific options."#
    )?;
    Ok(())
}

fn print_command_usage<W: Write>(command: Option<&str>, out: &mut W) -> AnyResult<()> {
    match command {
        None => print_usage(out),
        Some("query") => print_query_usage(out),
        Some("explain") => print_explain_usage(out),
        Some("audit") => print_audit_usage(out),
        Some("diff") => print_diff_usage(out),
        Some("eval") => print_eval_usage(out),
        Some(other) => Err(format!("unknown command {other:?}").into()),
    }
}

fn print_query_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"Usage: related query <file> [options]

Options:
  --repo PATH                 Repository or subdirectory (default: .)
  --mode MODE                 direct, pagerank, path, or hot (default: direct)
  --top N                     Maximum results (default: {DEFAULT_TOP})
  --format FORMAT             text or json (default: text)
  --evidence N                Evidence commits per result (default: 0)
  --accuracy LEVEL            fast or exact (default: fast)
  --history-backend NAME      History reader (default: {DEFAULT_ON_DEMAND_BACKEND})
  --max-commits N             Target commits to use (default: {DEFAULT_MAX_COMMITS}; 0 = unlimited)
  --since DATE                Restrict history by Git date expression
  --max-files-per-commit N    Ignore broader commits (default: {DEFAULT_MAX_FILES})
  --half-life-days N          Time-decay half-life (default: {DEFAULT_HALF_LIFE_DAYS})
  --jobs N                    Worker count for supported backends
  --scan-commits N            Pack-walk scan limit (0 = backend default)
  --exclude PATTERNS          Comma-separated path patterns to hide
  -h, --help                  Show this help"#
    )?;
    Ok(())
}

fn print_explain_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"Usage: related explain <file-a> <file-b> [options]

Options:
  --repo PATH                 Repository or subdirectory (default: .)
  --format FORMAT             text or json (default: text)
  --evidence N                Evidence commits to show (default: {DEFAULT_EVIDENCE})
  --accuracy LEVEL            fast or exact (default: fast)
  --history-backend NAME      History reader (default: {DEFAULT_ON_DEMAND_BACKEND})
  --max-commits N             Target commits to use (default: {DEFAULT_MAX_COMMITS}; 0 = unlimited)
  --since DATE                Restrict history by Git date expression
  --max-files-per-commit N    Ignore broader commits (default: {DEFAULT_MAX_FILES})
  --half-life-days N          Time-decay half-life (default: {DEFAULT_HALF_LIFE_DAYS})
  --jobs N                    Worker count for supported backends
  --scan-commits N            Pack-walk scan limit (0 = backend default)
  -h, --help                  Show this help"#
    )?;
    Ok(())
}

fn print_audit_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"Usage: related audit [options]

Options:
  --staged                    Audit staged changes
  --range RANGE               Audit files changed in a Git revision range
  --repo PATH                 Repository or subdirectory (default: .)
  --mode MODE                 direct or pagerank (default: direct)
  --top N                     Maximum candidates (default: {DEFAULT_AUDIT_TOP})
  --min-confidence LEVEL      low, medium, or high (default: medium)
  --fail-on-confidence LEVEL  Exit 3 when a displayed candidate meets LEVEL
  --format FORMAT             text or json (default: text)
  --evidence N                Evidence commits per candidate (default: 0)
  --accuracy LEVEL            fast or exact (default: fast)
  --history-backend NAME      Advanced history reader override
  --max-commits N             Target commits to use (default: {DEFAULT_MAX_COMMITS}; 0 = unlimited)
  --since DATE                Restrict history by Git date expression
  --max-files-per-commit N    Ignore broader commits (default: {DEFAULT_MAX_FILES})
  --half-life-days N          Time-decay half-life (default: {DEFAULT_HALF_LIFE_DAYS})
  --jobs N                    Worker count for supported backends
  --scan-commits N            Pack-walk scan limit (0 = backend default)
  --exclude PATTERNS          Comma-separated path patterns to hide
  -h, --help                  Show this help

The default worktree scope includes tracked modifications and untracked files.
Low-confidence candidates are omitted unless --min-confidence low is used.
Confidence uses the strongest changed-file pair: low <2, medium 2-24, high >=25."#
    )?;
    Ok(())
}

fn print_diff_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"Usage: related diff [options]

Options:
  --staged                    Inspect staged changes instead of unstaged changes
  --repo PATH                 Repository or subdirectory (default: .)
  --mode MODE                 direct, pagerank, path, or hot (default: direct)
  --top N                     Maximum results (default: {DEFAULT_TOP})
  --format FORMAT             text or json (default: text)
  --evidence N                Evidence commits per result (default: 0)
  --accuracy LEVEL            fast or exact (default: fast)
  --history-backend NAME      History reader (default: {DEFAULT_ON_DEMAND_BACKEND})
  --max-commits N             Target commits to use (default: {DEFAULT_MAX_COMMITS}; 0 = unlimited)
  --since DATE                Restrict history by Git date expression
  --max-files-per-commit N    Ignore broader commits (default: {DEFAULT_MAX_FILES})
  --half-life-days N          Time-decay half-life (default: {DEFAULT_HALF_LIFE_DAYS})
  --jobs N                    Worker count for supported backends
  --scan-commits N            Pack-walk scan limit (0 = backend default)
  --exclude PATTERNS          Comma-separated path patterns to hide
  -h, --help                  Show this help"#
    )?;
    Ok(())
}

fn print_eval_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"Usage: related eval [options]

Options:
  --repo PATH                 Repository or subdirectory (default: .)
  --task TASK                 Evaluation task (default: audit)
  --min-confidence LEVEL      Audit threshold: low, medium, or high (default: medium)
  --test-commits N            Holdout commits (default: 200)
  --train-commits N           Training commits (default: 1000)
  --top N                     Evaluation cutoff (default: 5)
  --format FORMAT             text or json (default: text)
  --max-files-per-commit N    Ignore broader commits (default: {DEFAULT_MAX_FILES})
  --half-life-days N          Time-decay half-life (default: {DEFAULT_HALF_LIFE_DAYS})
  --modes MODES               Comma-separated ranking modes
  -h, --help                  Show this help"#
    )?;
    Ok(())
}

fn cmd_query<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_args(
        args,
        &[
            "repo",
            "mode",
            "top",
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
            "exclude",
        ],
        &["on-demand", "help", "h"],
    )?;
    if flag_bool(&parsed, "help") || flag_bool(&parsed, "h") {
        return print_query_usage(out);
    }
    if parsed.positionals.len() != 1 {
        return Err("query requires exactly one file".into());
    }
    cmd_query_on_demand(&parsed, out)
}

fn cmd_query_on_demand<W: Write>(parsed: &ParsedArgs, out: &mut W) -> AnyResult<()> {
    let repo = flag_string(parsed, "repo", ".");
    let mode = flag_string(parsed, "mode", "direct");
    validate_query_mode(&mode)?;
    let top = flag_positive_usize(parsed, "top", DEFAULT_TOP)?;
    let output_format = parse_output_format(&flag_string(parsed, "format", "text"))?;
    let exclude_patterns = parse_exclude_patterns(parsed);
    let mut config = parse_on_demand_config(parsed, 0)?;

    let repo = RepoContext::discover(&repo)?;
    let root = repo.root_str()?;
    let backend_hint = configure_backend_for_repo(&repo, &mut config)?;
    let target = normalize_input_path(&repo.root, &repo.input_base, &parsed.positionals[0])?;
    if !git_path_is_tracked(root, &target)? {
        return Err(format!(
            "{:?} is not tracked in the repository",
            parsed.positionals[0]
        )
        .into());
    }
    let query_top = filtered_query_top(top, &exclude_patterns);
    let (mut related, runtime_backend_hint) = with_default_pack_fallback(&mut config, |config| {
        query_on_demand(root, &target, &mode, query_top, config)
    })?;
    filter_related_results(&mut related, &exclude_patterns, top);
    let mut hints = query_hints(&related, &exclude_patterns);
    if flag_bool(parsed, "on-demand") {
        hints.insert(
            0,
            "--on-demand is redundant because query already runs on demand.".to_string(),
        );
    }
    if let Some(hint) = runtime_backend_hint {
        hints.insert(0, hint);
    }
    if let Some(hint) = backend_hint {
        hints.insert(0, hint);
    }
    let output = QueryOutput {
        schema_version: JSON_SCHEMA_VERSION,
        target,
        mode: format!("{mode}:on-demand:{:?}", config.backend),
        related,
        hints,
    };
    match output_format {
        OutputFormat::Text => print_query(out, &output)?,
        OutputFormat::Json => print_json(out, &output)?,
    }
    Ok(())
}

fn query_on_demand(
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

fn query_from_commits(
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

fn parse_on_demand_config(
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

fn build_on_demand_graph_data(
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

fn cmd_explain<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_args(
        args,
        &[
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
        ],
        &["help", "h"],
    )?;
    if flag_bool(&parsed, "help") || flag_bool(&parsed, "h") {
        return print_explain_usage(out);
    }
    if parsed.positionals.len() != 2 {
        return Err("explain requires exactly two files".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let output_format = parse_output_format(&flag_string(&parsed, "format", "text"))?;
    let mut config = parse_on_demand_config(&parsed, DEFAULT_EVIDENCE)?;
    let repo = RepoContext::discover(&repo)?;
    let root = repo.root_str()?;
    let backend_hint = configure_backend_for_repo(&repo, &mut config)?;
    let (mut output, runtime_backend_hint) = with_default_pack_fallback(&mut config, |config| {
        explain_relationship(
            root,
            &repo.input_base,
            &parsed.positionals[0],
            &parsed.positionals[1],
            config,
        )
    })?;

    if let Some(hint) = backend_hint {
        output.hints.push(hint);
    }
    if let Some(hint) = runtime_backend_hint {
        output.hints.push(hint);
    }
    if output_format == OutputFormat::Json {
        return print_json(out, &output);
    }

    if !output.related {
        writeln!(
            out,
            "{} and {} have no direct co-change evidence in this history window.",
            escape_text(&output.a),
            escape_text(&output.b)
        )?;
        for hint in &output.hints {
            writeln!(out, "hint: {hint}")?;
        }
        return Ok(());
    }

    writeln!(
        out,
        "{} <-> {}",
        escape_text(&output.a),
        escape_text(&output.b)
    )?;
    writeln!(
        out,
        "cochanged={} weight={:.6} last_seen={}",
        output.cochanges, output.weight, output.last_seen
    )?;
    for ev in &output.evidence {
        writeln!(
            out,
            "- {} {} files={} weight={:.6} {}",
            short_hash(&ev.hash),
            ev.date,
            ev.file_count,
            ev.weight,
            escape_text(&ev.subject)
        )?;
    }
    for hint in &output.hints {
        writeln!(out, "hint: {hint}")?;
    }
    Ok(())
}

fn cmd_audit<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_args(
        args,
        &[
            "repo",
            "range",
            "mode",
            "top",
            "min-confidence",
            "fail-on-confidence",
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
            "exclude",
        ],
        &["staged", "help", "h"],
    )?;
    if flag_bool(&parsed, "help") || flag_bool(&parsed, "h") {
        return print_audit_usage(out);
    }
    if !parsed.positionals.is_empty() {
        return Err("audit does not accept positional arguments".into());
    }
    if flag_bool(&parsed, "staged") && parsed.flags.contains_key("range") {
        return Err("--staged and --range cannot be used together".into());
    }

    let repo_arg = flag_string(&parsed, "repo", ".");
    let mode = flag_string(&parsed, "mode", "direct");
    if !matches!(mode.as_str(), "direct" | "pagerank") {
        return Err(format!("unknown audit mode {mode:?}; use direct or pagerank").into());
    }
    let top = flag_positive_usize(&parsed, "top", DEFAULT_AUDIT_TOP)?;
    let minimum_confidence = parse_confidence(&flag_string(&parsed, "min-confidence", "medium"))?;
    let fail_on_confidence = flag_optional_string(&parsed, "fail-on-confidence")
        .map(|value| parse_confidence(&value))
        .transpose()?;
    if fail_on_confidence.is_some_and(|threshold| threshold < minimum_confidence) {
        return Err("--fail-on-confidence cannot be lower than --min-confidence".into());
    }
    let output_format = parse_output_format(&flag_string(&parsed, "format", "text"))?;
    let exclude_patterns = parse_exclude_patterns(&parsed);
    let mut config = parse_on_demand_config(&parsed, 0)?;

    let repo = RepoContext::discover(&repo_arg)?;
    let root = repo.root_str()?;
    let backend_hint = configure_backend_for_repo(&repo, &mut config)?;
    let (scope, audit_paths) = if flag_bool(&parsed, "staged") {
        ("staged".to_string(), git_diff_audit_paths(root, true)?)
    } else if let Some(range) = flag_optional_string(&parsed, "range") {
        (
            format!("range:{range}"),
            git_diff_audit_paths_for_range(root, &range)?,
        )
    } else {
        ("worktree".to_string(), git_worktree_audit_paths(root)?)
    };
    if audit_paths.is_empty() {
        return Err(format!("no changed files found for {scope}").into());
    }
    let candidate_paths =
        git_audit_candidate_paths(root, flag_optional_string(&parsed, "range").as_deref())?;
    let seeds: Vec<String> = audit_paths.iter().map(|path| path.path.clone()).collect();
    let history_targets: Vec<String> = audit_paths
        .iter()
        .map(|path| path.history_path.clone())
        .collect();
    let changed_paths: HashSet<String> = audit_paths
        .iter()
        .flat_map(|path| [path.path.clone(), path.history_path.clone()])
        .collect();
    let diff_rename_mapping = audit_paths
        .iter()
        .any(|path| path.path != path.history_path);

    let per_seed_top = top.saturating_mul(8).max(64);
    let mut results_by_seed = Vec::with_capacity(seeds.len());
    let mut runtime_backend_hint = None;
    if config.backend == OnDemandBackend::GitCli {
        for (idx, (history_seed, commits)) in
            git_followed_commits_for_targets(root, &history_targets, &config)?
                .into_iter()
                .enumerate()
        {
            let seed = &seeds[idx];
            let mut results =
                query_from_commits(root, &history_seed, &commits, &mode, per_seed_top, &config)?;
            results.retain(|result| {
                candidate_paths.contains(&result.path)
                    && !changed_paths.contains(&result.path)
                    && !path_matches_any_pattern(&result.path, &exclude_patterns)
            });
            results_by_seed.push((seed.clone(), results));
        }
    } else {
        for (seed, history_seed) in seeds.iter().zip(&history_targets) {
            let (mut results, hint) = with_default_pack_fallback(&mut config, |config| {
                query_on_demand(root, history_seed, &mode, per_seed_top, config)
            })?;
            if runtime_backend_hint.is_none() {
                runtime_backend_hint = hint;
            }
            results.retain(|result| {
                candidate_paths.contains(&result.path)
                    && !changed_paths.contains(&result.path)
                    && !path_matches_any_pattern(&result.path, &exclude_patterns)
            });
            results_by_seed.push((seed.clone(), results));
        }
    }

    let (candidates, filtered_low_confidence) = aggregate_audit_results(
        &seeds,
        results_by_seed,
        minimum_confidence,
        top,
        config.evidence_limit,
    );
    let abstained = candidates.is_empty();
    let enforcement = fail_on_confidence.map(|threshold| {
        let finding_count = candidates
            .iter()
            .filter(|candidate| candidate.confidence >= threshold)
            .count();
        AuditEnforcement {
            threshold,
            finding_count,
            triggered: finding_count > 0,
            exit_code: EXIT_AUDIT_FINDINGS,
        }
    });
    let mut hints = broad_change_hints(
        &candidates
            .iter()
            .map(|candidate| candidate.path.as_str())
            .collect::<Vec<_>>(),
        &exclude_patterns,
    );
    if filtered_low_confidence > 0 {
        hints.push(format!(
            "Omitted {filtered_low_confidence} lower-confidence candidates; use --min-confidence low to inspect them."
        ));
    }
    if let Some(hint) = runtime_backend_hint {
        hints.insert(0, hint);
    }
    if let Some(hint) = backend_hint {
        hints.insert(0, hint);
    }
    let output = AuditOutput {
        schema_version: AUDIT_JSON_SCHEMA_VERSION,
        scope,
        seeds,
        mode,
        minimum_confidence,
        confidence_thresholds: confidence_thresholds(),
        candidates,
        abstained,
        enforcement,
        history_coverage: history_coverage(&config, diff_rename_mapping),
        hints,
    };
    match output_format {
        OutputFormat::Text => print_audit(out, &output)?,
        OutputFormat::Json => print_json(out, &output)?,
    }
    if let Some(enforcement) = &output.enforcement
        && enforcement.triggered
    {
        return Err(AuditFindingsError {
            count: enforcement.finding_count,
            threshold: confidence_name(enforcement.threshold).to_string(),
        }
        .into());
    }
    Ok(())
}

fn parse_confidence(value: &str) -> AnyResult<Confidence> {
    match value {
        "low" => Ok(Confidence::Low),
        "medium" => Ok(Confidence::Medium),
        "high" => Ok(Confidence::High),
        other => Err(format!("unknown confidence {other:?}; use low, medium, or high").into()),
    }
}

fn history_coverage(config: &OnDemandConfig, diff_rename_mapping: bool) -> HistoryCoverage {
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

pub(crate) fn explain_relationship(
    root: &str,
    input_base: &Path,
    a_input: &str,
    b_input: &str,
    config: &OnDemandConfig,
) -> AnyResult<ExplainOutput> {
    let target = normalize_input_path(Path::new(root), input_base, a_input)?;
    let data = build_on_demand_graph_data(root, &target, config)?;
    let graph = RelatedGraph::new(&data);
    let a = graph.resolve_path(root, input_base, a_input)?;
    let b = graph.resolve_path_or_tracked(root, input_base, b_input)?;
    let key = pair_key(&a, &b);
    let Some(pair) = graph.pairs.get(&key) else {
        return Ok(ExplainOutput {
            schema_version: JSON_SCHEMA_VERSION,
            a,
            b,
            related: false,
            cochanges: 0,
            weight: 0.0,
            last_seen: String::new(),
            evidence: Vec::new(),
            hints: Vec::new(),
        });
    };

    Ok(ExplainOutput {
        schema_version: JSON_SCHEMA_VERSION,
        a,
        b,
        related: true,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen: pair.last_seen.clone(),
        evidence: pair.evidence.clone(),
        hints: Vec::new(),
    })
}

fn cmd_diff<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_args(
        args,
        &[
            "repo",
            "mode",
            "top",
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
            "exclude",
        ],
        &["staged", "help", "h"],
    )?;
    if flag_bool(&parsed, "help") || flag_bool(&parsed, "h") {
        return print_diff_usage(out);
    }
    if !parsed.positionals.is_empty() {
        return Err("diff does not accept positional arguments".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let mode = flag_string(&parsed, "mode", "direct");
    validate_query_mode(&mode)?;
    let top = flag_positive_usize(&parsed, "top", DEFAULT_TOP)?;
    let output_format = parse_output_format(&flag_string(&parsed, "format", "text"))?;
    let staged = flag_bool(&parsed, "staged");
    let exclude_patterns = parse_exclude_patterns(&parsed);
    let mut config = parse_on_demand_config(&parsed, 0)?;

    let repo = RepoContext::discover(&repo)?;
    let root = repo.root_str()?;
    let backend_hint = configure_backend_for_repo(&repo, &mut config)?;
    let changed = git_diff_names(root, staged)?;
    if changed.is_empty() {
        return Err("no changed files found".into());
    }

    let changed_set: HashSet<String> = changed.iter().cloned().collect();
    let mut aggregate: HashMap<String, ResultItem> = HashMap::default();
    let query_top = filtered_query_top(top, &exclude_patterns);
    let mut runtime_backend_hint = None;
    for target in &changed {
        let (results, hint) = with_default_pack_fallback(&mut config, |config| {
            query_on_demand(root, target, &mode, query_top, config)
        })?;
        if runtime_backend_hint.is_none() {
            runtime_backend_hint = hint;
        }
        for result in results {
            if changed_set.contains(&result.path) {
                continue;
            }
            if let Some(previous) = aggregate.get_mut(&result.path) {
                merge_diff_result(previous, result, config.evidence_limit);
            } else {
                aggregate.insert(result.path.clone(), result);
            }
        }
    }
    let mut related: Vec<ResultItem> = aggregate.into_values().collect();
    filter_related_results(&mut related, &exclude_patterns, top);
    let mut hints = query_hints(&related, &exclude_patterns);
    if let Some(hint) = runtime_backend_hint {
        hints.insert(0, hint);
    }
    if let Some(hint) = backend_hint {
        hints.insert(0, hint);
    }
    let output = QueryOutput {
        schema_version: JSON_SCHEMA_VERSION,
        target: changed.join(","),
        mode,
        related,
        hints,
    };
    match output_format {
        OutputFormat::Text => print_query(out, &output)?,
        OutputFormat::Json => print_json(out, &output)?,
    }
    Ok(())
}

pub(crate) fn merge_diff_result(
    target: &mut ResultItem,
    source: ResultItem,
    evidence_limit: usize,
) {
    target.score += source.score;
    target.cochanges = target.cochanges.saturating_add(source.cochanges);
    target.weight += source.weight;
    if source.last_seen > target.last_seen {
        target.last_seen = source.last_seen;
    }
    if target.reason != source.reason {
        target.reason = "diff_aggregate".to_string();
    }
    if evidence_limit == 0 {
        return;
    }
    target.evidence.extend(source.evidence);
    target
        .evidence
        .sort_by(|left, right| right.date.cmp(&left.date).then(left.hash.cmp(&right.hash)));
    target
        .evidence
        .dedup_by(|left, right| left.hash == right.hash);
    target.evidence.truncate(evidence_limit);
}

fn cmd_eval<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_args(
        args,
        &[
            "repo",
            "test-commits",
            "train-commits",
            "top",
            "format",
            "max-files-per-commit",
            "half-life-days",
            "modes",
            "query-shape",
            "task",
            "min-confidence",
        ],
        &["help", "h"],
    )?;
    if flag_bool(&parsed, "help") || flag_bool(&parsed, "h") {
        return print_eval_usage(out);
    }
    if !parsed.positionals.is_empty() {
        return Err("eval does not accept positional arguments".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let test_commits = flag_positive_usize(&parsed, "test-commits", 200)?;
    let train_commits = flag_positive_usize(&parsed, "train-commits", 1000)?;
    let top = flag_positive_usize(&parsed, "top", 5)?;
    let output_format = parse_output_format(&flag_string(&parsed, "format", "text"))?;
    let max_files = flag_positive_usize(&parsed, "max-files-per-commit", DEFAULT_MAX_FILES)?;
    let half_life = flag_positive_f64(&parsed, "half-life-days", DEFAULT_HALF_LIFE_DAYS)?;
    let task = flag_string(&parsed, "task", "audit");
    if !matches!(task.as_str(), "query" | "audit") {
        return Err(format!("unknown eval task {task:?}; use audit").into());
    }
    let default_modes = if task == "audit" {
        "direct,pagerank"
    } else {
        "direct,pagerank,path,hot"
    };
    let modes = parse_modes(&flag_string(&parsed, "modes", default_modes));
    let query_shape = flag_string(&parsed, "query-shape", "on-demand");
    for mode in &modes {
        validate_query_mode(mode)?;
    }

    let repo = RepoContext::discover(&repo)?;
    let root = repo.root_str()?;
    let total = test_commits
        .checked_add(train_commits)
        .ok_or("test-commits and train-commits are too large")?;
    let graph_config = GraphBuildConfig {
        max_files_per_commit: max_files,
        half_life_days: half_life,
        evidence_limit: 0,
    };
    if task == "audit" {
        if query_shape != "on-demand" {
            return Err("audit evaluation supports only --query-shape on-demand".into());
        }
        let minimum_confidence =
            parse_confidence(&flag_string(&parsed, "min-confidence", "medium"))?;
        let records = git_log_rename_aware(root, total, None)?;
        let available_total = records.len().min(total);
        let history =
            prepare_rename_aware_audit_history(&records[..available_total], test_commits)?;
        let mut report = evaluate_audit_on_demand(
            &history.train,
            &history.test,
            &modes,
            top,
            graph_config,
            minimum_confidence,
        )?;
        report.repo_root = root.to_string();
        report.train_commits = history.train.len();
        report.test_commits = history.test.len();
        report.top_k = top;
        report.max_files_per_commit = max_files;
        report.rename_tracking = "training-window+current-test-diff".to_string();
        report.training_renames = history.training_renames;
        report.test_diff_renames = history.test_diff_renames;
        match output_format {
            OutputFormat::Text => print_audit_eval(out, &report)?,
            OutputFormat::Json => print_json(out, &report)?,
        }
        return Ok(());
    }
    let commits = git_log(root, total, None)?;
    if commits.len() <= test_commits {
        return Err(format!("not enough commits for evaluation: got {}", commits.len()).into());
    }
    let available_total = commits.len().min(total);
    let test = &commits[..test_commits];
    let train = &commits[test_commits..available_total];
    let mut report = match query_shape.as_str() {
        "on-demand" => evaluate_on_demand(train, test, &modes, top, graph_config)?,
        "global" => {
            let data = build_graph_data(root, train, graph_config);
            let graph = RelatedGraph::new(&data);
            evaluate_global(&graph, test, &modes, top, max_files)?
        }
        other => {
            return Err(format!("unknown query shape {other:?}; use on-demand or global").into());
        }
    };
    report.repo_root = root.to_string();
    report.train_commits = train.len();
    report.test_commits = test.len();
    report.top_k = top;
    report.max_files_per_commit = max_files;

    match output_format {
        OutputFormat::Text => print_eval(out, &report)?,
        OutputFormat::Json => print_json(out, &report)?,
    }
    Ok(())
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn validate_query_mode(mode: &str) -> AnyResult<()> {
    match mode {
        "direct" | "pagerank" | "path" | "hot" => Ok(()),
        other => Err(format!("unknown mode {other:?}; use direct, pagerank, path, or hot").into()),
    }
}

fn configure_backend_for_repo(
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

fn with_default_pack_fallback<T>(
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
