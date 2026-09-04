use super::help::print_audit_usage;
use super::options::parse_history_args;
use super::options::{parse_confidence, parse_on_demand_config};
use crate::audit::{aggregate_audit_results, audit_query_limit, confidence_thresholds};
use crate::cli::{flag_bool, flag_optional_string, flag_positive_usize, flag_string};
use crate::engine::{
    configure_backend_for_repo, history_coverage, query_from_commits, query_on_demand,
    with_default_pack_fallback,
};
use crate::filters::{broad_change_hints, parse_exclude_patterns, path_matches_any_pattern};
use crate::git_utils::{
    git_audit_candidate_paths, git_diff_audit_paths, git_diff_audit_paths_for_range,
    git_worktree_audit_paths,
};
use crate::history::git_followed_commits_for_targets;
use crate::model::{AuditEnforcement, AuditOutput, OnDemandBackend};
use crate::output::{OutputFormat, confidence_name, parse_output_format, print_audit, print_json};
use crate::repo::RepoContext;
use crate::{
    AUDIT_JSON_SCHEMA_VERSION, AnyResult, AuditFindingsError, DEFAULT_AUDIT_TOP,
    EXIT_AUDIT_FINDINGS,
};
use rustc_hash::FxHashSet as HashSet;
use std::io::Write;

pub(super) fn cmd_audit<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_history_args(
        args,
        &[
            "range",
            "mode",
            "top",
            "min-confidence",
            "fail-on-confidence",
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

    let per_seed_top = audit_query_limit(top);
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
