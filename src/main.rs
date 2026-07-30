mod cli;
mod commands;
mod evaluation;
mod filters;
mod git_utils;
mod graph;
mod history;
mod model;
mod output;
mod pack;
mod path_utils;
mod repo;

use commands::run;
use std::env;
use std::error::Error;

const DEFAULT_MAX_FILES: usize = 80;
const DEFAULT_MAX_COMMITS: usize = 1000;
const DEFAULT_HALF_LIFE_DAYS: f64 = 365.0;
const DEFAULT_EVIDENCE: usize = 8;
const DEFAULT_TOP: usize = 20;
const DEFAULT_ON_DEMAND_BACKEND: &str = "pack-fast";
const BROAD_CHANGE_EXCLUDE_SUGGESTION: &str = "*.lock,*-lock.*,*lockb,.github/workflows/*";

type AnyError = Box<dyn Error>;
type AnyResult<T> = Result<T, AnyError>;

fn main() {
    if let Err(err) = run(env::args().skip(1).collect()) {
        eprintln!("related: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
