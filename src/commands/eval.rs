use super::help::print_eval_usage;
use super::options::{parse_confidence, validate_query_mode};
use crate::cli::{
    flag_bool, flag_positive_f64, flag_positive_usize, flag_string, parse_args, parse_modes,
};
use crate::evaluation::{
    evaluate_audit_on_demand, evaluate_global, evaluate_on_demand,
    prepare_rename_aware_audit_history,
};
use crate::graph::RelatedGraph;
use crate::graph::build_graph_data;
use crate::history::{git_log, git_log_rename_aware};
use crate::model::GraphBuildConfig;
use crate::output::{OutputFormat, parse_output_format, print_audit_eval, print_eval, print_json};
use crate::repo::RepoContext;
use crate::{AnyResult, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_FILES};
use std::io::Write;

pub(super) fn cmd_eval<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
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
