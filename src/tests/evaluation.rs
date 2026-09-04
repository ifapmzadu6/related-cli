use super::support::{git, new_test_repo, write_commit};
use crate::commands::run_with_writer;
use crate::evaluation::{
    evaluate_audit_on_demand, evaluate_global, evaluate_on_demand,
    prepare_rename_aware_audit_history,
};
use crate::graph::RelatedGraph;
use crate::graph::build_graph_data;
use crate::model::{Commit, Confidence, GraphBuildConfig, HistoryRename, RenameAwareCommit};
use std::fs;

#[test]
fn cli_legacy_query_eval_keeps_on_demand_and_global_available() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    write_commit(&repo, "pair three", &[("a.md", "a3\n"), ("b.md", "b3\n")]);

    for (extra, expected_shape) in [
        (Vec::<String>::new(), "on-demand"),
        (
            vec!["--query-shape".to_string(), "global".to_string()],
            "global",
        ),
    ] {
        let mut args = vec![
            "eval".to_string(),
            "--task".to_string(),
            "query".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--test-commits".to_string(),
            "1".to_string(),
            "--train-commits".to_string(),
            "2".to_string(),
            "--top".to_string(),
            "1".to_string(),
            "--modes".to_string(),
            "direct".to_string(),
        ];
        args.extend(extra);
        let mut output = Vec::new();
        run_with_writer(args, &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains(&format!("query_shape={expected_shape}")));
        assert!(text.contains("direct"));
    }

    fs::remove_dir_all(repo).ok();
}

#[test]
fn audit_evaluation_maps_only_training_and_current_test_renames() {
    fn record(hash: &str, files: &[&str], rename: Option<(&str, &str)>) -> RenameAwareCommit {
        RenameAwareCommit {
            commit: Commit {
                hash: hash.to_string(),
                unix_time: 1,
                date: "2026-01-01T00:00:00Z".to_string(),
                subject: hash.to_string(),
                files: files.iter().map(|file| (*file).to_string()).collect(),
            },
            renames: rename
                .map(|(old_path, new_path)| {
                    vec![HistoryRename {
                        old_path: old_path.to_string(),
                        new_path: new_path.to_string(),
                    }]
                })
                .unwrap_or_default(),
        }
    }

    let records = vec![
        record("test-after", &["new.md", "companion.md"], None),
        record(
            "test-rename",
            &["new.md", "companion.md"],
            Some(("old.md", "new.md")),
        ),
        record("train-two", &["old.md", "companion.md"], None),
        record("train-one", &["old.md", "companion.md"], None),
    ];
    let history = prepare_rename_aware_audit_history(&records, 2).unwrap();
    assert_eq!(history.training_renames, 0);
    assert_eq!(history.test_diff_renames, 1);
    assert_eq!(history.test[0].files, vec!["new.md", "companion.md"]);
    assert_eq!(history.test[1].files, vec!["old.md", "companion.md"]);

    let config = GraphBuildConfig {
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 0,
    };
    let report = evaluate_audit_on_demand(
        &history.train,
        &history.test,
        &["direct".to_string()],
        5,
        config,
        Confidence::Medium,
    )
    .unwrap();
    assert_eq!(report.evaluated_tasks, 2);
    assert_eq!(report.skipped_unknown_targets, 1);
    assert_eq!(report.metrics[0].hit_rate_at_k, 1.0);

    let records = vec![
        record("test", &["new.md", "companion.md"], None),
        record("train-new", &["new.md", "companion.md"], None),
        record(
            "train-rename",
            &["new.md", "companion.md"],
            Some(("old.md", "new.md")),
        ),
        record("train-old", &["old.md", "companion.md"], None),
    ];
    let history = prepare_rename_aware_audit_history(&records, 1).unwrap();
    assert_eq!(history.training_renames, 1);
    assert_eq!(history.test_diff_renames, 0);
    assert!(
        history
            .train
            .iter()
            .all(|commit| commit.files.contains(&"new.md".to_string()))
    );
}

#[test]
fn cli_audit_evaluation_parses_rename_boundaries() {
    let repo = new_test_repo();
    let old = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
    write_commit(
        &repo,
        "old pair one",
        &[("old.md", old), ("companion.md", "one\n")],
    );
    write_commit(
        &repo,
        "old pair two",
        &[
            ("old.md", &format!("{old}eleven\n")),
            ("companion.md", "two\n"),
        ],
    );
    git(&repo, &["mv", "old.md", "new.md"]);
    fs::write(repo.join("companion.md"), "three\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "rename with companion"]);

    let evaluate = |test_commits: &str, train_commits: &str| {
        let mut output = Vec::new();
        run_with_writer(
            vec![
                "eval".to_string(),
                "--task".to_string(),
                "audit".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--test-commits".to_string(),
                test_commits.to_string(),
                "--train-commits".to_string(),
                train_commits.to_string(),
                "--top".to_string(),
                "5".to_string(),
                "--modes".to_string(),
                "direct".to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &mut output,
        )
        .unwrap();
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };

    let at_rename = evaluate("1", "2");
    assert_eq!(
        at_rename["rename_tracking"],
        "training-window+current-test-diff"
    );
    assert_eq!(at_rename["training_renames"], 0);
    assert_eq!(at_rename["test_diff_renames"], 1);
    assert_eq!(at_rename["evaluated_tasks"], 2);
    assert_eq!(at_rename["metrics"][0]["hit_rate_at_k"], 1.0);

    write_commit(
        &repo,
        "new pair",
        &[
            ("new.md", &format!("{old}after\n")),
            ("companion.md", "four\n"),
        ],
    );
    let after_rename = evaluate("1", "3");
    assert_eq!(after_rename["training_renames"], 1);
    assert_eq!(after_rename["test_diff_renames"], 0);
    assert_eq!(after_rename["evaluated_tasks"], 2);
    assert_eq!(after_rename["metrics"][0]["hit_rate_at_k"], 1.0);

    fs::remove_dir_all(repo).ok();
}

#[test]
fn on_demand_eval_matches_the_shipping_graph_shape() {
    fn commit(hash: &str, time: i64, files: &[&str]) -> Commit {
        Commit {
            hash: hash.to_string(),
            unix_time: time,
            date: format!("2026-01-{:02}T00:00:00Z", time),
            subject: hash.to_string(),
            files: files.iter().map(|file| (*file).to_string()).collect(),
        }
    }

    let train = vec![
        commit("ab", 2, &["a.md", "b.md"]),
        commit("bc", 1, &["b.md", "c.md"]),
    ];
    let test = vec![commit("ac", 3, &["a.md", "c.md"])];
    let config = GraphBuildConfig {
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 0,
    };
    let modes = vec!["pagerank".to_string()];
    let data = build_graph_data("", &train, config);
    let graph = RelatedGraph::new(&data);
    let global = evaluate_global(&graph, &test, &modes, 2, 10).unwrap();
    let on_demand = evaluate_on_demand(&train, &test, &modes, 2, config).unwrap();
    let audit =
        evaluate_audit_on_demand(&train, &test, &modes, 2, config, Confidence::Medium).unwrap();

    assert_eq!(global.query_shape, "global");
    assert_eq!(on_demand.query_shape, "on-demand");
    assert_eq!(audit.query_shape, "on-demand-leave-one-out");
    assert_eq!(audit.evaluated_tasks, 2);
    assert_eq!(global.metrics[0].hit_rate_at_k, 1.0);
    assert_eq!(on_demand.metrics[0].hit_rate_at_k, 0.0);
    assert_eq!(audit.metrics[0].hit_rate_at_k, 0.0);
}
