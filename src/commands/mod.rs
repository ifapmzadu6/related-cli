//! CLI dispatch. Parsing and presentation live in individual command modules.

mod audit;
mod diff;
mod eval;
mod explain;
mod help;
mod options;
mod query;
use crate::AnyResult;
use audit::cmd_audit;
use diff::cmd_diff;
#[cfg(test)]
pub(crate) use diff::merge_diff_result;
use eval::cmd_eval;
use explain::cmd_explain;
#[cfg(test)]
pub(crate) use explain::explain_relationship;
use help::{print_command_usage, print_usage};
use query::cmd_query;
use std::io::{self, Write};

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
        "audit" => cmd_audit(&args[1..], out),
        "diff" => cmd_diff(&args[1..], out),
        "eval" => cmd_eval(&args[1..], out),
        "version" | "-V" | "--version" => {
            writeln!(out, "related {}", env!("CARGO_PKG_VERSION"))?;
            Ok(())
        }
        "help" => print_command_usage(args.get(1).map(String::as_str), out),
        "-h" | "--help" => {
            print_usage(out)?;
            Ok(())
        }
        other => Err(format!("unknown command {other:?}").into()),
    }
}
