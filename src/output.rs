use crate::AnyResult;
use crate::model::{AuditEvalReport, AuditOutput, Confidence, EvalReport, QueryOutput};
use serde::Serialize;
use std::borrow::Cow;
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputFormat {
    Text,
    Json,
}

pub(crate) fn parse_output_format(value: &str) -> AnyResult<OutputFormat> {
    match value {
        "text" => Ok(OutputFormat::Text),
        "json" => Ok(OutputFormat::Json),
        other => Err(format!("unknown output format {other:?}; use text or json").into()),
    }
}

pub(crate) fn print_json<W: Write, T: Serialize>(out: &mut W, value: &T) -> AnyResult<()> {
    serde_json::to_writer(&mut *out, value)?;
    writeln!(out)?;
    Ok(())
}

pub(crate) fn print_query<W: Write>(out: &mut W, output: &QueryOutput) -> io::Result<()> {
    writeln!(
        out,
        "related {} {}",
        escape_text(&output.target),
        escape_text(&output.mode)
    )?;
    if output.related.is_empty() {
        writeln!(out, "no related files found")?;
        for hint in &output.hints {
            writeln!(out, "hint: {hint}")?;
        }
        return Ok(());
    }
    for (idx, item) in output.related.iter().enumerate() {
        write!(out, "{} {}", idx + 1, escape_text(&item.path))?;
        if item.cochanges > 0 {
            write!(out, " co={}", item.cochanges)?;
        }
        if item.cochanges == 0 || item.reason != "direct_cochange" {
            write!(out, " s={:.3}", item.score)?;
        }
        if !item.evidence.is_empty() {
            if !item.last_seen.is_empty() {
                write!(out, " seen={}", item.last_seen)?;
            }
            if item.weight > 0.0 {
                write!(out, " w={:.3}", item.weight)?;
            }
        }
        if let Some(reason) = compact_reason(&item.reason) {
            write!(out, " via={reason}")?;
        }
        writeln!(out)?;
        for ev in &item.evidence {
            writeln!(
                out,
                "  - {} {} files={} weight={:.6} {}",
                short_hash(&ev.hash),
                ev.date,
                ev.file_count,
                ev.weight,
                escape_text(&ev.subject)
            )?;
        }
    }
    for hint in &output.hints {
        writeln!(out, "hint: {hint}")?;
    }
    Ok(())
}

pub(crate) fn print_audit<W: Write>(out: &mut W, output: &AuditOutput) -> io::Result<()> {
    writeln!(
        out,
        "audit scope={} mode={} seeds={} minimum_confidence={} backend={} completeness={}",
        escape_text(&output.scope),
        escape_text(&output.mode),
        output.seeds.len(),
        confidence_name(output.minimum_confidence),
        escape_text(&output.history_coverage.backend),
        escape_text(&output.history_coverage.completeness),
    )?;
    writeln!(
        out,
        "changed {}",
        output
            .seeds
            .iter()
            .map(|seed| escape_text(seed))
            .collect::<Vec<_>>()
            .join(",")
    )?;
    if output.abstained {
        writeln!(out, "no candidates met the confidence threshold")?;
    } else {
        for (idx, candidate) in output.candidates.iter().enumerate() {
            writeln!(
                out,
                "{} {} confidence={} support={}/{} co={} strongest_co={} s={:.3}",
                idx + 1,
                escape_text(&candidate.path),
                confidence_name(candidate.confidence),
                candidate.support_count,
                output.seeds.len(),
                candidate.cochanges,
                candidate.strongest_pair_cochanges,
                candidate.score,
            )?;
            writeln!(
                out,
                "  supported_by {}",
                candidate
                    .supported_by
                    .iter()
                    .map(|seed| escape_text(seed))
                    .collect::<Vec<_>>()
                    .join(",")
            )?;
            for ev in &candidate.evidence {
                writeln!(
                    out,
                    "  - {} {} files={} weight={:.6} {}",
                    short_hash(&ev.hash),
                    ev.date,
                    ev.file_count,
                    ev.weight,
                    escape_text(&ev.subject)
                )?;
            }
        }
    }
    for hint in &output.hints {
        writeln!(out, "hint: {hint}")?;
    }
    Ok(())
}

fn confidence_name(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

fn compact_reason(reason: &str) -> Option<&str> {
    match reason {
        "direct_cochange" => None,
        "pagerank_direct_evidence" => Some("ppr-direct"),
        "pagerank_via_cochange_graph" => Some("ppr"),
        "path_name_baseline" => Some("path"),
        "hot_file_baseline" => Some("hot"),
        other => Some(other),
    }
}

pub(crate) fn print_eval<W: Write>(out: &mut W, report: &EvalReport) -> io::Result<()> {
    writeln!(out, "repo: {}", escape_text(&report.repo_root))?;
    writeln!(
        out,
        "query_shape={} train_commits={} test_commits={} top_k={} max_files_per_commit={}",
        report.query_shape,
        report.train_commits,
        report.test_commits,
        report.top_k,
        report.max_files_per_commit
    )?;
    writeln!(
        out,
        "candidate_tasks={} evaluated_tasks={} skipped_unknown_seed={} skipped_no_known_target={}",
        report.candidate_tasks,
        report.evaluated_tasks,
        report.skipped_unknown_seed,
        report.skipped_no_known_target
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{:<10} {:>8} {:>10} {:>12} {:>10} {:>10} {:>11}",
        "mode", "tasks", "hit@k", "precision@k", "recall@k", "mrr", "avg_results"
    )?;
    for metric in &report.metrics {
        writeln!(
            out,
            "{:<10} {:>8} {:>10.4} {:>12.4} {:>10.4} {:>10.4} {:>11.2}",
            metric.mode,
            metric.tasks,
            metric.hit_rate_at_k,
            metric.precision_at_k,
            metric.recall_at_k,
            metric.mrr,
            metric.avg_results
        )?;
    }
    Ok(())
}

pub(crate) fn print_audit_eval<W: Write>(out: &mut W, report: &AuditEvalReport) -> io::Result<()> {
    writeln!(out, "repo: {}", escape_text(&report.repo_root))?;
    writeln!(
        out,
        "task=audit query_shape={} train_commits={} test_commits={} top_k={} max_files_per_commit={} minimum_confidence={}",
        report.query_shape,
        report.train_commits,
        report.test_commits,
        report.top_k,
        report.max_files_per_commit,
        confidence_name(report.minimum_confidence),
    )?;
    writeln!(
        out,
        "candidate_tasks={} evaluated_tasks={} skipped_unknown_targets={} skipped_insufficient_known_files={}",
        report.candidate_tasks,
        report.evaluated_tasks,
        report.skipped_unknown_targets,
        report.skipped_insufficient_known_files
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{:<10} {:>8} {:>10} {:>12} {:>10} {:>11} {:>10} {:>12}",
        "mode", "tasks", "hit@k", "precision@k", "mrr", "avg_results", "avg_false", "abstention"
    )?;
    for metric in &report.metrics {
        writeln!(
            out,
            "{:<10} {:>8} {:>10.4} {:>12.4} {:>10.4} {:>11.2} {:>10.2} {:>12.4}",
            metric.mode,
            metric.tasks,
            metric.hit_rate_at_k,
            metric.precision_at_k,
            metric.mrr,
            metric.avg_results,
            metric.avg_false_positives,
            metric.abstention_rate,
        )?;
    }
    Ok(())
}

pub(crate) fn short_hash(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

pub(crate) fn escape_text(value: &str) -> Cow<'_, str> {
    if !value.chars().any(char::is_control) {
        return Cow::Borrowed(value);
    }
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use super::{escape_text, print_query};
    use crate::model::QueryOutput;

    #[test]
    fn escapes_control_characters_without_escaping_unicode() {
        assert_eq!(escape_text("café.md"), "café.md");
        assert_eq!(escape_text("line\n\u{1b}[31m"), "line\\n\\u{1b}[31m");
    }

    #[test]
    fn empty_query_output_keeps_hints() {
        let output = QueryOutput {
            schema_version: crate::JSON_SCHEMA_VERSION,
            target: "a.md".to_string(),
            mode: "direct".to_string(),
            related: Vec::new(),
            hints: vec!["used fallback".to_string()],
        };
        let mut text = Vec::new();
        print_query(&mut text, &output).unwrap();
        let text = String::from_utf8(text).unwrap();
        assert!(text.contains("no related files found\nhint: used fallback\n"));
    }
}
