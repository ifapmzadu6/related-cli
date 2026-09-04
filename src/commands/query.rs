use super::help::print_query_usage;
use super::options::parse_history_args;
use super::options::{parse_on_demand_config, validate_query_mode};
use crate::cli::{ParsedArgs, flag_bool, flag_positive_usize, flag_string};
use crate::engine::{configure_backend_for_repo, query_on_demand, with_default_pack_fallback};
use crate::filters::{
    filter_related_results, filtered_query_top, parse_exclude_patterns, query_hints,
};
use crate::git_utils::git_path_is_tracked;
use crate::model::QueryOutput;
use crate::output::{OutputFormat, parse_output_format, print_json, print_query};
use crate::path_utils::normalize_input_path;
use crate::repo::RepoContext;
use crate::{AnyResult, DEFAULT_TOP, JSON_SCHEMA_VERSION};
use std::io::Write;

pub(super) fn cmd_query<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_history_args(
        args,
        &["mode", "top", "exclude"],
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
