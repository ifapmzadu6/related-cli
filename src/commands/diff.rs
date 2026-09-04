use super::help::print_diff_usage;
use super::options::parse_history_args;
use super::options::{parse_on_demand_config, validate_query_mode};
use crate::cli::{flag_bool, flag_positive_usize, flag_string};
use crate::engine::{configure_backend_for_repo, query_on_demand, with_default_pack_fallback};
use crate::filters::{
    filter_related_results, filtered_query_top, parse_exclude_patterns, query_hints,
};
use crate::git_utils::git_diff_names;
use crate::model::{QueryOutput, ResultItem};
use crate::output::{OutputFormat, parse_output_format, print_json, print_query};
use crate::repo::RepoContext;
use crate::{AnyResult, DEFAULT_TOP, JSON_SCHEMA_VERSION};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::io::Write;

pub(super) fn cmd_diff<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_history_args(args, &["mode", "top", "exclude"], &["staged", "help", "h"])?;
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
