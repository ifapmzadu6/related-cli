use super::help::print_explain_usage;
use super::options::parse_history_args;
use super::options::parse_on_demand_config;
use crate::cli::{flag_bool, flag_string};
use crate::engine::{
    build_on_demand_graph_data, configure_backend_for_repo, with_default_pack_fallback,
};
use crate::graph::RelatedGraph;
use crate::model::{ExplainOutput, OnDemandConfig};
use crate::output::{OutputFormat, escape_text, parse_output_format, print_json, short_hash};
use crate::path_utils::normalize_input_path;
use crate::repo::RepoContext;
use crate::{AnyResult, DEFAULT_EVIDENCE, JSON_SCHEMA_VERSION};
use std::io::Write;
use std::path::Path;

pub(super) fn cmd_explain<W: Write>(args: &[String], out: &mut W) -> AnyResult<()> {
    let parsed = parse_history_args(args, &[], &["help", "h"])?;
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
    let Some(pair) = graph.pair(&a, &b) else {
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
