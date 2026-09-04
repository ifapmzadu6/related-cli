use super::cache::TrainingQueries;
use crate::graph::RelatedGraph;
use crate::model::{Commit, EvalMetrics, EvalReport, GraphBuildConfig, ResultItem};
use crate::{AnyResult, JSON_SCHEMA_VERSION};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) fn evaluate_global(
    graph: &RelatedGraph<'_>,
    test: &[Commit],
    modes: &[String],
    top: usize,
    max_files: usize,
) -> AnyResult<EvalReport> {
    let known_files = graph.data.files.keys().cloned().collect();
    evaluate_with_query(
        "global",
        test,
        &known_files,
        modes,
        top,
        max_files,
        |seed, mode| graph.query(seed, mode, top, -1),
    )
}

pub(crate) fn evaluate_on_demand(
    train: &[Commit],
    test: &[Commit],
    modes: &[String],
    top: usize,
    graph_config: GraphBuildConfig,
) -> AnyResult<EvalReport> {
    let mut queries = TrainingQueries::new(train, modes, top, graph_config);
    let known_files = queries.known_files();

    evaluate_with_query(
        "on-demand",
        test,
        &known_files,
        modes,
        top,
        graph_config.max_files_per_commit,
        |seed, mode| queries.query(seed, mode),
    )
}

fn evaluate_with_query(
    query_shape: &str,
    test: &[Commit],
    known_files: &HashSet<String>,
    modes: &[String],
    top: usize,
    max_files: usize,
    mut query: impl FnMut(&str, &str) -> AnyResult<Vec<ResultItem>>,
) -> AnyResult<EvalReport> {
    let mut accs: HashMap<String, EvalAccumulator> = modes
        .iter()
        .map(|mode| {
            (
                mode.clone(),
                EvalAccumulator {
                    mode: mode.clone(),
                    tasks: 0,
                    hit_tasks: 0,
                    precision_sum: 0.0,
                    recall_sum: 0.0,
                    mrr_sum: 0.0,
                    results_sum: 0,
                },
            )
        })
        .collect();
    let mut report = EvalReport {
        schema_version: JSON_SCHEMA_VERSION,
        repo_root: String::new(),
        query_shape: query_shape.to_string(),
        train_commits: 0,
        test_commits: 0,
        top_k: top,
        max_files_per_commit: max_files,
        candidate_tasks: 0,
        evaluated_tasks: 0,
        skipped_unknown_seed: 0,
        skipped_no_known_target: 0,
        metrics: Vec::new(),
    };

    for commit in test {
        if commit.files.len() < 2 || commit.files.len() > max_files {
            continue;
        }
        let known_set: HashSet<String> = commit
            .files
            .iter()
            .filter(|file| known_files.contains(*file))
            .cloned()
            .collect();
        if known_set.len() < 2 {
            continue;
        }
        for seed in &commit.files {
            report.candidate_tasks += 1;
            if !known_set.contains(seed) {
                report.skipped_unknown_seed += 1;
                continue;
            }
            let targets: HashSet<String> = commit
                .files
                .iter()
                .filter(|target| *target != seed && known_set.contains(*target))
                .cloned()
                .collect();
            if targets.is_empty() {
                report.skipped_no_known_target += 1;
                continue;
            }
            report.evaluated_tasks += 1;
            for mode in modes {
                let results = query(seed, mode)?;
                accs.get_mut(mode)
                    .ok_or_else(|| format!("missing accumulator for mode {mode:?}"))?
                    .add(&results, &targets, top);
            }
        }
    }

    report.metrics = modes
        .iter()
        .filter_map(|mode| accs.remove(mode).map(EvalAccumulator::metrics))
        .collect();
    report
        .metrics
        .sort_by(|left, right| left.mode.cmp(&right.mode));
    Ok(report)
}

impl EvalAccumulator {
    fn add(&mut self, results: &[ResultItem], targets: &HashSet<String>, top: usize) {
        self.tasks += 1;
        let limit = results.len().min(top);
        let mut hits = 0;
        let mut first_hit = 0;
        for (idx, result) in results.iter().take(limit).enumerate() {
            if targets.contains(&result.path) {
                hits += 1;
                if first_hit == 0 {
                    first_hit = idx + 1;
                }
            }
        }
        if hits > 0 {
            self.hit_tasks += 1;
            self.mrr_sum += 1.0 / first_hit as f64;
        }
        self.precision_sum += hits as f64 / top as f64;
        self.recall_sum += hits as f64 / targets.len() as f64;
        self.results_sum += results.len();
    }

    fn metrics(self) -> EvalMetrics {
        if self.tasks == 0 {
            return EvalMetrics {
                mode: self.mode,
                ..Default::default()
            };
        }
        EvalMetrics {
            mode: self.mode,
            tasks: self.tasks,
            hit_rate_at_k: self.hit_tasks as f64 / self.tasks as f64,
            precision_at_k: self.precision_sum / self.tasks as f64,
            recall_at_k: self.recall_sum / self.tasks as f64,
            mrr: self.mrr_sum / self.tasks as f64,
            avg_results: self.results_sum as f64 / self.tasks as f64,
        }
    }
}

#[derive(Clone, Debug)]
struct EvalAccumulator {
    mode: String,
    tasks: usize,
    hit_tasks: usize,
    precision_sum: f64,
    recall_sum: f64,
    mrr_sum: f64,
    results_sum: usize,
}
