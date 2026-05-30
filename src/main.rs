mod cli;
mod filters;
mod git_utils;
mod output;
mod path_utils;

use cli::{
    ParsedArgs, flag_bool, flag_f64, flag_optional_string, flag_string, flag_usize, parse_args,
    parse_modes,
};
use filters::{filter_related_results, filtered_query_top, parse_exclude_patterns, query_hints};
use git_utils::{git_diff_names, git_path_is_tracked, run_git, run_git_with_stdin};
use gix::bstr::ByteSlice;
use output::{print_eval, print_query, short_hash};
use path_utils::{
    literal_pathspec, normalize_git_path, normalize_input_path, ordered_pair, pair_key,
    path_basename, path_similarity, path_tokens,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

const DEFAULT_MAX_FILES: usize = 80;
const DEFAULT_MAX_COMMITS: usize = 1000;
const DEFAULT_HALF_LIFE_DAYS: f64 = 365.0;
const DEFAULT_EVIDENCE: usize = 8;
const DEFAULT_TOP: usize = 20;
const DEFAULT_ON_DEMAND_BACKEND: &str = "pack-fast";
const DEFAULT_PACK_FAST_SCAN_COMMITS: usize = 17_500;
const PACK_FAST_MIN_SCAN_COMMITS: usize = 1_000;
const PACK_FAST_MIN_TARGET_COMMITS: usize = 256;
const PACK_FAST_STALL_COMMITS: usize = 5_000;
const PACK_DIRECT_PARALLEL_MIN_COMMITS: usize = 256;
const BROAD_CHANGE_EXCLUDE_SUGGESTION: &str = "*.lock,*-lock.*,*lockb,.github/workflows/*";

type AnyError = Box<dyn Error>;
type AnyResult<T> = Result<T, AnyError>;

#[derive(Clone, Debug)]
struct Commit {
    hash: String,
    unix_time: i64,
    date: String,
    subject: String,
    files: Vec<String>,
}

#[derive(Clone, Debug)]
struct Evidence {
    hash: String,
    date: String,
    subject: String,
    file_count: usize,
    weight: f64,
}

#[derive(Clone, Debug, Default)]
struct FileStat {
    changes: usize,
    weighted_changes: f64,
    last_seen: String,
}

#[derive(Clone, Debug)]
struct PairStat {
    a: String,
    b: String,
    cochanges: usize,
    weight: f64,
    last_seen: String,
    evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, Default)]
struct DirectPairStat<'a> {
    cochanges: usize,
    weight: f64,
    other_weight: f64,
    last_seen: &'a str,
    evidence: Vec<Evidence>,
}

struct DirectScoredPair<'a> {
    path: &'a str,
    pair: DirectPairStat<'a>,
    score: f64,
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

#[derive(Clone, Debug)]
struct GraphData {
    files: HashMap<String, FileStat>,
    pairs: Vec<PairStat>,
}

#[derive(Clone, Copy, Debug)]
struct GraphBuildConfig {
    max_files_per_commit: usize,
    half_life_days: f64,
    evidence_limit: usize,
}

#[derive(Clone, Debug)]
struct ResultItem {
    path: String,
    score: f64,
    cochanges: usize,
    weight: f64,
    last_seen: String,
    reason: String,
    evidence: Vec<Evidence>,
}

#[derive(Clone, Debug)]
struct QueryOutput {
    target: String,
    mode: String,
    related: Vec<ResultItem>,
    hints: Vec<String>,
}

#[derive(Clone, Debug)]
struct ExplainOutput {
    a: String,
    b: String,
    related: bool,
    cochanges: usize,
    weight: f64,
    last_seen: String,
    evidence: Vec<Evidence>,
}

#[derive(Clone, Debug)]
struct EvalReport {
    repo_root: String,
    train_commits: usize,
    test_commits: usize,
    top_k: usize,
    max_files_per_commit: usize,
    candidate_tasks: usize,
    evaluated_tasks: usize,
    skipped_unknown_seed: usize,
    skipped_no_known_target: usize,
    metrics: Vec<EvalMetrics>,
}

#[derive(Clone, Debug, Default)]
struct EvalMetrics {
    mode: String,
    tasks: usize,
    hit_rate_at_k: f64,
    precision_at_k: f64,
    recall_at_k: f64,
    mrr: f64,
    avg_results: f64,
}

#[derive(Clone, Debug)]
struct EvalAccumulator {
    mode: String,
    tasks: usize,
    hit_tasks: usize,
    precision_sum: f64,
    recall_sum: f64,
    mrr_sum: f64,
    results_sum: usize,
}

struct RelatedGraph<'a> {
    data: &'a GraphData,
    pairs: HashMap<String, PairStat>,
    adj: HashMap<String, HashMap<String, f64>>,
    degree: HashMap<String, f64>,
    paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GraphPathMatch<'a> {
    Known(String),
    Missing(String),
    Ambiguous(Vec<&'a str>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OnDemandBackend {
    Hybrid,
    Gix,
    GitCli,
    GitBatch,
    GitBatchParallel,
    GitDiffTree,
    GitDiffTreeParallel,
    GitRevList,
    GitRemoveEmpty,
    PackScan,
    PackFast,
}

#[derive(Clone, Debug)]
struct GixCommitSeed {
    id: gix::hash::ObjectId,
    first_parent: Option<gix::hash::ObjectId>,
}

#[derive(Clone, Debug)]
struct OnDemandConfig {
    backend: OnDemandBackend,
    max_commits: usize,
    since: Option<String>,
    max_files_per_commit: usize,
    half_life_days: f64,
    evidence_limit: usize,
    jobs: usize,
    jobs_explicit: bool,
    scan_commits: usize,
}

fn main() {
    if let Err(err) = run(env::args().skip(1).collect()) {
        eprintln!("related: {err}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> AnyResult<()> {
    let mut stdout = io::stdout();
    run_with_writer(args, &mut stdout)
}

fn run_with_writer<W: Write>(args: Vec<String>, out: &mut W) -> AnyResult<()> {
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
  related eval [--repo PATH] [--test-commits N] [--train-commits N]

The graph is built on demand from files that changed together in Git commits.
No source parsing, imports, embeddings, or file contents are used."#
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
    let top = flag_usize(parsed, "top", DEFAULT_TOP)?;
    let exclude_patterns = parse_exclude_patterns(parsed);
    let config = parse_on_demand_config(parsed, 0)?;

    let root = git_root(&repo)?;
    let target = normalize_input_path(&root, &parsed.positionals[0]);
    let query_top = filtered_query_top(top, &exclude_patterns);
    let mut related = query_on_demand(&root, &target, &mode, query_top, &config)?;
    filter_related_results(&mut related, &exclude_patterns, top);
    let hints = query_hints(&related, &exclude_patterns);
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
        max_commits: flag_usize(parsed, "max-commits", DEFAULT_MAX_COMMITS)?,
        since: flag_optional_string(parsed, "since"),
        max_files_per_commit: flag_usize(parsed, "max-files-per-commit", DEFAULT_MAX_FILES)?,
        half_life_days: flag_f64(parsed, "half-life-days", DEFAULT_HALF_LIFE_DAYS)?,
        evidence_limit: flag_usize(parsed, "evidence", default_evidence)?,
        jobs: flag_usize(parsed, "jobs", default_jobs())?.max(1),
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
    let config = parse_on_demand_config(&parsed, DEFAULT_EVIDENCE)?;
    let root = git_root(&repo)?;
    let output = explain_relationship(
        &root,
        &parsed.positionals[0],
        &parsed.positionals[1],
        &config,
    )?;

    if !output.related {
        writeln!(
            out,
            "{} and {} have no direct co-change evidence in this history window.",
            output.a, output.b
        )?;
        return Ok(());
    }

    writeln!(out, "{} <-> {}", output.a, output.b)?;
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
            ev.subject
        )?;
    }
    Ok(())
}

fn explain_relationship(
    root: &str,
    a_input: &str,
    b_input: &str,
    config: &OnDemandConfig,
) -> AnyResult<ExplainOutput> {
    let target = normalize_input_path(root, a_input);
    let data = build_on_demand_graph_data(root, &target, config)?;
    let graph = RelatedGraph::new(&data);
    let a = graph.resolve_path(root, &target)?;
    let b = graph.resolve_path_or_tracked(root, b_input)?;
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
    let top = flag_usize(&parsed, "top", DEFAULT_TOP)?;
    let staged = flag_bool(&parsed, "staged");
    let exclude_patterns = parse_exclude_patterns(&parsed);
    let config = parse_on_demand_config(&parsed, 0)?;

    let root = git_root(&repo)?;
    let changed = git_diff_names(&root, staged)?;
    if changed.is_empty() {
        return Err("no changed files found".into());
    }

    let changed_set: HashSet<String> = changed.iter().cloned().collect();
    let mut aggregate: HashMap<String, ResultItem> = HashMap::default();
    let query_top = filtered_query_top(top, &exclude_patterns);
    for target in &changed_set {
        for mut result in query_on_demand(&root, target, &mode, query_top, &config)? {
            if changed_set.contains(&result.path) {
                continue;
            }
            if let Some(previous) = aggregate.remove(&result.path) {
                result.score += previous.score;
                result.cochanges = result.cochanges.max(previous.cochanges);
            }
            aggregate.insert(result.path.clone(), result);
        }
    }
    let mut related: Vec<ResultItem> = aggregate.into_values().collect();
    filter_related_results(&mut related, &exclude_patterns, top);
    let hints = query_hints(&related, &exclude_patterns);
    let output = QueryOutput {
        target: changed.join(","),
        mode,
        related,
        hints,
    };
    print_query(out, &output)?;
    Ok(())
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
        ],
        &[],
    )?;
    if !parsed.positionals.is_empty() {
        return Err("eval does not accept positional arguments".into());
    }

    let repo = flag_string(&parsed, "repo", ".");
    let test_commits = flag_usize(&parsed, "test-commits", 200)?;
    let train_commits = flag_usize(&parsed, "train-commits", 1000)?;
    let top = flag_usize(&parsed, "top", 10)?;
    let max_files = flag_usize(&parsed, "max-files-per-commit", DEFAULT_MAX_FILES)?;
    let half_life = flag_f64(&parsed, "half-life-days", DEFAULT_HALF_LIFE_DAYS)?;
    let modes = parse_modes(&flag_string(&parsed, "modes", "direct,pagerank,path,hot"));
    if test_commits == 0 || train_commits == 0 {
        return Err("test-commits and train-commits must be positive".into());
    }
    if top == 0 {
        return Err("top must be positive".into());
    }

    let root = git_root(&repo)?;
    let total = test_commits + train_commits;
    let commits = git_log(&root, total, None)?;
    if commits.len() < test_commits + 1 {
        return Err(format!("not enough commits for evaluation: got {}", commits.len()).into());
    }
    let available_total = commits.len().min(total);
    let test = &commits[..test_commits];
    let train = &commits[test_commits..available_total];
    let data = build_graph_data(
        &root,
        train,
        GraphBuildConfig {
            max_files_per_commit: max_files,
            half_life_days: half_life,
            evidence_limit: 0,
        },
    );
    let graph = RelatedGraph::new(&data);
    let mut report = evaluate(&graph, test, &modes, top, max_files)?;
    report.repo_root = root;
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

fn git_root(repo: &str) -> AnyResult<String> {
    let abs = fs::canonicalize(repo)?;
    if looks_like_worktree_root(&abs) {
        return Ok(abs.display().to_string());
    }
    let out = run_git(&abs, &["rev-parse", "--show-toplevel"])?;
    Ok(String::from_utf8(out)?.trim().to_string())
}

fn looks_like_worktree_root(path: &Path) -> bool {
    let git = path.join(".git");
    if git.is_file() {
        return true;
    }
    git.is_dir() && git.join("HEAD").is_file()
}

fn git_log(repo: &str, max_commits: usize, since: Option<&str>) -> AnyResult<Vec<Commit>> {
    let mut args = vec![
        "log".to_string(),
        "--name-only".to_string(),
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

fn git_log_for_target(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    git_log_for_target_git(repo, target, max_commits, since, false)
}

fn git_log_for_target_remove_empty(
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
    let mut args = vec![
        "log".to_string(),
        "--no-renames".to_string(),
        "--full-diff".to_string(),
        "--name-only".to_string(),
        "--diff-filter=ACMRT".to_string(),
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s".to_string(),
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
    parse_git_log(&out)
}

fn git_log_for_target_batch(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_commits, since)?;
    git_show_selected_commits(repo, &seeds)
}

fn git_log_for_target_batch_parallel(
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

fn git_log_for_target_diff_tree(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_target_commit_seeds(repo, target, max_commits, since)?;
    git_diff_tree_selected_commits(repo, &seeds)
}

fn git_log_for_target_diff_tree_parallel(
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

fn git_log_for_target_rev_list(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<Commit>> {
    let seeds = git_rev_list_target_commit_seeds(repo, target, max_commits, since)?;
    git_diff_tree_selected_commits(repo, &seeds)
}

fn git_log_for_target_pack_scan(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
    scan_commits: usize,
) -> AnyResult<Vec<Commit>> {
    pack_log_for_target(repo, target, max_commits, since, scan_commits)
}

fn git_log_for_target_pack_fast(
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

fn git_log_direct_for_target(
    repo: &str,
    target: &str,
    config: &OnDemandConfig,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    git_log_direct_for_target_git(repo, target, config, top, false)
}

fn git_log_direct_for_target_remove_empty(
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
    let pretty = if config.evidence_limit == 0 {
        "--pretty=format:%x1e%ct%x1f%cI"
    } else {
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s"
    };
    let mut args = vec![
        "log".to_string(),
        "--no-renames".to_string(),
        "--full-diff".to_string(),
        "--name-only".to_string(),
        "--diff-filter=ACMRT".to_string(),
        pretty.to_string(),
    ];
    if remove_empty {
        args.push("--remove-empty".to_string());
    }
    if config.max_commits > 0 {
        args.push(format!("--max-count={}", config.max_commits));
    }
    if let Some(since) = &config.since {
        args.push("--since".to_string());
        args.push(since.clone());
    }
    args.push("--".to_string());
    args.push(literal_pathspec(target));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_git(Path::new(repo), &arg_refs)?;
    parse_git_log_direct(&out, target, config, top, config.evidence_limit as isize)
}

fn git_pack_scan_direct_for_target(
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

fn git_pack_fast_direct_for_target(
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

fn git_diff_tree_direct_for_target(
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
        "--diff-filter=ACMRT",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
    ];
    let out = run_git_with_stdin(Path::new(repo), &args, input.as_bytes())?;
    parse_git_log(&out)
}

fn git_diff_tree_selected_commits(repo: &str, seeds: &[GixCommitSeed]) -> AnyResult<Vec<Commit>> {
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
        "--diff-filter=ACMRT",
        "--pretty=format:%x1e%H%x1f%ct%x1f%cI%x1f%s",
    ];
    let out = run_git_with_stdin(Path::new(repo), &args, input.as_bytes())?;
    parse_git_log(&out)
}

fn git_diff_tree_direct_from_hash_input(
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
        "--diff-filter=ACMRT",
        pretty,
    ];
    let out = run_git_with_stdin(Path::new(repo), &args, input)?;
    parse_git_log_direct(&out, target, config, top, config.evidence_limit as isize)
}

fn gix_log_for_target(
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

fn gix_log_for_git_selected_target(
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

fn git_target_commit_seeds(
    repo: &str,
    target: &str,
    max_commits: usize,
    since: Option<&str>,
) -> AnyResult<Vec<GixCommitSeed>> {
    let out = git_target_commit_hash_input(repo, target, max_commits, since)?;
    parse_commit_seeds(&out)
}

fn git_target_commit_hash_input(
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
        |store, id, raw| {
            let files = pack_changed_files_for_commit(store, raw, diff_file_limit)?;
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
                &target,
                &selected,
                max_files,
                half_life,
                top,
                config.jobs,
            );
        }
        return pack_direct_for_selected_serial(
            &mut store, &target, &selected, max_files, half_life, top,
        );
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
        |store, id, raw| {
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
                if other == &target {
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
) -> AnyResult<Vec<(RawObjectId, RawCommit)>> {
    let mut selected = Vec::with_capacity(max_commits.min(1024));
    pack_visit_target_commits(
        store,
        target,
        max_commits,
        since_seconds,
        scan_commits,
        latency_bounded_scan,
        |_, id, raw| {
            selected.push((id, raw.clone()));
            Ok(())
        },
    )?;
    Ok(selected)
}

fn pack_direct_for_selected_serial(
    store: &mut RawGitStore,
    target: &str,
    selected: &[(RawObjectId, RawCommit)],
    max_files: usize,
    half_life: f64,
    top: usize,
) -> AnyResult<Vec<ResultItem>> {
    let Some((_, latest_commit)) = selected.first() else {
        return Ok(Vec::new());
    };
    let latest = latest_commit.time;
    let diff_file_limit = Some(max_files.saturating_add(1));
    let mut partial = PackDirectPartial::new(top);
    let mut files = Vec::with_capacity(max_files.saturating_add(1));
    let mut prefix = Vec::new();
    for (_, raw) in selected {
        pack_direct_add_commit_no_evidence(
            store,
            target,
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
    target: &str,
    selected: &[(RawObjectId, RawCommit)],
    max_files: usize,
    half_life: f64,
    top: usize,
    jobs: usize,
) -> AnyResult<Vec<ResultItem>> {
    let Some((_, latest_commit)) = selected.first() else {
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
                for (_, raw) in chunk {
                    pack_direct_add_commit_no_evidence(
                        &mut store,
                        target,
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
    mut visitor: impl FnMut(&mut RawGitStore, RawObjectId, &RawCommit) -> AnyResult<()>,
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
            visitor(store, item.id, &commit)?;
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
    first_parent: Option<PackWalkParent>,
    extra_parents: Vec<PackWalkParent>,
}

impl PackPathDecision {
    fn new(include: bool) -> Self {
        Self {
            include,
            first_parent: None,
            extra_parents: Vec::new(),
        }
    }

    fn one_parent(include: bool, parent: PackWalkParent) -> Self {
        Self {
            include,
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
    components: Vec<Vec<u8>>,
    entry_cache: HashMap<RawObjectId, Option<RawTreeEntry>>,
    child_cache: HashMap<(RawObjectId, usize), Option<RawTreeEntry>>,
}

impl PackTargetPath {
    fn new(target: &str) -> Self {
        Self {
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
                self.entry_cache.insert(tree_id, None);
                return Ok(None);
            };
            current = entry.id;
            found = Some(entry);
        }
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

fn pack_path_history_decision(
    store: &mut RawGitStore,
    target: &mut PackTargetPath,
    commit: &RawCommit,
) -> AnyResult<PackPathDecision> {
    if commit.parents.is_empty() {
        return Ok(PackPathDecision {
            include: target.entry_at_path(store, commit.tree)?.is_some(),
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
                return Ok(PackPathDecision::one_parent(false, walk_parent));
            } else {
                decision.include |= new_exists;
                decision.push_parent(walk_parent);
            }
        }
    }

    if !saw_parent {
        decision.include = target.entry_at_path(store, commit.tree)?.is_some();
    }
    Ok(decision)
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
            pack_diff_trees(store, old_child, Some(entry.id), prefix, out, file_limit)?;
        } else {
            out.push(String::from_utf8_lossy(prefix).into_owned());
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
    commit_cache: HashMap<RawObjectId, RawCommit>,
    tree_entries_cache: HashMap<RawObjectId, Arc<[RawNamedTreeEntry]>>,
    offset_cache: HashMap<(usize, u64), RawGitObject>,
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
            commit_cache: HashMap::default(),
            tree_entries_cache: HashMap::default(),
            offset_cache: HashMap::default(),
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
            commit_cache: HashMap::default(),
            tree_entries_cache: HashMap::default(),
            offset_cache: HashMap::default(),
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
        self.tree_entries_cache
            .insert(tree_id, Arc::clone(&entries));
        Ok(entries)
    }

    fn find_object(&mut self, id: RawObjectId) -> AnyResult<RawGitObject> {
        Ok(self.find_object_ref(id)?.clone())
    }

    fn find_object_ref(&mut self, id: RawObjectId) -> AnyResult<&RawGitObject> {
        if !self.object_cache.contains_key(&id) {
            let object = self.load_object(id)?;
            self.object_cache.insert(id, object);
        }
        match self.object_cache.get(&id) {
            Some(object) => Ok(object),
            None => Err(format!("object {} not found after load", id.to_hex()).into()),
        }
    }

    fn load_object(&mut self, id: RawObjectId) -> AnyResult<RawGitObject> {
        Ok(if let Some(object) = self.find_loose_object(id)? {
            object
        } else if let Some((pack_index, offset)) = self.find_pack_offset(id) {
            self.find_pack_object_at(pack_index, offset)?
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
        let mut decoder = flate2::read::ZlibDecoder::new(file);
        let mut inflated = Vec::new();
        decoder.read_to_end(&mut inflated)?;
        let Some(header_end) = inflated.iter().position(|byte| *byte == 0) else {
            return Err("loose object missing header delimiter".into());
        };
        let header = std::str::from_utf8(&inflated[..header_end])?;
        let kind = header
            .split_once(' ')
            .map(|(kind, _)| raw_kind_from_name(kind))
            .transpose()?
            .ok_or("loose object missing kind")?;
        Ok(Some(RawGitObject {
            kind,
            data: inflated[header_end + 1..].to_vec().into(),
        }))
    }

    fn find_pack_object_at(&mut self, pack_index: usize, offset: u64) -> AnyResult<RawGitObject> {
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
                let base = match base {
                    PackedBase::Offset(base_offset) => {
                        self.find_pack_object_at(pack_index, base_offset)?
                    }
                    PackedBase::Id(base_id) => self.find_object(base_id)?,
                };
                RawGitObject {
                    kind: base.kind,
                    data: apply_pack_delta(&base.data, &raw.data)?.into(),
                }
            }
            other => return Err(format!("unsupported pack object type {other}").into()),
        };
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
        let count = fanout[255] as usize;
        let names_start = 8 + 256 * 4;
        let crc_start = names_start + count * 20;
        let offsets_start = crc_start + count * 4;
        let large_offsets_start = offsets_start + count * 4;
        if data.len() < large_offsets_start + 40 {
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
        size |= ((byte & 0x7f) as u64) << shift;
        shift += 7;
    }
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
    let mut decoder =
        flate2::bufread::ZlibDecoder::new(pack.get(pos..).ok_or("truncated pack object")?);
    let mut data = Vec::with_capacity(size.min(1024 * 1024) as usize);
    decoder.read_to_end(&mut data)?;
    Ok(PackedRawObject {
        type_code,
        base,
        data,
    })
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
        distance = ((distance + 1) << 7) | ((byte & 0x7f) as u64);
    }
    object_offset
        .checked_sub(distance)
        .ok_or_else(|| "invalid ofs-delta base offset".into())
}

fn apply_pack_delta(base: &[u8], delta: &[u8]) -> AnyResult<Vec<u8>> {
    let mut pos = 0usize;
    let source_size = read_delta_varint(delta, &mut pos)?;
    let target_size = read_delta_varint(delta, &mut pos)?;
    if source_size != base.len() {
        return Err(format!(
            "delta source size mismatch: expected {source_size}, got {}",
            base.len()
        )
        .into());
    }
    let mut out = Vec::with_capacity(target_size);
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
    let mut shift = 0usize;
    let mut out = 0usize;
    loop {
        let byte = read_delta_byte(data, pos)?;
        out |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok(out);
        }
        shift += 7;
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

fn parse_gix_since(since: Option<&str>) -> AnyResult<Option<i64>> {
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

fn format_gix_time(time: gix::date::Time) -> String {
    time.format_or_unix(gix::date::time::format::ISO8601_STRICT)
}

fn parse_git_log(out: &[u8]) -> AnyResult<Vec<Commit>> {
    let text = std::str::from_utf8(out)?;
    let mut commits = Vec::new();
    for raw_record in text.split('\x1e') {
        let raw_record = raw_record.trim();
        if raw_record.is_empty() {
            continue;
        }
        let mut lines = raw_record.lines();
        let Some(header) = lines.next() else {
            continue;
        };
        let mut fields = header.splitn(4, '\x1f');
        let hash = fields.next().ok_or("missing commit hash")?.to_string();
        let unix_time: i64 = fields
            .next()
            .ok_or("missing commit unix time")?
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let date = fields.next().ok_or("missing commit date")?.to_string();
        let subject = fields.next().unwrap_or_default().to_string();
        let mut seen = HashSet::default();
        let mut files = Vec::new();
        for line in lines {
            let file = normalize_git_path(line);
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
    let text = std::str::from_utf8(out)?;
    let max_files = if config.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        config.max_files_per_commit
    };
    let half_life = if config.half_life_days <= 0.0 {
        DEFAULT_HALF_LIFE_DAYS
    } else {
        config.half_life_days
    };
    let mut latest = None;
    let mut target_weight = 0.0;
    let mut pairs: HashMap<&str, DirectPairStat<'_>> =
        HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default());
    let mut other_files = Vec::with_capacity(max_files.min(DEFAULT_MAX_FILES));

    for raw_record in text.split('\x1e') {
        other_files.clear();
        let raw_record = raw_record.strip_prefix('\n').unwrap_or(raw_record);
        if raw_record.is_empty() {
            continue;
        }
        let mut lines = raw_record.lines();
        let Some(header) = lines.next() else {
            continue;
        };
        let (hash, unix_time_raw, date, subject) = if config.evidence_limit == 0 {
            let (unix_time_raw, date) = header
                .split_once('\x1f')
                .ok_or("missing compact commit header field")?;
            ("", unix_time_raw, date, "")
        } else {
            let mut fields = header.splitn(4, '\x1f');
            let hash = fields.next().ok_or("missing commit hash")?;
            let unix_time_raw = fields.next().ok_or("missing commit unix time")?;
            let date = fields.next().ok_or("missing commit date")?;
            let subject = fields.next().unwrap_or_default();
            (hash, unix_time_raw, date, subject)
        };
        let unix_time: i64 = unix_time_raw
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let mut file_count = 0usize;
        let mut has_target = false;

        for line in lines {
            if line.is_empty() {
                continue;
            }
            file_count += 1;
            if line == target {
                has_target = true;
            } else {
                other_files.push(line);
            }
            if file_count > max_files {
                break;
            }
        }
        if file_count == 0 || file_count > max_files || !has_target {
            continue;
        }

        let latest = *latest.get_or_insert(unix_time);
        let decay = time_decay(latest, unix_time, half_life);
        target_weight += decay;

        let pair_weight = decay / ((file_count + 1) as f64).log2();
        let mut evidence = None;
        for other in &other_files {
            let pair = pairs.entry(*other).or_default();
            pair.cochanges += 1;
            pair.weight += pair_weight;
            pair.other_weight += decay;
            if pair.last_seen.is_empty() || date > pair.last_seen {
                pair.last_seen = date;
            }
            if pair.evidence.len() < config.evidence_limit {
                let evidence = evidence.get_or_insert_with(|| Evidence {
                    hash: hash.to_string(),
                    date: date.to_string(),
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

fn build_graph_data(_repo_root: &str, commits: &[Commit], cfg: GraphBuildConfig) -> GraphData {
    let max_files = if cfg.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        cfg.max_files_per_commit
    };
    let half_life = if cfg.half_life_days <= 0.0 {
        DEFAULT_HALF_LIFE_DAYS
    } else {
        cfg.half_life_days
    };
    let latest = commits
        .iter()
        .map(|commit| commit.unix_time)
        .max()
        .unwrap_or(0);
    let mut files: HashMap<String, FileStat> = HashMap::default();
    let mut pair_map: HashMap<String, PairStat> = HashMap::default();

    for commit in commits {
        if commit.files.is_empty() {
            continue;
        }
        if commit.files.len() > max_files {
            continue;
        }
        let decay = time_decay(latest, commit.unix_time, half_life);
        for file in &commit.files {
            let stat = files.entry(file.clone()).or_default();
            stat.changes += 1;
            stat.weighted_changes += decay;
            if stat.last_seen.is_empty() || commit.date > stat.last_seen {
                stat.last_seen = commit.date.clone();
            }
        }
        if commit.files.len() < 2 {
            continue;
        }
        let pair_weight = decay / ((commit.files.len() + 1) as f64).log2();
        let evidence = Evidence {
            hash: commit.hash.clone(),
            date: commit.date.clone(),
            subject: commit.subject.clone(),
            file_count: commit.files.len(),
            weight: pair_weight,
        };
        for i in 0..commit.files.len() {
            for j in i + 1..commit.files.len() {
                let a = &commit.files[i];
                let b = &commit.files[j];
                let key = pair_key(a, b);
                let pair = pair_map.entry(key).or_insert_with(|| {
                    let (left, right) = ordered_pair(a, b);
                    PairStat {
                        a: left.to_string(),
                        b: right.to_string(),
                        cochanges: 0,
                        weight: 0.0,
                        last_seen: String::new(),
                        evidence: Vec::new(),
                    }
                });
                pair.cochanges += 1;
                pair.weight += pair_weight;
                if pair.last_seen.is_empty() || commit.date > pair.last_seen {
                    pair.last_seen = commit.date.clone();
                }
                if pair.evidence.len() < cfg.evidence_limit {
                    pair.evidence.push(evidence.clone());
                }
            }
        }
    }

    let mut pairs: Vec<PairStat> = pair_map.into_values().collect();
    pairs.sort_by(|left, right| left.a.cmp(&right.a).then(left.b.cmp(&right.b)));
    GraphData { files, pairs }
}

fn query_direct_from_commits(
    target: &str,
    commits: &[Commit],
    config: &OnDemandConfig,
    top: usize,
    evidence_limit: isize,
) -> Vec<ResultItem> {
    let max_files = if config.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        config.max_files_per_commit
    };
    let half_life = if config.half_life_days <= 0.0 {
        DEFAULT_HALF_LIFE_DAYS
    } else {
        config.half_life_days
    };
    let latest = commits
        .iter()
        .filter(|commit| {
            !commit.files.is_empty()
                && commit.files.len() <= max_files
                && commit.files.iter().any(|file| file == target)
        })
        .map(|commit| commit.unix_time)
        .max()
        .unwrap_or(0);
    let mut target_weight = 0.0;
    let mut pairs: HashMap<&str, DirectPairStat<'_>> =
        HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default());

    for commit in commits {
        if commit.files.is_empty() || commit.files.len() > max_files {
            continue;
        }
        if !commit.files.iter().any(|file| file == target) {
            continue;
        }
        let decay = time_decay(latest, commit.unix_time, half_life);
        target_weight += decay;
        let pair_weight = decay / ((commit.files.len() + 1) as f64).log2();
        let mut evidence = None;
        for other in &commit.files {
            if other == target {
                continue;
            }
            let pair = pairs.entry(other.as_str()).or_default();
            pair.cochanges += 1;
            pair.weight += pair_weight;
            pair.other_weight += decay;
            let date = commit.date.as_str();
            if pair.last_seen.is_empty() || date > pair.last_seen {
                pair.last_seen = date;
            }
            if pair.evidence.len() < config.evidence_limit {
                let evidence = evidence.get_or_insert_with(|| Evidence {
                    hash: commit.hash.clone(),
                    date: commit.date.clone(),
                    subject: commit.subject.clone(),
                    file_count: commit.files.len(),
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
    scored
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
        .collect()
}

impl<'a> RelatedGraph<'a> {
    fn new(data: &'a GraphData) -> Self {
        let mut pairs = HashMap::default();
        let mut adj: HashMap<String, HashMap<String, f64>> = HashMap::default();
        let mut degree: HashMap<String, f64> = HashMap::default();
        let mut paths: Vec<String> = data.files.keys().cloned().collect();
        paths.sort();

        for pair in &data.pairs {
            pairs.insert(pair_key(&pair.a, &pair.b), pair.clone());
            adj.entry(pair.a.clone())
                .or_default()
                .entry(pair.b.clone())
                .and_modify(|weight| *weight += pair.weight)
                .or_insert(pair.weight);
            adj.entry(pair.b.clone())
                .or_default()
                .entry(pair.a.clone())
                .and_modify(|weight| *weight += pair.weight)
                .or_insert(pair.weight);
            *degree.entry(pair.a.clone()).or_default() += pair.weight;
            *degree.entry(pair.b.clone()).or_default() += pair.weight;
        }

        Self {
            data,
            pairs,
            adj,
            degree,
            paths,
        }
    }

    fn query(
        &self,
        target: &str,
        mode: &str,
        top: usize,
        evidence_limit: isize,
    ) -> AnyResult<Vec<ResultItem>> {
        match mode {
            "direct" => Ok(self.query_direct(target, top, evidence_limit)),
            "pagerank" => Ok(self.query_pagerank(target, top, evidence_limit)),
            "path" => Ok(self.query_path(target, top)),
            "hot" => Ok(self.query_hot(target, top)),
            other => Err(format!("unknown mode {other:?}").into()),
        }
    }

    fn query_direct(&self, target: &str, top: usize, evidence_limit: isize) -> Vec<ResultItem> {
        let mut results = Vec::new();
        if let Some(neighbors) = self.adj.get(target) {
            for other in neighbors.keys() {
                if let Some(pair) = self.pairs.get(&pair_key(target, other)) {
                    let score = self.normalized_pair_score(pair);
                    results.push(pair_result(
                        pair,
                        other,
                        score,
                        "direct_cochange",
                        evidence_limit,
                    ));
                }
            }
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn query_pagerank(&self, target: &str, top: usize, evidence_limit: isize) -> Vec<ResultItem> {
        const ALPHA: f64 = 0.85;
        const ITERATIONS: usize = 30;

        let mut rank: HashMap<String, f64> = HashMap::default();
        rank.insert(target.to_string(), 1.0);
        for _ in 0..ITERATIONS {
            let mut next = HashMap::default();
            next.insert(target.to_string(), 1.0 - ALPHA);
            for (node, value) in &rank {
                if *value == 0.0 {
                    continue;
                }
                let degree = *self.degree.get(node).unwrap_or(&0.0);
                if degree == 0.0 {
                    *next.entry(target.to_string()).or_default() += ALPHA * value;
                    continue;
                }
                if let Some(neighbors) = self.adj.get(node) {
                    for (neighbor, weight) in neighbors {
                        *next.entry(neighbor.clone()).or_default() +=
                            ALPHA * value * weight / degree;
                    }
                }
            }
            rank = next;
        }

        let mut results = Vec::new();
        for (path, score) in rank {
            if path == target || score <= 0.0 {
                continue;
            }
            if let Some(pair) = self.pairs.get(&pair_key(target, &path)) {
                results.push(pair_result(
                    pair,
                    &path,
                    score,
                    "pagerank_direct_evidence",
                    evidence_limit,
                ));
            } else {
                results.push(ResultItem {
                    path,
                    score,
                    cochanges: 0,
                    weight: 0.0,
                    last_seen: String::new(),
                    reason: "pagerank_via_cochange_graph".to_string(),
                    evidence: Vec::new(),
                });
            }
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn query_path(&self, target: &str, top: usize) -> Vec<ResultItem> {
        let target_tokens = path_tokens(target);
        let mut results = Vec::new();
        for path in &self.paths {
            if path == target {
                continue;
            }
            let score = path_similarity(target, path, &target_tokens);
            if score <= 0.0 {
                continue;
            }
            results.push(ResultItem {
                path: path.clone(),
                score,
                cochanges: 0,
                weight: 0.0,
                last_seen: String::new(),
                reason: "path_name_baseline".to_string(),
                evidence: Vec::new(),
            });
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn query_hot(&self, target: &str, top: usize) -> Vec<ResultItem> {
        let mut results = Vec::new();
        for (path, stat) in &self.data.files {
            if path == target || stat.weighted_changes <= 0.0 {
                continue;
            }
            results.push(ResultItem {
                path: path.clone(),
                score: stat.weighted_changes,
                cochanges: 0,
                weight: stat.weighted_changes,
                last_seen: stat.last_seen.clone(),
                reason: "hot_file_baseline".to_string(),
                evidence: Vec::new(),
            });
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn normalized_pair_score(&self, pair: &PairStat) -> f64 {
        let a = self
            .data
            .files
            .get(&pair.a)
            .map(|stat| stat.weighted_changes)
            .unwrap_or_default();
        let b = self
            .data
            .files
            .get(&pair.b)
            .map(|stat| stat.weighted_changes)
            .unwrap_or_default();
        if a <= 0.0 || b <= 0.0 {
            pair.weight
        } else {
            pair.weight / (a * b).sqrt()
        }
    }

    fn resolve_path(&self, repo_root: &str, input: &str) -> AnyResult<String> {
        let path = normalize_input_path(repo_root, input);
        self.resolve_known_or_ambiguous_path(input, path, true)
    }

    fn resolve_path_or_tracked(&self, repo_root: &str, input: &str) -> AnyResult<String> {
        let path = normalize_input_path(repo_root, input);
        self.resolve_known_or_tracked_path(repo_root, input, path)
    }

    fn resolve_known_or_ambiguous_path(
        &self,
        input: &str,
        path: String,
        require_graph_presence: bool,
    ) -> AnyResult<String> {
        if path.is_empty() {
            return Err(format!("{input:?} is not a valid path").into());
        }
        match self.match_graph_path(path) {
            GraphPathMatch::Known(path) => Ok(path),
            GraphPathMatch::Missing(_) if require_graph_presence => {
                Err(format!("{input:?} is not present in the co-change graph").into())
            }
            GraphPathMatch::Missing(path) => Ok(path),
            GraphPathMatch::Ambiguous(matches) => Err(ambiguous_path_error(input, &matches).into()),
        }
    }

    fn resolve_known_or_tracked_path(
        &self,
        repo_root: &str,
        input: &str,
        path: String,
    ) -> AnyResult<String> {
        if path.is_empty() {
            return Err(format!("{input:?} is not a valid path").into());
        }
        match self.match_graph_path(path) {
            GraphPathMatch::Known(path) => Ok(path),
            GraphPathMatch::Missing(path) if git_path_is_tracked(repo_root, &path)? => Ok(path),
            GraphPathMatch::Missing(_) => Err(format!(
                "{input:?} is not tracked in the repository and is not present in the co-change graph"
            )
            .into()),
            GraphPathMatch::Ambiguous(matches) => Err(ambiguous_path_error(input, &matches).into()),
        }
    }

    fn match_graph_path<'b>(&'b self, path: String) -> GraphPathMatch<'b> {
        if self.data.files.contains_key(&path) {
            return GraphPathMatch::Known(path);
        }

        let suffix = format!("/{path}");
        let mut first_match = None;
        let mut ambiguous = Vec::new();
        for candidate in &self.paths {
            if candidate.as_str() != path
                && !candidate.ends_with(&suffix)
                && path_basename(candidate) != path
            {
                continue;
            }
            let candidate = candidate.as_str();
            if let Some(first) = first_match {
                if ambiguous.is_empty() {
                    ambiguous.push(first);
                }
                ambiguous.push(candidate);
            } else {
                first_match = Some(candidate);
            }
        }

        if !ambiguous.is_empty() {
            GraphPathMatch::Ambiguous(ambiguous)
        } else if let Some(path) = first_match {
            GraphPathMatch::Known(path.to_string())
        } else {
            GraphPathMatch::Missing(path)
        }
    }
}

fn ambiguous_path_error(input: &str, matches: &[&str]) -> String {
    format!("{input:?} is ambiguous: {}", matches.join(", "))
}

fn evaluate(
    graph: &RelatedGraph<'_>,
    test: &[Commit],
    modes: &[String],
    top: usize,
    max_files: usize,
) -> AnyResult<EvalReport> {
    let mut accs: HashMap<String, EvalAccumulator> = modes
        .iter()
        .map(|mode| {
            (
                mode.clone(),
                EvalAccumulator {
                    mode: mode.clone(),
                    tasks: 0,
                    hit_tasks: 0,
                    precision_sum: 0.0,
                    recall_sum: 0.0,
                    mrr_sum: 0.0,
                    results_sum: 0,
                },
            )
        })
        .collect();
    let mut report = EvalReport {
        repo_root: String::new(),
        train_commits: 0,
        test_commits: 0,
        top_k: top,
        max_files_per_commit: max_files,
        candidate_tasks: 0,
        evaluated_tasks: 0,
        skipped_unknown_seed: 0,
        skipped_no_known_target: 0,
        metrics: Vec::new(),
    };

    for commit in test {
        if commit.files.len() < 2 || commit.files.len() > max_files {
            continue;
        }
        let known_files: Vec<&String> = commit
            .files
            .iter()
            .filter(|file| graph.data.files.contains_key(*file))
            .collect();
        if known_files.len() < 2 {
            continue;
        }
        let known_set: HashSet<String> = known_files.iter().map(|file| (*file).clone()).collect();
        for seed in &commit.files {
            report.candidate_tasks += 1;
            if !known_set.contains(seed) {
                report.skipped_unknown_seed += 1;
                continue;
            }
            let targets: HashSet<String> = commit
                .files
                .iter()
                .filter(|target| *target != seed && known_set.contains(*target))
                .cloned()
                .collect();
            if targets.is_empty() {
                report.skipped_no_known_target += 1;
                continue;
            }
            report.evaluated_tasks += 1;
            for mode in modes {
                let results = graph.query(seed, mode, top, -1)?;
                accs.get_mut(mode)
                    .expect("mode accumulator")
                    .add(&results, &targets, top);
            }
        }
    }

    report.metrics = modes
        .iter()
        .filter_map(|mode| accs.remove(mode).map(|acc| acc.metrics()))
        .collect();
    report
        .metrics
        .sort_by(|left, right| left.mode.cmp(&right.mode));
    Ok(report)
}

impl EvalAccumulator {
    fn add(&mut self, results: &[ResultItem], targets: &HashSet<String>, top: usize) {
        self.tasks += 1;
        let limit = results.len().min(top);
        let mut hits = 0;
        let mut first_hit = 0;
        for (idx, result) in results.iter().take(limit).enumerate() {
            if targets.contains(&result.path) {
                hits += 1;
                if first_hit == 0 {
                    first_hit = idx + 1;
                }
            }
        }
        if hits > 0 {
            self.hit_tasks += 1;
            self.mrr_sum += 1.0 / first_hit as f64;
        }
        self.precision_sum += hits as f64 / top as f64;
        self.recall_sum += hits as f64 / targets.len() as f64;
        self.results_sum += results.len();
    }

    fn metrics(self) -> EvalMetrics {
        if self.tasks == 0 {
            return EvalMetrics {
                mode: self.mode,
                ..Default::default()
            };
        }
        EvalMetrics {
            mode: self.mode,
            tasks: self.tasks,
            hit_rate_at_k: self.hit_tasks as f64 / self.tasks as f64,
            precision_at_k: self.precision_sum / self.tasks as f64,
            recall_at_k: self.recall_sum / self.tasks as f64,
            mrr: self.mrr_sum / self.tasks as f64,
            avg_results: self.results_sum as f64 / self.tasks as f64,
        }
    }
}

fn pair_result(
    pair: &PairStat,
    path: &str,
    score: f64,
    reason: &str,
    evidence_limit: isize,
) -> ResultItem {
    let evidence = if evidence_limit >= 0 {
        pair.evidence
            .iter()
            .take(evidence_limit as usize)
            .cloned()
            .collect()
    } else {
        pair.evidence.clone()
    };
    ResultItem {
        path: path.to_string(),
        score,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen: pair.last_seen.clone(),
        reason: reason.to_string(),
        evidence,
    }
}

fn direct_pair_result(
    pair: DirectPairStat<'_>,
    path: &str,
    score: f64,
    reason: &str,
    evidence_limit: isize,
) -> ResultItem {
    let evidence = if evidence_limit >= 0 {
        pair.evidence
            .into_iter()
            .take(evidence_limit as usize)
            .collect()
    } else {
        pair.evidence
    };
    ResultItem {
        path: path.to_string(),
        score,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen: pair.last_seen.to_string(),
        reason: reason.to_string(),
        evidence,
    }
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

fn time_decay(latest: i64, when: i64, half_life_days: f64) -> f64 {
    if half_life_days <= 0.0 {
        return 1.0;
    }
    let age_days = ((latest - when).max(0) as f64) / 86_400.0;
    (-std::f64::consts::LN_2 * age_days / half_life_days).exp()
}

fn sort_results(results: &mut [ResultItem]) {
    results.sort_unstable_by(result_cmp);
}

fn truncate_top_results(results: &mut Vec<ResultItem>, top: usize) {
    if top == 0 {
        results.clear();
        return;
    }
    if results.len() > top {
        results.select_nth_unstable_by(top, result_cmp);
        results.truncate(top);
    }
    sort_results(results);
}

fn truncate_top_direct_pairs(results: &mut Vec<DirectScoredPair<'_>>, top: usize) {
    if top == 0 {
        results.clear();
        return;
    }
    if results.len() > top {
        results.select_nth_unstable_by(top, direct_scored_pair_cmp);
        results.truncate(top);
    }
    results.sort_unstable_by(direct_scored_pair_cmp);
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

fn result_cmp(left: &ResultItem, right: &ResultItem) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then(left.path.cmp(&right.path))
}

fn direct_pair_capacity(top: usize) -> usize {
    top.saturating_mul(32).clamp(64, 4096)
}

fn effective_max_files(config: &OnDemandConfig) -> usize {
    if config.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        config.max_files_per_commit
    }
}

fn direct_scored_pair_cmp(left: &DirectScoredPair<'_>, right: &DirectScoredPair<'_>) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then(left.path.cmp(right.path))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

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
    fn graph_query_explain_and_eval() {
        let repo = new_test_repo();
        write_commit(
            &repo,
            "initial auth pair",
            &[
                ("src/auth.ts", "auth v1\n"),
                ("tests/auth.test.ts", "test v1\n"),
            ],
        );
        write_commit(
            &repo,
            "second auth pair",
            &[
                ("src/auth.ts", "auth v2\n"),
                ("tests/auth.test.ts", "test v2\n"),
            ],
        );
        write_commit(
            &repo,
            "unrelated docs pair",
            &[("docs/api.md", "api\n"), ("openapi.yaml", "openapi\n")],
        );
        write_commit(
            &repo,
            "future auth change",
            &[
                ("src/auth.ts", "auth v3\n"),
                ("tests/auth.test.ts", "test v3\n"),
                ("docs/auth.md", "auth docs\n"),
            ],
        );

        let commits = git_log(repo.to_str().unwrap(), 20, None).unwrap();
        assert_eq!(commits.len(), 4);
        let config = OnDemandConfig {
            backend: OnDemandBackend::GitDiffTree,
            max_commits: 20,
            since: None,
            max_files_per_commit: 10,
            half_life_days: 365.0,
            evidence_limit: 3,
            jobs: 1,
            jobs_explicit: false,
            scan_commits: 0,
        };
        let seeds =
            git_target_commit_seeds(repo.to_str().unwrap(), "src/auth.ts", 20, None).unwrap();
        let selected_commits =
            git_diff_tree_selected_commits(repo.to_str().unwrap(), &seeds).unwrap();
        let input =
            git_target_commit_hash_input(repo.to_str().unwrap(), "src/auth.ts", 20, None).unwrap();
        let streamed_results = git_diff_tree_direct_from_hash_input(
            repo.to_str().unwrap(),
            &input,
            "src/auth.ts",
            &config,
            5,
        )
        .unwrap();
        let selected_results =
            query_direct_from_commits("src/auth.ts", &selected_commits, &config, 5, 3);
        assert_eq!(streamed_results.len(), selected_results.len());
        for (left, right) in streamed_results.iter().zip(selected_results.iter()) {
            assert_eq!(left.path, right.path);
            assert_eq!(left.cochanges, right.cochanges);
            assert!((left.score - right.score).abs() < 1e-12);
        }
        let log_results =
            git_log_direct_for_target(repo.to_str().unwrap(), "src/auth.ts", &config, 5).unwrap();
        assert_eq!(log_results.len(), selected_results.len());
        for (left, right) in log_results.iter().zip(selected_results.iter()) {
            assert_eq!(left.path, right.path);
            assert_eq!(left.cochanges, right.cochanges);
            assert!((left.score - right.score).abs() < 1e-12);
        }
        let mut compact_config = config.clone();
        compact_config.evidence_limit = 0;
        let compact_log_results =
            git_log_direct_for_target(repo.to_str().unwrap(), "src/auth.ts", &compact_config, 5)
                .unwrap();
        let compact_selected_results =
            query_direct_from_commits("src/auth.ts", &selected_commits, &compact_config, 5, 0);
        assert_eq!(compact_log_results.len(), compact_selected_results.len());
        for (left, right) in compact_log_results
            .iter()
            .zip(compact_selected_results.iter())
        {
            assert_eq!(left.path, right.path);
            assert_eq!(left.cochanges, right.cochanges);
            assert!((left.score - right.score).abs() < 1e-12);
            assert!(left.evidence.is_empty());
        }

        let data = build_graph_data(
            repo.to_str().unwrap(),
            &commits[1..],
            GraphBuildConfig {
                max_files_per_commit: config.max_files_per_commit,
                half_life_days: config.half_life_days,
                evidence_limit: config.evidence_limit,
            },
        );
        let graph = RelatedGraph::new(&data);
        let results = graph.query("src/auth.ts", "direct", 5, 3).unwrap();
        let fast_results = query_direct_from_commits("src/auth.ts", &commits[1..], &config, 5, 3);
        assert_eq!(
            results.iter().map(|item| &item.path).collect::<Vec<_>>(),
            fast_results
                .iter()
                .map(|item| &item.path)
                .collect::<Vec<_>>()
        );
        assert!(!results.is_empty());
        assert_eq!(results[0].path, "tests/auth.test.ts");
        assert_eq!(results[0].cochanges, 2);
        assert!(
            graph
                .pairs
                .contains_key(&pair_key("src/auth.ts", "tests/auth.test.ts"))
        );

        let report = evaluate(
            &graph,
            &commits[..1],
            &[
                "direct".to_string(),
                "pagerank".to_string(),
                "path".to_string(),
            ],
            3,
            10,
        )
        .unwrap();
        assert!(report.evaluated_tasks > 0);
        let direct = report
            .metrics
            .iter()
            .find(|metric| metric.mode == "direct")
            .unwrap();
        assert!(direct.hit_rate_at_k > 0.0);

        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn cli_on_demand_default_query() {
        let repo = new_test_repo();
        write_commit(&repo, "pair", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
        write_commit(&repo, "pair again", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
        let mut output = Vec::new();
        run_with_writer(
            vec![
                "query".to_string(),
                "a.md".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--history-backend".to_string(),
                "git".to_string(),
                "--max-commits".to_string(),
                "20".to_string(),
                "--mode".to_string(),
                "direct".to_string(),
            ],
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with("related a.md direct:on-demand:GitCli\n"));
        assert!(text.contains("1 b.md co=2\n"));

        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn cli_rejects_json_flag() {
        let mut output = Vec::new();
        let err = run_with_writer(
            vec![
                "query".to_string(),
                "a.md".to_string(),
                "--json".to_string(),
            ],
            &mut output,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown flag --json"));
    }

    #[test]
    fn cli_default_text_output_is_compact() {
        let repo = new_test_repo();
        write_commit(&repo, "pair", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
        write_commit(&repo, "pair again", &[("a.md", "a2\n"), ("b.md", "b2\n")]);

        let mut output = Vec::new();
        run_with_writer(
            vec![
                "query".to_string(),
                "a.md".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--history-backend".to_string(),
                "git".to_string(),
                "--max-commits".to_string(),
                "20".to_string(),
                "--mode".to_string(),
                "direct".to_string(),
            ],
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with("related a.md direct:on-demand:GitCli\n"));
        assert!(text.contains("1 b.md co=2\n"));
        assert!(!text.contains("seen="));
        assert!(!text.contains("cochanged="));
        assert!(!text.contains("direct_cochange"));

        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn cli_query_supports_exclude_patterns() {
        let repo = new_test_repo();
        write_commit(
            &repo,
            "pair with lockfile",
            &[
                ("a.md", "a1\n"),
                ("b.md", "b1\n"),
                ("Cargo.lock", "lock1\n"),
                ("package-lock.json", "package lock 1\n"),
                ("pnpm-lock.yaml", "pnpm lock 1\n"),
                ("bun.lockb", "bun lock 1\n"),
            ],
        );
        write_commit(
            &repo,
            "pair with lockfile again",
            &[
                ("a.md", "a2\n"),
                ("b.md", "b2\n"),
                ("Cargo.lock", "lock2\n"),
                ("package-lock.json", "package lock 2\n"),
                ("pnpm-lock.yaml", "pnpm lock 2\n"),
                ("bun.lockb", "bun lock 2\n"),
            ],
        );

        let mut output = Vec::new();
        run_with_writer(
            vec![
                "query".to_string(),
                "a.md".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--history-backend".to_string(),
                "git".to_string(),
                "--max-commits".to_string(),
                "20".to_string(),
                "--exclude".to_string(),
                BROAD_CHANGE_EXCLUDE_SUGGESTION.to_string(),
            ],
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("1 b.md co=2"));
        assert!(!text.contains("Cargo.lock"));
        assert!(!text.contains("package-lock.json"));
        assert!(!text.contains("pnpm-lock.yaml"));
        assert!(!text.contains("bun.lockb"));

        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn cli_explain_supports_text_and_unrelated_files() {
        let repo = new_test_repo();
        write_commit(&repo, "pair", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
        write_commit(&repo, "other pair", &[("c.md", "c1\n"), ("d.md", "d1\n")]);
        let config = OnDemandConfig {
            backend: OnDemandBackend::GitCli,
            max_commits: 20,
            since: None,
            max_files_per_commit: 10,
            half_life_days: 365.0,
            evidence_limit: 3,
            jobs: 1,
            jobs_explicit: false,
            scan_commits: 0,
        };

        let related =
            explain_relationship(repo.to_str().unwrap(), "a.md", "b.md", &config).unwrap();
        assert!(related.related);
        assert_eq!(related.a, "a.md");
        assert_eq!(related.b, "b.md");
        assert_eq!(related.cochanges, 1);
        assert_eq!(related.evidence[0].subject, "pair");

        let unrelated =
            explain_relationship(repo.to_str().unwrap(), "a.md", "c.md", &config).unwrap();
        assert!(!unrelated.related);
        assert_eq!(unrelated.a, "a.md");
        assert_eq!(unrelated.b, "c.md");
        assert_eq!(unrelated.cochanges, 0);

        let missing = explain_relationship(repo.to_str().unwrap(), "a.md", "missing.md", &config)
            .unwrap_err()
            .to_string();
        assert!(missing.contains("not tracked in the repository"));

        let mut output = Vec::new();
        run_with_writer(
            vec![
                "explain".to_string(),
                "a.md".to_string(),
                "b.md".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--history-backend".to_string(),
                "git".to_string(),
            ],
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("a.md <-> b.md"));
        assert!(text.contains("cochanged=1"));
        assert!(text.contains("files=2"));

        let mut output = Vec::new();
        run_with_writer(
            vec![
                "explain".to_string(),
                "a.md".to_string(),
                "c.md".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--history-backend".to_string(),
                "git".to_string(),
            ],
            &mut output,
        )
        .unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("a.md and c.md have no direct co-change evidence"));

        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn target_pathspec_is_literal() {
        let repo = new_test_repo();
        write_commit(
            &repo,
            "literal special path",
            &[
                ("src/literal[1].ts", "special\n"),
                ("tests/literal.test.ts", "test\n"),
            ],
        );
        write_commit(
            &repo,
            "similar glob match",
            &[("src/literal1.ts", "plain\n"), ("docs/plain.md", "plain\n")],
        );
        let config = OnDemandConfig {
            backend: OnDemandBackend::GitCli,
            max_commits: 20,
            since: None,
            max_files_per_commit: 10,
            half_life_days: 365.0,
            evidence_limit: 0,
            jobs: 1,
            jobs_explicit: false,
            scan_commits: 0,
        };

        let results =
            git_log_direct_for_target(repo.to_str().unwrap(), "src/literal[1].ts", &config, 5)
                .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|item| item.path.as_str())
                .collect::<Vec<_>>(),
            vec!["tests/literal.test.ts"]
        );

        fs::remove_dir_all(repo).ok();
    }

    #[test]
    fn pack_scan_backend_matches_git_direct_query() {
        let repo = new_test_repo();
        write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
        write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
        write_commit(&repo, "other", &[("c.md", "c\n"), ("d.md", "d\n")]);
        git(&repo, &["gc", "--quiet"]);

        let mut config = OnDemandConfig {
            backend: OnDemandBackend::PackScan,
            max_commits: 20,
            since: None,
            max_files_per_commit: 10,
            half_life_days: 365.0,
            evidence_limit: 0,
            jobs: 1,
            jobs_explicit: false,
            scan_commits: 0,
        };
        let pack_results =
            git_pack_scan_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
        config.backend = OnDemandBackend::GitCli;
        let git_results =
            git_log_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();

        assert_eq!(
            pack_results
                .iter()
                .map(|item| (item.path.as_str(), item.cochanges))
                .collect::<Vec<_>>(),
            git_results
                .iter()
                .map(|item| (item.path.as_str(), item.cochanges))
                .collect::<Vec<_>>()
        );

        config.backend = OnDemandBackend::PackScan;
        config.evidence_limit = 2;
        let pack_results =
            git_pack_scan_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
        config.backend = OnDemandBackend::GitCli;
        let git_results =
            git_log_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
        assert_eq!(
            pack_results[0].evidence.len(),
            git_results[0].evidence.len()
        );
        assert_eq!(pack_results[0].evidence[0].subject, "pair two");

        fs::remove_dir_all(repo).ok();
    }

    fn new_test_repo() -> PathBuf {
        let repo = temp_dir();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init"]);
        git(&repo, &["config", "user.email", "test@example.com"]);
        git(&repo, &["config", "user.name", "Test User"]);
        repo
    }

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("related-test-{}-{id}", std::process::id()))
    }

    fn write_commit(repo: &Path, message: &str, files: &[(&str, &str)]) {
        for (path, content) in files {
            let full = repo.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        git(repo, &["add", "."]);
        git(repo, &["commit", "-m", message]);
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed\nstdout={}\nstderr={}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
