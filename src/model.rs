use rustc_hash::FxHashMap as HashMap;
use serde::Serialize;

#[derive(Clone, Debug)]
pub(crate) struct Commit {
    pub(crate) hash: String,
    pub(crate) unix_time: i64,
    pub(crate) date: String,
    pub(crate) subject: String,
    pub(crate) files: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct HistoryRename {
    pub(crate) old_path: String,
    pub(crate) new_path: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RenameAwareCommit {
    pub(crate) commit: Commit,
    pub(crate) renames: Vec<HistoryRename>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Evidence {
    pub(crate) hash: String,
    pub(crate) date: String,
    pub(crate) subject: String,
    pub(crate) file_count: usize,
    pub(crate) weight: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FileStat {
    pub(crate) changes: usize,
    pub(crate) weighted_changes: f64,
    pub(crate) last_seen: String,
}

#[derive(Clone, Debug)]
pub(crate) struct PairStat {
    pub(crate) a: String,
    pub(crate) b: String,
    pub(crate) cochanges: usize,
    pub(crate) weight: f64,
    pub(crate) last_seen: String,
    pub(crate) evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DirectPairStat {
    pub(crate) cochanges: usize,
    pub(crate) weight: f64,
    pub(crate) other_weight: f64,
    pub(crate) last_seen: String,
    pub(crate) evidence: Vec<Evidence>,
}

pub(crate) struct DirectScoredPair {
    pub(crate) path: String,
    pub(crate) pair: DirectPairStat,
    pub(crate) score: f64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PackDirectPairStat {
    pub(crate) cochanges: usize,
    pub(crate) weight: f64,
    pub(crate) other_weight: f64,
    pub(crate) last_seen_time: Option<i64>,
    pub(crate) last_seen_offset: i32,
    pub(crate) evidence: Vec<Evidence>,
}

pub(crate) struct PackDirectScoredPair {
    pub(crate) path: String,
    pub(crate) pair: PackDirectPairStat,
    pub(crate) score: f64,
}

pub(crate) struct PackDirectPartial {
    pub(crate) target_weight: f64,
    pub(crate) pairs: HashMap<String, PackDirectPairStat>,
}

impl PackDirectPartial {
    pub(crate) fn new(top: usize) -> Self {
        Self {
            target_weight: 0.0,
            pairs: HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct GraphData {
    pub(crate) files: HashMap<String, FileStat>,
    pub(crate) pairs: Vec<PairStat>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphBuildConfig {
    pub(crate) max_files_per_commit: usize,
    pub(crate) half_life_days: f64,
    pub(crate) evidence_limit: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ResultItem {
    pub(crate) path: String,
    pub(crate) score: f64,
    pub(crate) cochanges: usize,
    pub(crate) weight: f64,
    pub(crate) last_seen: String,
    pub(crate) reason: String,
    pub(crate) evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct QueryOutput {
    pub(crate) schema_version: u32,
    pub(crate) target: String,
    pub(crate) mode: String,
    pub(crate) related: Vec<ResultItem>,
    pub(crate) hints: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Confidence {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AuditCandidate {
    pub(crate) path: String,
    pub(crate) score: f64,
    pub(crate) confidence: Confidence,
    pub(crate) support_count: usize,
    pub(crate) supported_by: Vec<String>,
    pub(crate) cochanges: usize,
    pub(crate) strongest_pair_cochanges: usize,
    pub(crate) weight: f64,
    pub(crate) last_seen: String,
    pub(crate) reason: String,
    pub(crate) evidence: Vec<Evidence>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct HistoryCoverage {
    pub(crate) backend: String,
    pub(crate) completeness: String,
    pub(crate) approximate: bool,
    pub(crate) rename_tracking: String,
    pub(crate) max_target_commits: usize,
    pub(crate) scan_commits: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AuditOutput {
    pub(crate) schema_version: u32,
    pub(crate) scope: String,
    pub(crate) seeds: Vec<String>,
    pub(crate) mode: String,
    pub(crate) minimum_confidence: Confidence,
    pub(crate) confidence_thresholds: ConfidenceThresholds,
    pub(crate) candidates: Vec<AuditCandidate>,
    pub(crate) abstained: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) enforcement: Option<AuditEnforcement>,
    pub(crate) history_coverage: HistoryCoverage,
    pub(crate) hints: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct ConfidenceThresholds {
    pub(crate) medium_min_strongest_pair_cochanges: usize,
    pub(crate) high_min_strongest_pair_cochanges: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AuditEnforcement {
    pub(crate) threshold: Confidence,
    pub(crate) finding_count: usize,
    pub(crate) triggered: bool,
    pub(crate) exit_code: i32,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ExplainOutput {
    pub(crate) schema_version: u32,
    pub(crate) a: String,
    pub(crate) b: String,
    pub(crate) related: bool,
    pub(crate) cochanges: usize,
    pub(crate) weight: f64,
    pub(crate) last_seen: String,
    pub(crate) evidence: Vec<Evidence>,
    pub(crate) hints: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct EvalReport {
    pub(crate) schema_version: u32,
    pub(crate) repo_root: String,
    pub(crate) query_shape: String,
    pub(crate) train_commits: usize,
    pub(crate) test_commits: usize,
    pub(crate) top_k: usize,
    pub(crate) max_files_per_commit: usize,
    pub(crate) candidate_tasks: usize,
    pub(crate) evaluated_tasks: usize,
    pub(crate) skipped_unknown_seed: usize,
    pub(crate) skipped_no_known_target: usize,
    pub(crate) metrics: Vec<EvalMetrics>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct AuditEvalReport {
    pub(crate) schema_version: u32,
    pub(crate) repo_root: String,
    pub(crate) query_shape: String,
    pub(crate) train_commits: usize,
    pub(crate) test_commits: usize,
    pub(crate) top_k: usize,
    pub(crate) max_files_per_commit: usize,
    pub(crate) minimum_confidence: Confidence,
    pub(crate) confidence_thresholds: ConfidenceThresholds,
    pub(crate) rename_tracking: String,
    pub(crate) training_renames: usize,
    pub(crate) test_diff_renames: usize,
    pub(crate) candidate_tasks: usize,
    pub(crate) evaluated_tasks: usize,
    pub(crate) skipped_unknown_targets: usize,
    pub(crate) skipped_insufficient_known_files: usize,
    pub(crate) metrics: Vec<AuditEvalMetrics>,
    pub(crate) confidence_metrics: Vec<AuditConfidenceMetrics>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct AuditConfidenceMetrics {
    pub(crate) mode: String,
    pub(crate) confidence: Confidence,
    pub(crate) candidates: usize,
    pub(crate) correct_candidates: usize,
    pub(crate) candidate_precision: f64,
    pub(crate) tasks_with_candidates: usize,
    pub(crate) tasks_with_correct_candidate: usize,
    pub(crate) task_coverage: f64,
    pub(crate) conditional_hit_rate: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct AuditEvalMetrics {
    pub(crate) mode: String,
    pub(crate) tasks: usize,
    pub(crate) hits_at_k: usize,
    pub(crate) hit_rate_at_k: f64,
    pub(crate) precision_at_k: f64,
    pub(crate) mrr: f64,
    pub(crate) avg_results: f64,
    pub(crate) avg_false_positives: f64,
    pub(crate) abstention_rate: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct EvalMetrics {
    pub(crate) mode: String,
    pub(crate) tasks: usize,
    pub(crate) hit_rate_at_k: f64,
    pub(crate) precision_at_k: f64,
    pub(crate) recall_at_k: f64,
    pub(crate) mrr: f64,
    pub(crate) avg_results: f64,
}

#[derive(Clone, Debug)]
pub(crate) struct EvalAccumulator {
    pub(crate) mode: String,
    pub(crate) tasks: usize,
    pub(crate) hit_tasks: usize,
    pub(crate) precision_sum: f64,
    pub(crate) recall_sum: f64,
    pub(crate) mrr_sum: f64,
    pub(crate) results_sum: usize,
}

pub(crate) struct RelatedGraph<'a> {
    pub(crate) data: &'a GraphData,
    pub(crate) pairs: HashMap<String, PairStat>,
    pub(crate) adj: HashMap<String, HashMap<String, f64>>,
    pub(crate) degree: HashMap<String, f64>,
    pub(crate) paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GraphPathMatch<'a> {
    Known(String),
    Missing(String),
    Ambiguous(Vec<&'a str>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OnDemandBackend {
    Hybrid,
    Gix,
    GitCli,
    GitBatch,
    GitBatchParallel,
    GitDiffTree,
    GitDiffTreeParallel,
    GitRevList,
    GitRemoveEmpty,
    PackScan,
    PackFast,
}

#[derive(Clone, Debug)]
pub(crate) struct GixCommitSeed {
    pub(crate) id: gix::hash::ObjectId,
    pub(crate) first_parent: Option<gix::hash::ObjectId>,
}

#[derive(Clone, Debug)]
pub(crate) struct OnDemandConfig {
    pub(crate) backend: OnDemandBackend,
    pub(crate) backend_explicit: bool,
    pub(crate) max_commits: usize,
    pub(crate) since: Option<String>,
    pub(crate) max_files_per_commit: usize,
    pub(crate) half_life_days: f64,
    pub(crate) evidence_limit: usize,
    pub(crate) jobs: usize,
    pub(crate) jobs_explicit: bool,
    pub(crate) scan_commits: usize,
}

pub(crate) fn direct_pair_capacity(top: usize) -> usize {
    top.saturating_mul(32).clamp(64, 4096)
}
