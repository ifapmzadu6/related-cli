//! Chronological evaluation with a shared, training-only query cache.

mod audit;
mod cache;
mod history;
mod query;

pub(crate) use audit::evaluate_audit_on_demand;
pub(crate) use history::prepare_rename_aware_audit_history;
pub(crate) use query::{evaluate_global, evaluate_on_demand};
