use super::support::{new_test_repo, write_commit};
use crate::commands::merge_diff_result;
use crate::evaluation::evaluate_global;
use crate::graph::RelatedGraph;
use crate::graph::{build_graph_data, query_direct_from_commits};
use crate::history::{
    git_diff_tree_direct_from_hash_input, git_diff_tree_selected_commits, git_log,
    git_log_direct_for_target, git_target_commit_hash_input, git_target_commit_seeds,
};
use crate::model::{Evidence, GraphBuildConfig, OnDemandBackend, OnDemandConfig, ResultItem};
use std::fs;

#[test]
fn graph_query_explain_and_eval() {
    let repo = new_test_repo();
    write_commit(
        &repo,
        "initial auth pair",
        &[
            ("src/auth.ts", "auth v1\n"),
            ("tests/auth.test.ts", "test v1\n"),
        ],
    );
    write_commit(
        &repo,
        "second auth pair",
        &[
            ("src/auth.ts", "auth v2\n"),
            ("tests/auth.test.ts", "test v2\n"),
        ],
    );
    write_commit(
        &repo,
        "unrelated docs pair",
        &[("docs/api.md", "api\n"), ("openapi.yaml", "openapi\n")],
    );
    write_commit(
        &repo,
        "future auth change",
        &[
            ("src/auth.ts", "auth v3\n"),
            ("tests/auth.test.ts", "test v3\n"),
            ("docs/auth.md", "auth docs\n"),
        ],
    );

    let commits = git_log(repo.to_str().unwrap(), 20, None).unwrap();
    assert_eq!(commits.len(), 4);
    let config = OnDemandConfig {
        backend: OnDemandBackend::GitDiffTree,
        backend_explicit: true,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 3,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let seeds = git_target_commit_seeds(repo.to_str().unwrap(), "src/auth.ts", 20, None).unwrap();
    let selected_commits = git_diff_tree_selected_commits(repo.to_str().unwrap(), &seeds).unwrap();
    let input =
        git_target_commit_hash_input(repo.to_str().unwrap(), "src/auth.ts", 20, None).unwrap();
    let streamed_results = git_diff_tree_direct_from_hash_input(
        repo.to_str().unwrap(),
        &input,
        "src/auth.ts",
        &config,
        5,
    )
    .unwrap();
    let selected_results =
        query_direct_from_commits("src/auth.ts", &selected_commits, &config, 5, 3);
    assert_eq!(streamed_results.len(), selected_results.len());
    for (left, right) in streamed_results.iter().zip(selected_results.iter()) {
        assert_eq!(left.path, right.path);
        assert_eq!(left.cochanges, right.cochanges);
        assert!((left.score - right.score).abs() < 1e-12);
    }
    let log_results =
        git_log_direct_for_target(repo.to_str().unwrap(), "src/auth.ts", &config, 5).unwrap();
    assert_eq!(log_results.len(), selected_results.len());
    for (left, right) in log_results.iter().zip(selected_results.iter()) {
        assert_eq!(left.path, right.path);
        assert_eq!(left.cochanges, right.cochanges);
        assert!((left.score - right.score).abs() < 1e-12);
    }
    let mut compact_config = config.clone();
    compact_config.evidence_limit = 0;
    let compact_log_results =
        git_log_direct_for_target(repo.to_str().unwrap(), "src/auth.ts", &compact_config, 5)
            .unwrap();
    let compact_selected_results =
        query_direct_from_commits("src/auth.ts", &selected_commits, &compact_config, 5, 0);
    assert_eq!(compact_log_results.len(), compact_selected_results.len());
    for (left, right) in compact_log_results
        .iter()
        .zip(compact_selected_results.iter())
    {
        assert_eq!(left.path, right.path);
        assert_eq!(left.cochanges, right.cochanges);
        assert!((left.score - right.score).abs() < 1e-12);
        assert!(left.evidence.is_empty());
    }

    let data = build_graph_data(
        repo.to_str().unwrap(),
        &commits[1..],
        GraphBuildConfig {
            max_files_per_commit: config.max_files_per_commit,
            half_life_days: config.half_life_days,
            evidence_limit: config.evidence_limit,
        },
    );
    let graph = RelatedGraph::new(&data);
    let results = graph.query("src/auth.ts", "direct", 5, 3).unwrap();
    let fast_results = query_direct_from_commits("src/auth.ts", &commits[1..], &config, 5, 3);
    assert_eq!(
        results.iter().map(|item| &item.path).collect::<Vec<_>>(),
        fast_results
            .iter()
            .map(|item| &item.path)
            .collect::<Vec<_>>()
    );
    assert!(!results.is_empty());
    assert_eq!(results[0].path, "tests/auth.test.ts");
    assert_eq!(results[0].cochanges, 2);
    assert!(graph.pair("src/auth.ts", "tests/auth.test.ts").is_some());

    let report = evaluate_global(
        &graph,
        &commits[..1],
        &[
            "direct".to_string(),
            "pagerank".to_string(),
            "path".to_string(),
        ],
        3,
        10,
    )
    .unwrap();
    assert!(report.evaluated_tasks > 0);
    let direct = report
        .metrics
        .iter()
        .find(|metric| metric.mode == "direct")
        .unwrap();
    assert!(direct.hit_rate_at_k > 0.0);

    fs::remove_dir_all(repo).ok();
}

#[test]
fn diff_aggregation_merges_metadata_and_deduplicates_evidence() {
    fn evidence(hash: &str, date: &str) -> Evidence {
        Evidence {
            hash: hash.to_string(),
            date: date.to_string(),
            subject: hash.to_string(),
            file_count: 2,
            weight: 1.0,
        }
    }

    let mut target = ResultItem {
        path: "shared.md".to_string(),
        score: 1.25,
        cochanges: 2,
        weight: 0.5,
        last_seen: "2026-01-01".to_string(),
        reason: "direct_cochange".to_string(),
        evidence: vec![
            evidence("same", "2026-01-01"),
            evidence("old", "2025-01-01"),
        ],
    };
    let source = ResultItem {
        path: "shared.md".to_string(),
        score: 2.75,
        cochanges: 3,
        weight: 1.5,
        last_seen: "2026-02-01".to_string(),
        reason: "pagerank_direct_evidence".to_string(),
        evidence: vec![
            evidence("new", "2026-02-01"),
            evidence("same", "2026-01-01"),
        ],
    };

    merge_diff_result(&mut target, source, 2);

    assert_eq!(target.score, 4.0);
    assert_eq!(target.cochanges, 5);
    assert_eq!(target.weight, 2.0);
    assert_eq!(target.last_seen, "2026-02-01");
    assert_eq!(target.reason, "diff_aggregate");
    assert_eq!(
        target
            .evidence
            .iter()
            .map(|item| item.hash.as_str())
            .collect::<Vec<_>>(),
        vec!["new", "same"]
    );
}
