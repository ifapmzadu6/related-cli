use crate::{
    AnyResult, DEFAULT_AUDIT_TOP, DEFAULT_EVIDENCE, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_COMMITS,
    DEFAULT_MAX_FILES, DEFAULT_ON_DEMAND_BACKEND, DEFAULT_TOP,
};
use std::io::Write;

pub(super) fn print_usage<W: Write>(out: &mut W) -> AnyResult<()> {
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

pub(super) fn print_command_usage<W: Write>(command: Option<&str>, out: &mut W) -> AnyResult<()> {
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

pub(super) fn print_query_usage<W: Write>(out: &mut W) -> AnyResult<()> {
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

pub(super) fn print_explain_usage<W: Write>(out: &mut W) -> AnyResult<()> {
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

pub(super) fn print_audit_usage<W: Write>(out: &mut W) -> AnyResult<()> {
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

pub(super) fn print_diff_usage<W: Write>(out: &mut W) -> AnyResult<()> {
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

pub(super) fn print_eval_usage<W: Write>(out: &mut W) -> AnyResult<()> {
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
