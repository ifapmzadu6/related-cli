use crate::git_utils::git_path_is_tracked;
use crate::model::{
    Commit, DirectPairStat, DirectScoredPair, Evidence, FileStat, GraphBuildConfig, GraphData,
    OnDemandConfig, PairStat, ResultItem, direct_pair_capacity,
};
use crate::path_utils::{
    normalize_input_path, ordered_pair, pair_key, path_basename, path_similarity, path_tokens,
};
use crate::ranking::truncate_top_by;
use crate::{AnyResult, DEFAULT_HALF_LIFE_DAYS, DEFAULT_MAX_FILES};
use rustc_hash::FxHashMap as HashMap;
use std::cmp::Ordering;
use std::path::Path;

pub(crate) fn build_graph_data(
    _repo_root: &str,
    commits: &[Commit],
    cfg: GraphBuildConfig,
) -> GraphData {
    let max_files = if cfg.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        cfg.max_files_per_commit
    };
    let half_life = if cfg.half_life_days <= 0.0 {
        DEFAULT_HALF_LIFE_DAYS
    } else {
        cfg.half_life_days
    };
    let latest = commits
        .iter()
        .map(|commit| commit.unix_time)
        .max()
        .unwrap_or(0);
    let mut files: HashMap<String, FileStat> = HashMap::default();
    let mut pair_map: HashMap<String, PairStat> = HashMap::default();

    for commit in commits {
        if commit.files.is_empty() {
            continue;
        }
        if commit.files.len() > max_files {
            continue;
        }
        let decay = time_decay(latest, commit.unix_time, half_life);
        for file in &commit.files {
            let stat = files.entry(file.clone()).or_default();
            stat.changes += 1;
            stat.weighted_changes += decay;
            if stat.last_seen.is_empty() || commit.date > stat.last_seen {
                stat.last_seen = commit.date.clone();
            }
        }
        if commit.files.len() < 2 {
            continue;
        }
        let pair_weight = decay / ((commit.files.len() + 1) as f64).log2();
        let evidence = Evidence {
            hash: commit.hash.clone(),
            date: commit.date.clone(),
            subject: commit.subject.clone(),
            file_count: commit.files.len(),
            weight: pair_weight,
        };
        for i in 0..commit.files.len() {
            for j in i + 1..commit.files.len() {
                let a = &commit.files[i];
                let b = &commit.files[j];
                let key = pair_key(a, b);
                let pair = pair_map.entry(key).or_insert_with(|| {
                    let (left, right) = ordered_pair(a, b);
                    PairStat {
                        a: left.to_string(),
                        b: right.to_string(),
                        cochanges: 0,
                        weight: 0.0,
                        last_seen: String::new(),
                        evidence: Vec::new(),
                    }
                });
                pair.cochanges += 1;
                pair.weight += pair_weight;
                if pair.last_seen.is_empty() || commit.date > pair.last_seen {
                    pair.last_seen = commit.date.clone();
                }
                if pair.evidence.len() < cfg.evidence_limit {
                    pair.evidence.push(evidence.clone());
                }
            }
        }
    }

    let mut pairs: Vec<PairStat> = pair_map.into_values().collect();
    pairs.sort_by(|left, right| left.a.cmp(&right.a).then(left.b.cmp(&right.b)));
    GraphData { files, pairs }
}

pub(crate) fn query_direct_from_commits(
    target: &str,
    commits: &[Commit],
    config: &OnDemandConfig,
    top: usize,
    evidence_limit: isize,
) -> Vec<ResultItem> {
    let max_files = if config.max_files_per_commit == 0 {
        DEFAULT_MAX_FILES
    } else {
        config.max_files_per_commit
    };
    let half_life = if config.half_life_days <= 0.0 {
        DEFAULT_HALF_LIFE_DAYS
    } else {
        config.half_life_days
    };
    let latest = commits
        .iter()
        .filter(|commit| {
            !commit.files.is_empty()
                && commit.files.len() <= max_files
                && commit.files.iter().any(|file| file == target)
        })
        .map(|commit| commit.unix_time)
        .max()
        .unwrap_or(0);
    let mut target_weight = 0.0;
    let mut pairs: HashMap<String, DirectPairStat> =
        HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default());

    for commit in commits {
        if commit.files.is_empty() || commit.files.len() > max_files {
            continue;
        }
        if !commit.files.iter().any(|file| file == target) {
            continue;
        }
        let decay = time_decay(latest, commit.unix_time, half_life);
        target_weight += decay;
        let pair_weight = decay / ((commit.files.len() + 1) as f64).log2();
        let mut evidence = None;
        for other in &commit.files {
            if other == target {
                continue;
            }
            let pair = pairs.entry(other.clone()).or_default();
            pair.cochanges += 1;
            pair.weight += pair_weight;
            pair.other_weight += decay;
            let date = commit.date.as_str();
            if pair.last_seen.is_empty() || date > pair.last_seen.as_str() {
                pair.last_seen = date.to_string();
            }
            if pair.evidence.len() < config.evidence_limit {
                let evidence = evidence.get_or_insert_with(|| Evidence {
                    hash: commit.hash.clone(),
                    date: commit.date.clone(),
                    subject: commit.subject.clone(),
                    file_count: commit.files.len(),
                    weight: pair_weight,
                });
                pair.evidence.push(evidence.clone());
            }
        }
    }

    let mut scored = Vec::with_capacity(pairs.len());
    for (path, pair) in pairs {
        let score = if target_weight <= 0.0 || pair.other_weight <= 0.0 {
            pair.weight
        } else {
            pair.weight / (target_weight * pair.other_weight).sqrt()
        };
        scored.push(DirectScoredPair { path, pair, score });
    }
    truncate_top_direct_pairs(&mut scored, top);
    scored
        .into_iter()
        .map(|item| {
            direct_pair_result(
                item.pair,
                item.path,
                item.score,
                "direct_cochange",
                evidence_limit,
            )
        })
        .collect()
}

impl<'a> RelatedGraph<'a> {
    pub(crate) fn pair(&self, a: &str, b: &str) -> Option<&PairStat> {
        self.pairs.get(&pair_key(a, b))
    }

    pub(crate) fn new(data: &'a GraphData) -> Self {
        let mut pairs = HashMap::default();
        let mut adj: HashMap<String, HashMap<String, f64>> = HashMap::default();
        let mut degree: HashMap<String, f64> = HashMap::default();
        let mut paths: Vec<String> = data.files.keys().cloned().collect();
        paths.sort();

        for pair in &data.pairs {
            pairs.insert(pair_key(&pair.a, &pair.b), pair.clone());
            adj.entry(pair.a.clone())
                .or_default()
                .entry(pair.b.clone())
                .and_modify(|weight| *weight += pair.weight)
                .or_insert(pair.weight);
            adj.entry(pair.b.clone())
                .or_default()
                .entry(pair.a.clone())
                .and_modify(|weight| *weight += pair.weight)
                .or_insert(pair.weight);
            *degree.entry(pair.a.clone()).or_default() += pair.weight;
            *degree.entry(pair.b.clone()).or_default() += pair.weight;
        }

        Self {
            data,
            pairs,
            adj,
            degree,
            paths,
        }
    }

    pub(crate) fn query(
        &self,
        target: &str,
        mode: &str,
        top: usize,
        evidence_limit: isize,
    ) -> AnyResult<Vec<ResultItem>> {
        match mode {
            "direct" => Ok(self.query_direct(target, top, evidence_limit)),
            "pagerank" => Ok(self.query_pagerank(target, top, evidence_limit)),
            "path" => Ok(self.query_path(target, top)),
            "hot" => Ok(self.query_hot(target, top)),
            other => Err(format!("unknown mode {other:?}").into()),
        }
    }

    fn query_direct(&self, target: &str, top: usize, evidence_limit: isize) -> Vec<ResultItem> {
        let mut results = Vec::new();
        if let Some(neighbors) = self.adj.get(target) {
            for other in neighbors.keys() {
                if let Some(pair) = self.pairs.get(&pair_key(target, other)) {
                    let score = self.normalized_pair_score(pair);
                    results.push(pair_result(
                        pair,
                        other,
                        score,
                        "direct_cochange",
                        evidence_limit,
                    ));
                }
            }
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn query_pagerank(&self, target: &str, top: usize, evidence_limit: isize) -> Vec<ResultItem> {
        const ALPHA: f64 = 0.85;
        const ITERATIONS: usize = 30;

        let mut rank: HashMap<String, f64> = HashMap::default();
        rank.insert(target.to_string(), 1.0);
        for _ in 0..ITERATIONS {
            let mut next = HashMap::default();
            next.insert(target.to_string(), 1.0 - ALPHA);
            for (node, value) in &rank {
                if *value == 0.0 {
                    continue;
                }
                let degree = *self.degree.get(node).unwrap_or(&0.0);
                if degree == 0.0 {
                    *next.entry(target.to_string()).or_default() += ALPHA * value;
                    continue;
                }
                if let Some(neighbors) = self.adj.get(node) {
                    for (neighbor, weight) in neighbors {
                        *next.entry(neighbor.clone()).or_default() +=
                            ALPHA * value * weight / degree;
                    }
                }
            }
            rank = next;
        }

        let mut results = Vec::new();
        for (path, score) in rank {
            if path == target || score <= 0.0 {
                continue;
            }
            if let Some(pair) = self.pairs.get(&pair_key(target, &path)) {
                results.push(pair_result(
                    pair,
                    &path,
                    score,
                    "pagerank_direct_evidence",
                    evidence_limit,
                ));
            } else {
                results.push(ResultItem {
                    path,
                    score,
                    cochanges: 0,
                    weight: 0.0,
                    last_seen: String::new(),
                    reason: "pagerank_via_cochange_graph".to_string(),
                    evidence: Vec::new(),
                });
            }
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn query_path(&self, target: &str, top: usize) -> Vec<ResultItem> {
        let target_tokens = path_tokens(target);
        let mut results = Vec::new();
        for path in &self.paths {
            if path == target {
                continue;
            }
            let score = path_similarity(target, path, &target_tokens);
            if score <= 0.0 {
                continue;
            }
            results.push(ResultItem {
                path: path.clone(),
                score,
                cochanges: 0,
                weight: 0.0,
                last_seen: String::new(),
                reason: "path_name_baseline".to_string(),
                evidence: Vec::new(),
            });
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn query_hot(&self, target: &str, top: usize) -> Vec<ResultItem> {
        let mut results = Vec::new();
        for (path, stat) in &self.data.files {
            if path == target || stat.weighted_changes <= 0.0 {
                continue;
            }
            results.push(ResultItem {
                path: path.clone(),
                score: stat.weighted_changes,
                cochanges: 0,
                weight: stat.weighted_changes,
                last_seen: stat.last_seen.clone(),
                reason: "hot_file_baseline".to_string(),
                evidence: Vec::new(),
            });
        }
        truncate_top_results(&mut results, top);
        results
    }

    fn normalized_pair_score(&self, pair: &PairStat) -> f64 {
        let a = self
            .data
            .files
            .get(&pair.a)
            .map(|stat| stat.weighted_changes)
            .unwrap_or_default();
        let b = self
            .data
            .files
            .get(&pair.b)
            .map(|stat| stat.weighted_changes)
            .unwrap_or_default();
        if a <= 0.0 || b <= 0.0 {
            pair.weight
        } else {
            pair.weight / (a * b).sqrt()
        }
    }

    pub(crate) fn resolve_path(
        &self,
        repo_root: &str,
        input_base: &Path,
        input: &str,
    ) -> AnyResult<String> {
        let path = normalize_input_path(Path::new(repo_root), input_base, input)?;
        self.resolve_known_or_ambiguous_path(input, path, true)
    }

    pub(crate) fn resolve_path_or_tracked(
        &self,
        repo_root: &str,
        input_base: &Path,
        input: &str,
    ) -> AnyResult<String> {
        let path = normalize_input_path(Path::new(repo_root), input_base, input)?;
        self.resolve_known_or_tracked_path(repo_root, input, path)
    }

    fn resolve_known_or_ambiguous_path(
        &self,
        input: &str,
        path: String,
        require_graph_presence: bool,
    ) -> AnyResult<String> {
        if path.is_empty() {
            return Err(format!("{input:?} is not a valid path").into());
        }
        match self.match_graph_path(path) {
            GraphPathMatch::Known(path) => Ok(path),
            GraphPathMatch::Missing(_) if require_graph_presence => {
                Err(format!("{input:?} is not present in the co-change graph").into())
            }
            GraphPathMatch::Missing(path) => Ok(path),
            GraphPathMatch::Ambiguous(matches) => Err(ambiguous_path_error(input, &matches).into()),
        }
    }

    fn resolve_known_or_tracked_path(
        &self,
        repo_root: &str,
        input: &str,
        path: String,
    ) -> AnyResult<String> {
        if path.is_empty() {
            return Err(format!("{input:?} is not a valid path").into());
        }
        match self.match_graph_path(path) {
            GraphPathMatch::Known(path) => Ok(path),
            GraphPathMatch::Missing(path) if git_path_is_tracked(repo_root, &path)? => Ok(path),
            GraphPathMatch::Missing(_) => Err(format!(
                "{input:?} is not tracked in the repository and is not present in the co-change graph"
            )
            .into()),
            GraphPathMatch::Ambiguous(matches) => Err(ambiguous_path_error(input, &matches).into()),
        }
    }

    fn match_graph_path<'b>(&'b self, path: String) -> GraphPathMatch<'b> {
        if self.data.files.contains_key(&path) {
            return GraphPathMatch::Known(path);
        }

        let suffix = format!("/{path}");
        let mut first_match = None;
        let mut ambiguous = Vec::new();
        for candidate in &self.paths {
            if candidate.as_str() != path
                && !candidate.ends_with(&suffix)
                && path_basename(candidate) != path
            {
                continue;
            }
            let candidate = candidate.as_str();
            if let Some(first) = first_match {
                if ambiguous.is_empty() {
                    ambiguous.push(first);
                }
                ambiguous.push(candidate);
            } else {
                first_match = Some(candidate);
            }
        }

        if !ambiguous.is_empty() {
            GraphPathMatch::Ambiguous(ambiguous)
        } else if let Some(path) = first_match {
            GraphPathMatch::Known(path.to_string())
        } else {
            GraphPathMatch::Missing(path)
        }
    }
}

fn ambiguous_path_error(input: &str, matches: &[&str]) -> String {
    format!("{input:?} is ambiguous: {}", matches.join(", "))
}

fn pair_result(
    pair: &PairStat,
    path: &str,
    score: f64,
    reason: &str,
    evidence_limit: isize,
) -> ResultItem {
    let evidence = if evidence_limit >= 0 {
        pair.evidence
            .iter()
            .take(evidence_limit as usize)
            .cloned()
            .collect()
    } else {
        pair.evidence.clone()
    };
    ResultItem {
        path: path.to_string(),
        score,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen: pair.last_seen.clone(),
        reason: reason.to_string(),
        evidence,
    }
}

pub(crate) fn direct_pair_result(
    pair: DirectPairStat,
    path: String,
    score: f64,
    reason: &str,
    evidence_limit: isize,
) -> ResultItem {
    let evidence = if evidence_limit >= 0 {
        pair.evidence
            .into_iter()
            .take(evidence_limit as usize)
            .collect()
    } else {
        pair.evidence
    };
    ResultItem {
        path,
        score,
        cochanges: pair.cochanges,
        weight: pair.weight,
        last_seen: pair.last_seen,
        reason: reason.to_string(),
        evidence,
    }
}

pub(crate) fn time_decay(latest: i64, when: i64, half_life_days: f64) -> f64 {
    if !half_life_days.is_finite() || half_life_days <= 0.0 {
        return 1.0;
    }
    let age_days = ((latest - when).max(0) as f64) / 86_400.0;
    (-std::f64::consts::LN_2 * age_days / half_life_days).exp()
}

pub(crate) fn truncate_top_results(results: &mut Vec<ResultItem>, top: usize) {
    truncate_top_by(results, top, result_cmp);
}

pub(crate) fn truncate_top_direct_pairs(results: &mut Vec<DirectScoredPair>, top: usize) {
    truncate_top_by(results, top, direct_scored_pair_cmp);
}

fn result_cmp(left: &ResultItem, right: &ResultItem) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then(left.path.cmp(&right.path))
}

fn direct_scored_pair_cmp(left: &DirectScoredPair, right: &DirectScoredPair) -> Ordering {
    right
        .score
        .total_cmp(&left.score)
        .then(left.path.cmp(&right.path))
}

pub(crate) struct RelatedGraph<'a> {
    pub(crate) data: &'a GraphData,
    pairs: HashMap<String, PairStat>,
    adj: HashMap<String, HashMap<String, f64>>,
    degree: HashMap<String, f64>,
    paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GraphPathMatch<'a> {
    Known(String),
    Missing(String),
    Ambiguous(Vec<&'a str>),
}
