use crate::model::{AuditCandidate, Confidence, ConfidenceThresholds, Evidence, ResultItem};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cmp::Ordering;

const MEDIUM_CONFIDENCE_STRONGEST_PAIR: usize = 2;
const HIGH_CONFIDENCE_STRONGEST_PAIR: usize = 25;

pub(crate) fn confidence_thresholds() -> ConfidenceThresholds {
    ConfidenceThresholds {
        medium_min_strongest_pair_cochanges: MEDIUM_CONFIDENCE_STRONGEST_PAIR,
        high_min_strongest_pair_cochanges: HIGH_CONFIDENCE_STRONGEST_PAIR,
    }
}

#[derive(Debug)]
struct PendingCandidate {
    path: String,
    score: f64,
    supported_by: Vec<String>,
    cochanges: usize,
    strongest_pair_cochanges: usize,
    weight: f64,
    last_seen: String,
    reason: String,
    evidence: Vec<Evidence>,
}

pub(crate) fn aggregate_audit_results(
    seeds: &[String],
    results_by_seed: Vec<(String, Vec<ResultItem>)>,
    minimum_confidence: Confidence,
    top: usize,
    evidence_limit: usize,
) -> (Vec<AuditCandidate>, usize) {
    let seed_set: HashSet<&str> = seeds.iter().map(String::as_str).collect();
    let mut aggregate: HashMap<String, PendingCandidate> = HashMap::default();

    for (seed, results) in results_by_seed {
        for result in results {
            if seed_set.contains(result.path.as_str()) {
                continue;
            }
            if let Some(previous) = aggregate.get_mut(&result.path) {
                previous.score += result.score;
                previous.cochanges = previous.cochanges.saturating_add(result.cochanges);
                previous.strongest_pair_cochanges =
                    previous.strongest_pair_cochanges.max(result.cochanges);
                previous.weight += result.weight;
                if result.last_seen > previous.last_seen {
                    previous.last_seen = result.last_seen;
                }
                previous.supported_by.push(seed.clone());
                previous.evidence.extend(result.evidence);
                if previous.reason != result.reason {
                    previous.reason = "changed_set_cochange".to_string();
                }
            } else {
                aggregate.insert(
                    result.path.clone(),
                    PendingCandidate {
                        path: result.path,
                        score: result.score,
                        supported_by: vec![seed.clone()],
                        cochanges: result.cochanges,
                        strongest_pair_cochanges: result.cochanges,
                        weight: result.weight,
                        last_seen: result.last_seen,
                        reason: result.reason,
                        evidence: result.evidence,
                    },
                );
            }
        }
    }

    let mut all_candidates: Vec<AuditCandidate> = aggregate
        .into_values()
        .map(|mut candidate| {
            candidate.supported_by.sort();
            candidate.supported_by.dedup();
            candidate
                .evidence
                .sort_by(|left, right| right.date.cmp(&left.date).then(left.hash.cmp(&right.hash)));
            candidate
                .evidence
                .dedup_by(|left, right| left.hash == right.hash);
            candidate.evidence.truncate(evidence_limit);
            let support_count = candidate.supported_by.len();
            let confidence = classify_confidence(candidate.strongest_pair_cochanges);
            AuditCandidate {
                path: candidate.path,
                score: candidate.score,
                confidence,
                support_count,
                supported_by: candidate.supported_by,
                cochanges: candidate.cochanges,
                strongest_pair_cochanges: candidate.strongest_pair_cochanges,
                weight: candidate.weight,
                last_seen: candidate.last_seen,
                reason: candidate.reason,
                evidence: candidate.evidence,
            }
        })
        .collect();

    all_candidates.sort_by(compare_candidates);
    let before_filter = all_candidates.len();
    all_candidates.retain(|candidate| candidate.confidence >= minimum_confidence);
    let filtered = before_filter.saturating_sub(all_candidates.len());
    all_candidates.truncate(top);
    (all_candidates, filtered)
}

fn classify_confidence(strongest_pair_cochanges: usize) -> Confidence {
    if strongest_pair_cochanges >= HIGH_CONFIDENCE_STRONGEST_PAIR {
        Confidence::High
    } else if strongest_pair_cochanges >= MEDIUM_CONFIDENCE_STRONGEST_PAIR {
        Confidence::Medium
    } else {
        Confidence::Low
    }
}

fn compare_candidates(left: &AuditCandidate, right: &AuditCandidate) -> Ordering {
    right
        .confidence
        .cmp(&left.confidence)
        .then(right.support_count.cmp(&left.support_count))
        .then_with(|| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
        })
        .then(right.cochanges.cmp(&left.cochanges))
        .then(left.path.cmp(&right.path))
}

/// Keep discovery and chronological evaluation on the same candidate budget.
pub(crate) fn audit_query_limit(top: usize) -> usize {
    top.saturating_mul(8).max(64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(path: &str, cochanges: usize, score: f64) -> ResultItem {
        ResultItem {
            path: path.to_string(),
            score,
            cochanges,
            weight: score,
            last_seen: "2026-01-01T00:00:00Z".to_string(),
            reason: "direct_cochange".to_string(),
            evidence: Vec::new(),
        }
    }

    #[test]
    fn aggregates_support_and_filters_changed_files() {
        let seeds = vec!["a.rs".to_string(), "b.rs".to_string()];
        let (candidates, filtered) = aggregate_audit_results(
            &seeds,
            vec![
                (
                    "a.rs".to_string(),
                    vec![result("b.rs", 3, 0.8), result("test.rs", 2, 0.7)],
                ),
                (
                    "b.rs".to_string(),
                    vec![result("test.rs", 3, 0.9), result("weak.md", 1, 0.2)],
                ),
            ],
            Confidence::Medium,
            20,
            0,
        );

        assert_eq!(filtered, 1);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, "test.rs");
        assert_eq!(candidates[0].confidence, Confidence::Medium);
        assert_eq!(candidates[0].support_count, 2);
        assert_eq!(candidates[0].supported_by, seeds);
        assert_eq!(candidates[0].cochanges, 5);
        assert_eq!(candidates[0].strongest_pair_cochanges, 3);
    }

    #[test]
    fn a_repeated_single_seed_relationship_is_medium_confidence() {
        let seeds = vec!["a.rs".to_string()];
        let (candidates, _) = aggregate_audit_results(
            &seeds,
            vec![("a.rs".to_string(), vec![result("test.rs", 2, 0.7)])],
            Confidence::Medium,
            20,
            0,
        );
        assert_eq!(candidates[0].confidence, Confidence::Medium);
    }

    #[test]
    fn a_relationship_repeated_twenty_five_times_is_high_confidence() {
        let seeds = vec!["a.rs".to_string()];
        let (candidates, _) = aggregate_audit_results(
            &seeds,
            vec![("a.rs".to_string(), vec![result("test.rs", 25, 0.9)])],
            Confidence::High,
            20,
            0,
        );
        assert_eq!(candidates[0].confidence, Confidence::High);
    }

    #[test]
    fn indirect_graph_support_without_cochanges_stays_low_confidence() {
        let seeds = vec!["a.rs".to_string(), "b.rs".to_string()];
        let (candidates, filtered) = aggregate_audit_results(
            &seeds,
            vec![
                ("a.rs".to_string(), vec![result("indirect.rs", 0, 0.5)]),
                ("b.rs".to_string(), vec![result("indirect.rs", 0, 0.5)]),
            ],
            Confidence::Medium,
            20,
            0,
        );
        assert!(candidates.is_empty());
        assert_eq!(filtered, 1);
    }
}
