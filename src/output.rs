use crate::model::{EvalReport, QueryOutput};
use std::borrow::Cow;
use std::io::{self, Write};

pub(crate) fn print_query<W: Write>(out: &mut W, output: &QueryOutput) -> io::Result<()> {
    writeln!(
        out,
        "related {} {}",
        escape_text(&output.target),
        escape_text(&output.mode)
    )?;
    if output.related.is_empty() {
        writeln!(out, "no related files found")?;
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
    use super::escape_text;

    #[test]
    fn escapes_control_characters_without_escaping_unicode() {
        assert_eq!(escape_text("café.md"), "café.md");
        assert_eq!(escape_text("line\n\u{1b}[31m"), "line\\n\\u{1b}[31m");
    }
}
