use crate::cli::{
    ParsedArgs, flag_bool, flag_optional_string, flag_positive_f64, flag_positive_usize,
    flag_string, flag_usize, parse_args, parse_modes,
};
use crate::evaluation::{evaluate_global, evaluate_on_demand};
use crate::filters::{
    filter_related_results, filtered_query_top, parse_exclude_patterns, query_hints,
};
use crate::git_utils::{git_diff_names, git_path_is_tracked};
use crate::graph::{build_graph_data, query_direct_from_commits};
use crate::history::{
    git_diff_tree_direct_for_target, git_log, git_log_direct_for_target,
    git_log_direct_for_target_remove_empty, git_log_for_target, git_log_for_target_batch,
    git_log_for_target_batch_parallel, git_log_for_target_diff_tree,
    git_log_for_target_diff_tree_parallel, git_log_for_target_remove_empty,
    git_log_for_target_rev_list, gix_log_for_git_selected_target, gix_log_for_target,
};
use crate::model::*;
use crate::output::{escape_text, print_eval, print_query, short_hash};
use crate::pack::{
    git_log_for_target_pack_fast, git_log_for_target_pack_scan, git_pack_fast_direct_for_target,
    git_pack_scan_direct_for_target,
};
use crate::path_utils::{normalize_input_path, pair_key};
use crate::repo::RepoContext;
use crate::{
    AnyResult, DEFAULT_EVIDENCE, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_COMMITS, DEFAULT_MAX_FILES,
    DEFAULT_ON_DEMAND_BACKEND, DEFAULT_TOP,
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
        "diff" => cmd_diff(&args[1..], out),
        "eval" => cmd_eval(&args[1..], out),
        "version" | "-V" | "--version" => {
            writeln!(out, "related {}", env!("CARGO_PKG_VERSION"))?;
            Ok(())
        }
        "help" | "-h" | "--help" => {
            print_usage(out)?;
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
}

fn print_usage<W: Write>(out: &mut W) -> AnyResult<()> {
    writeln!(
        out,
        r#"related: content-blind related-file ranking from Git co-change history

Usage:
  related query <file> [--mode direct|pagerank|path|hot] [--top N] [--exclude PATTERNS]
  related query <file> [--history-backend hybrid|gix|git|git-remove-empty|git-batch|git-batch-parallel|git-diff-tree|git-diff-tree-parallel|git-rev-list|pack-fast|pack-scan] [--max-commits N] [--jobs N]
  related explain <file-a> <file-b> [--max-commits N]
  related diff [--staged] [--mode direct|pagerank|path|hot] [--top N] [--exclude PATTERNS] [--max-commits N]
  related eval [--repo PATH] [--query-shape on-demand|global] [--test-commits N] [--train-commits N]

The graph is built on demand from files that changed together in Git commits.
No source parsing, imports, embeddings, or file contents are used.
Relative file paths are resolved from --repo (or the current directory), and
query targets must be tracked by Git. Eval defaults to the target-local
on-demand query shape; use --query-shape global for the research graph."#
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
            "evidence",
            "history-backend",
            "max-commits",
            "since",
            "max-files-per-commit",
            "half-life-days",
            "jobs",
            "scan-commits",
            "exclude",
        ],
        &["on-demand"],
    )?;
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
    let mut related = query_on_demand(root, &target, &mode, query_top, &config)?;
    filter_related_results(&mut related, &exclude_patterns, top);
    let mut hints = query_hints(&related, &exclude_patterns);
    if let Some(hint) = backend_hint {
        hints.insert(0, hint);
    }
    let output = QueryOutput {
        target,
        mode: format!("{mode}:on-demand:{:?}", config.backend),
        related,
        hints,
    };
    print_query(out, &output)?;
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
    if mode == "direct" {
        return Ok(query_direct_from_commits(
            target,
            &commits,
            config,
            top,
            config.evidence_limit as isize,
        ));
    }
    let data = build_graph_data(
        root,
        &commits,
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
    Ok(OnDemandConfig {
        backend: parse_on_demand_backend(&flag_string(
            parsed,
            "history-backend",
            DEFAULT_ON_DEMAND_BACKEND,
        ))?,
        backend_explicit: parsed.flags.contains_key("history-backend"),
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
            "evidence",
            "history-backend",
            "max-commits",
            "since",
            "max-files-per-commit",
            "half-life-days",
            "jobs",
            "scan-commits",
        ],
        &[],
    )?;
    if parsed.positionals.len() != 2 {
        return Err("explain requires exactly two files".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let mut config = parse_on_demand_config(&parsed, DEFAULT_EVIDENCE)?;
    let repo = RepoContext::discover(&repo)?;
    let root = repo.root_str()?;
    let backend_hint = configure_backend_for_repo(&repo, &mut config)?;
    let output = explain_relationship(
        root,
        &repo.input_base,
        &parsed.positionals[0],
        &parsed.positionals[1],
        &config,
    )?;

    if !output.related {
        writeln!(
            out,
            "{} and {} have no direct co-change evidence in this history window.",
            escape_text(&output.a),
            escape_text(&output.b)
        )?;
        if let Some(hint) = backend_hint {
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
    if let Some(hint) = backend_hint {
        writeln!(out, "hint: {hint}")?;
    }
    Ok(())
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
            a,
            b,
            related: false,
            cochanges: 0,
            weight: 0.0,
            last_seen: String::new(),
            evidence: Vec::new(),
        });
    };

    Ok(ExplainOutput {
        a,
        b,
        related: true,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen: pair.last_seen.clone(),
        evidence: pair.evidence.clone(),
    })
}

fn cmd_diff<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_args(
        args,
        &[
            "repo",
            "mode",
            "top",
            "evidence",
            "history-backend",
            "max-commits",
            "since",
            "max-files-per-commit",
            "half-life-days",
            "jobs",
            "scan-commits",
            "exclude",
        ],
        &["staged"],
    )?;
    if !parsed.positionals.is_empty() {
        return Err("diff does not accept positional arguments".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let mode = flag_string(&parsed, "mode", "direct");
    validate_query_mode(&mode)?;
    let top = flag_positive_usize(&parsed, "top", DEFAULT_TOP)?;
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
    for target in &changed {
        for result in query_on_demand(root, target, &mode, query_top, &config)? {
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
    if let Some(hint) = backend_hint {
        hints.insert(0, hint);
    }
    let output = QueryOutput {
        target: changed.join(","),
        mode,
        related,
        hints,
    };
    print_query(out, &output)?;
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
            "max-files-per-commit",
            "half-life-days",
            "modes",
            "query-shape",
        ],
        &[],
    )?;
    if !parsed.positionals.is_empty() {
        return Err("eval does not accept positional arguments".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let test_commits = flag_positive_usize(&parsed, "test-commits", 200)?;
    let train_commits = flag_positive_usize(&parsed, "train-commits", 1000)?;
    let top = flag_positive_usize(&parsed, "top", 10)?;
    let max_files = flag_positive_usize(&parsed, "max-files-per-commit", DEFAULT_MAX_FILES)?;
    let half_life = flag_positive_f64(&parsed, "half-life-days", DEFAULT_HALF_LIFE_DAYS)?;
    let modes = parse_modes(&flag_string(&parsed, "modes", "direct,pagerank,path,hot"));
    let query_shape = flag_string(&parsed, "query-shape", "on-demand");
    for mode in &modes {
        validate_query_mode(mode)?;
    }

    let repo = RepoContext::discover(&repo)?;
    let root = repo.root_str()?;
    let total = test_commits
        .checked_add(train_commits)
        .ok_or("test-commits and train-commits are too large")?;
    let commits = git_log(root, total, None)?;
    if commits.len() <= test_commits {
        return Err(format!("not enough commits for evaluation: got {}", commits.len()).into());
    }
    let available_total = commits.len().min(total);
    let test = &commits[..test_commits];
    let train = &commits[test_commits..available_total];
    let graph_config = GraphBuildConfig {
        max_files_per_commit: max_files,
        half_life_days: half_life,
        evidence_limit: 0,
    };
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

    print_eval(out, &report)?;
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
