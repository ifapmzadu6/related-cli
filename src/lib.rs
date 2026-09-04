mod audit;
mod cli;
mod commands;
mod engine;
mod evaluation;
mod filters;
mod git_utils;
mod graph;
mod history;
mod model;
mod output;
mod pack;
mod path_utils;
mod ranking;
mod repo;
use std::error::Error;
use std::fmt;

const DEFAULT_MAX_FILES: usize = 80;
const DEFAULT_MAX_COMMITS: usize = 1000;
const DEFAULT_HALF_LIFE_DAYS: f64 = 365.0;
const DEFAULT_EVIDENCE: usize = 8;
const DEFAULT_TOP: usize = 20;
const DEFAULT_AUDIT_TOP: usize = 5;
const DEFAULT_ON_DEMAND_BACKEND: &str = "pack-fast";
const BROAD_CHANGE_EXCLUDE_SUGGESTION: &str = "*.lock,*-lock.*,*lockb,.github/workflows/*";
const JSON_SCHEMA_VERSION: u32 = 1;
const AUDIT_JSON_SCHEMA_VERSION: u32 = 2;
pub const EXIT_AUDIT_FINDINGS: i32 = 3;

type AnyError = Box<dyn Error>;
type AnyResult<T> = Result<T, AnyError>;

pub fn run(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    commands::run(args)
}

#[derive(Debug)]
pub struct AuditFindingsError {
    pub count: usize,
    pub threshold: String,
}

impl fmt::Display for AuditFindingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "audit found {} candidate(s) at or above {} confidence",
            self.count, self.threshold
        )
    }
}

impl Error for AuditFindingsError {}

pub fn exit_code_for_error(error: &(dyn Error + 'static)) -> i32 {
    if error.downcast_ref::<AuditFindingsError>().is_some() {
        EXIT_AUDIT_FINDINGS
    } else {
        1
    }
}

#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    pub fn parse_repository_bytes(data: &[u8]) {
        crate::history::fuzz_parse_bytes(data);
        crate::pack::fuzz_parse_bytes(data);
    }
}

#[cfg(test)]
mod tests;
