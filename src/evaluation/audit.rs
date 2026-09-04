use super::cache::TrainingQueries;
use crate::audit::audit_query_limit;
use crate::audit::{aggregate_audit_results, confidence_thresholds};
use crate::model::{AuditEvalMetrics, AuditEvalReport, Commit, Confidence, GraphBuildConfig};
use crate::{AUDIT_JSON_SCHEMA_VERSION, AnyResult};
use rustc_hash::FxHashMap as HashMap;

pub(crate) fn evaluate_audit_on_demand(
    train: &[Commit],
    test: &[Commit],
    modes: &[String],
    top: usize,
    graph_config: GraphBuildConfig,
    minimum_confidence: Confidence,
) -> AnyResult<AuditEvalReport> {
    let mut queries = TrainingQueries::new(train, modes, audit_query_limit(top), graph_config);
    let known_files = queries.known_files();
    let mut accumulators: HashMap<String, AuditAccumulator> = modes
        .iter()
        .map(|mode| (mode.clone(), AuditAccumulator::new(mode)))
        .collect();
    let mut report = AuditEvalReport {
        schema_version: AUDIT_JSON_SCHEMA_VERSION,
        repo_root: String::new(),
        query_shape: "on-demand-leave-one-out".to_string(),
        train_commits: 0,
        test_commits: 0,
        top_k: top,
        max_files_per_commit: graph_config.max_files_per_commit,
        minimum_confidence,
        confidence_thresholds: confidence_thresholds(),
        rename_tracking: "none".to_string(),
        training_renames: 0,
        test_diff_renames: 0,
        candidate_tasks: 0,
        evaluated_tasks: 0,
        skipped_unknown_targets: 0,
        skipped_insufficient_known_files: 0,
        metrics: Vec::new(),
        confidence_metrics: Vec::new(),
    };

    for commit in test {
        if commit.files.len() < 2 || commit.files.len() > graph_config.max_files_per_commit {
            continue;
        }
        let known: Vec<String> = commit
            .files
            .iter()
            .filter(|file| known_files.contains(*file))
            .cloned()
            .collect();
        report.candidate_tasks += commit.files.len();
        report.skipped_unknown_targets += commit.files.len().saturating_sub(known.len());
        if known.len() < 2 {
            report.skipped_insufficient_known_files += known.len();
            continue;
        }

        for omitted in &known {
            let seeds: Vec<String> = known
                .iter()
                .filter(|file| *file != omitted)
                .cloned()
                .collect();
            report.evaluated_tasks += 1;
            for mode in modes {
                let mut results_by_seed = Vec::with_capacity(seeds.len());
                for seed in &seeds {
                    let results = queries.query(seed, mode)?;
                    results_by_seed.push((seed.clone(), results));
                }
                let (all_candidates, _) =
                    aggregate_audit_results(&seeds, results_by_seed, Confidence::Low, top, 0);
                let candidates: Vec<_> = all_candidates
                    .iter()
                    .filter(|candidate| candidate.confidence >= minimum_confidence)
                    .cloned()
                    .collect();
                accumulators
                    .get_mut(mode)
                    .ok_or_else(|| format!("missing audit accumulator for {mode:?}"))?
                    .add(&candidates, &all_candidates, omitted, top);
            }
        }
    }

    for mode in modes {
        if let Some(accumulator) = accumulators.remove(mode) {
            let (metrics, mut confidence_metrics) = accumulator.finish();
            report.metrics.push(metrics);
            report.confidence_metrics.append(&mut confidence_metrics);
        }
    }
    report
        .metrics
        .sort_by(|left, right| left.mode.cmp(&right.mode));
    report.confidence_metrics.sort_by(|left, right| {
        left.mode
            .cmp(&right.mode)
            .then(left.confidence.cmp(&right.confidence))
    });
    Ok(report)
}

struct AuditAccumulator {
    mode: String,
    tasks: usize,
    hits: usize,
    precision_sum: f64,
    mrr_sum: f64,
    results_sum: usize,
    false_positives_sum: usize,
    abstentions: usize,
    confidence_candidates: [usize; 3],
    confidence_correct_candidates: [usize; 3],
    confidence_tasks: [usize; 3],
    confidence_correct_tasks: [usize; 3],
}

impl AuditAccumulator {
    fn new(mode: &str) -> Self {
        Self {
            mode: mode.to_string(),
            tasks: 0,
            hits: 0,
            precision_sum: 0.0,
            mrr_sum: 0.0,
            results_sum: 0,
            false_positives_sum: 0,
            abstentions: 0,
            confidence_candidates: [0; 3],
            confidence_correct_candidates: [0; 3],
            confidence_tasks: [0; 3],
            confidence_correct_tasks: [0; 3],
        }
    }

    fn add(
        &mut self,
        candidates: &[crate::model::AuditCandidate],
        all_candidates: &[crate::model::AuditCandidate],
        omitted: &str,
        top: usize,
    ) {
        self.tasks += 1;
        let mut confidence_seen = [false; 3];
        let mut confidence_correct = [false; 3];
        for candidate in all_candidates.iter().take(top) {
            let index = confidence_index(candidate.confidence);
            self.confidence_candidates[index] += 1;
            confidence_seen[index] = true;
            if candidate.path == omitted {
                self.confidence_correct_candidates[index] += 1;
                confidence_correct[index] = true;
            }
        }
        for index in 0..3 {
            self.confidence_tasks[index] += usize::from(confidence_seen[index]);
            self.confidence_correct_tasks[index] += usize::from(confidence_correct[index]);
        }
        self.results_sum += candidates.len();
        if candidates.is_empty() {
            self.abstentions += 1;
        }
        if let Some(index) = candidates
            .iter()
            .take(top)
            .position(|candidate| candidate.path == omitted)
        {
            self.hits += 1;
            self.false_positives_sum += candidates.len().saturating_sub(1);
            self.precision_sum += 1.0 / top as f64;
            self.mrr_sum += 1.0 / (index + 1) as f64;
        } else {
            self.false_positives_sum += candidates.len();
        }
    }

    fn finish(self) -> (AuditEvalMetrics, Vec<crate::model::AuditConfidenceMetrics>) {
        let confidence_metrics = [Confidence::Low, Confidence::Medium, Confidence::High]
            .into_iter()
            .enumerate()
            .map(|(index, confidence)| {
                let candidates = self.confidence_candidates[index];
                let tasks_with_candidates = self.confidence_tasks[index];
                crate::model::AuditConfidenceMetrics {
                    mode: self.mode.clone(),
                    confidence,
                    candidates,
                    correct_candidates: self.confidence_correct_candidates[index],
                    candidate_precision: divide(
                        self.confidence_correct_candidates[index],
                        candidates,
                    ),
                    tasks_with_candidates,
                    tasks_with_correct_candidate: self.confidence_correct_tasks[index],
                    task_coverage: divide(tasks_with_candidates, self.tasks),
                    conditional_hit_rate: divide(
                        self.confidence_correct_tasks[index],
                        tasks_with_candidates,
                    ),
                }
            })
            .collect();
        if self.tasks == 0 {
            return (
                AuditEvalMetrics {
                    mode: self.mode,
                    ..Default::default()
                },
                confidence_metrics,
            );
        }
        (
            AuditEvalMetrics {
                mode: self.mode,
                tasks: self.tasks,
                hits_at_k: self.hits,
                hit_rate_at_k: self.hits as f64 / self.tasks as f64,
                precision_at_k: self.precision_sum / self.tasks as f64,
                mrr: self.mrr_sum / self.tasks as f64,
                avg_results: self.results_sum as f64 / self.tasks as f64,
                avg_false_positives: self.false_positives_sum as f64 / self.tasks as f64,
                abstention_rate: self.abstentions as f64 / self.tasks as f64,
            },
            confidence_metrics,
        )
    }
}

fn confidence_index(confidence: Confidence) -> usize {
    match confidence {
        Confidence::Low => 0,
        Confidence::Medium => 1,
        Confidence::High => 2,
    }
}

fn divide(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}
