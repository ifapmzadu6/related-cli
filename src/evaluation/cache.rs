//! Shared training-window queries for query and omission evaluation.

use crate::AnyResult;
use crate::graph::RelatedGraph;
use crate::graph::{build_graph_data, query_direct_from_commits};
use crate::model::{Commit, GraphBuildConfig, OnDemandBackend, OnDemandConfig, ResultItem};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) struct TrainingQueries<'a> {
    train: &'a [Commit],
    modes: &'a [String],
    top: usize,
    graph_config: GraphBuildConfig,
    direct_config: OnDemandConfig,
    results: HashMap<String, HashMap<String, Vec<ResultItem>>>,
}

impl<'a> TrainingQueries<'a> {
    pub(super) fn new(
        train: &'a [Commit],
        modes: &'a [String],
        top: usize,
        graph_config: GraphBuildConfig,
    ) -> Self {
        Self {
            train,
            modes,
            top,
            graph_config,
            direct_config: OnDemandConfig {
                backend: OnDemandBackend::GitCli,
                backend_explicit: true,
                max_commits: train.len(),
                since: None,
                max_files_per_commit: graph_config.max_files_per_commit,
                half_life_days: graph_config.half_life_days,
                evidence_limit: 0,
                jobs: 1,
                jobs_explicit: false,
                scan_commits: 0,
            },
            results: HashMap::default(),
        }
    }

    pub(super) fn known_files(&self) -> HashSet<String> {
        self.train
            .iter()
            .filter(|commit| {
                !commit.files.is_empty()
                    && commit.files.len() <= self.graph_config.max_files_per_commit
            })
            .flat_map(|commit| commit.files.iter().cloned())
            .collect()
    }

    pub(super) fn query(&mut self, seed: &str, mode: &str) -> AnyResult<Vec<ResultItem>> {
        if !self.results.contains_key(seed) {
            let commits: Vec<Commit> = self
                .train
                .iter()
                .filter(|commit| commit.files.iter().any(|file| file == seed))
                .cloned()
                .collect();
            let mut results = HashMap::default();
            if self.modes.iter().any(|mode| mode == "direct") {
                results.insert(
                    "direct".to_string(),
                    query_direct_from_commits(seed, &commits, &self.direct_config, self.top, 0),
                );
            }
            if self.modes.iter().any(|mode| mode != "direct") {
                let data = build_graph_data("", &commits, self.graph_config);
                let graph = RelatedGraph::new(&data);
                for mode in self.modes.iter().filter(|mode| mode.as_str() != "direct") {
                    results.insert(mode.clone(), graph.query(seed, mode, self.top, 0)?);
                }
            }
            self.results.insert(seed.to_string(), results);
        }
        self.results
            .get(seed)
            .and_then(|results| results.get(mode))
            .cloned()
            .ok_or_else(|| format!("missing cached evaluation result for {seed:?}/{mode:?}").into())
    }
}
