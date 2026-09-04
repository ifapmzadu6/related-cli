use super::support::{git, new_test_repo, write_commit};
use crate::commands::run_with_writer;
use std::fs;

#[test]
fn cli_rejects_json_flag() {
    let mut output = Vec::new();
    let err = run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--json".to_string(),
        ],
        &mut output,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("unknown flag --json"));
}

#[test]
fn cli_supports_json_format_for_all_commands() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    write_commit(&repo, "pair three", &[("a.md", "a3\n"), ("b.md", "b3\n")]);

    let run_json = |args: Vec<String>| {
        let mut output = Vec::new();
        run_with_writer(args, &mut output).unwrap();
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };

    let query = run_json(vec![
        "query".to_string(),
        "a.md".to_string(),
        "--repo".to_string(),
        repo.display().to_string(),
        "--history-backend".to_string(),
        "git".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    assert_eq!(query["schema_version"], 1);
    assert_eq!(query["target"], "a.md");
    assert_eq!(query["related"][0]["path"], "b.md");
    assert_eq!(query["related"][0]["cochanges"], 3);

    let explain = run_json(vec![
        "explain".to_string(),
        "a.md".to_string(),
        "b.md".to_string(),
        "--repo".to_string(),
        repo.display().to_string(),
        "--history-backend".to_string(),
        "git".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    assert_eq!(explain["schema_version"], 1);
    assert_eq!(explain["related"], true);
    assert_eq!(explain["cochanges"], 3);
    assert_eq!(explain["hints"], serde_json::json!([]));

    fs::write(repo.join("a.md"), "staged\n").unwrap();
    git(&repo, &["add", "a.md"]);
    let diff = run_json(vec![
        "diff".to_string(),
        "--staged".to_string(),
        "--repo".to_string(),
        repo.display().to_string(),
        "--history-backend".to_string(),
        "git".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    assert_eq!(diff["schema_version"], 1);
    assert_eq!(diff["target"], "a.md");
    assert_eq!(diff["related"][0]["path"], "b.md");

    let audit = run_json(vec![
        "audit".to_string(),
        "--staged".to_string(),
        "--repo".to_string(),
        repo.display().to_string(),
        "--accuracy".to_string(),
        "exact".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    assert_eq!(audit["schema_version"], 2);
    assert_eq!(audit["scope"], "staged");
    assert_eq!(audit["seeds"], serde_json::json!(["a.md"]));
    assert_eq!(audit["candidates"][0]["path"], "b.md");
    assert_eq!(audit["candidates"][0]["confidence"], "medium");
    assert_eq!(
        audit["confidence_thresholds"]["high_min_strongest_pair_cochanges"],
        25
    );
    assert_eq!(
        audit["candidates"][0]["supported_by"],
        serde_json::json!(["a.md"])
    );
    assert_eq!(audit["history_coverage"]["backend"], "GitCli");
    assert_eq!(audit["history_coverage"]["approximate"], false);
    assert_eq!(audit["history_coverage"]["rename_tracking"], "git-follow");

    let legacy_eval = run_json(vec![
        "eval".to_string(),
        "--task".to_string(),
        "query".to_string(),
        "--repo".to_string(),
        repo.display().to_string(),
        "--test-commits".to_string(),
        "1".to_string(),
        "--train-commits".to_string(),
        "2".to_string(),
        "--modes".to_string(),
        "direct".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    assert_eq!(legacy_eval["schema_version"], 1);
    assert_eq!(legacy_eval["query_shape"], "on-demand");
    assert_eq!(legacy_eval["metrics"][0]["mode"], "direct");

    let audit_eval = run_json(vec![
        "eval".to_string(),
        "--repo".to_string(),
        repo.display().to_string(),
        "--test-commits".to_string(),
        "1".to_string(),
        "--train-commits".to_string(),
        "2".to_string(),
        "--modes".to_string(),
        "direct".to_string(),
        "--format".to_string(),
        "json".to_string(),
    ]);
    assert_eq!(audit_eval["schema_version"], 2);
    assert_eq!(audit_eval["query_shape"], "on-demand-leave-one-out");
    assert_eq!(
        audit_eval["rename_tracking"],
        "training-window+current-test-diff"
    );
    assert_eq!(audit_eval["metrics"][0]["hit_rate_at_k"], 1.0);
    assert_eq!(
        audit_eval["confidence_thresholds"]["medium_min_strongest_pair_cochanges"],
        2
    );
    assert_eq!(
        audit_eval["confidence_metrics"].as_array().unwrap().len(),
        3
    );

    let invalid = run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--format".to_string(),
            "yaml".to_string(),
        ],
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(invalid.contains("use text or json"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn cli_subcommands_provide_help_without_repository_access() {
    for command in ["query", "explain", "audit", "diff", "eval"] {
        let mut output = Vec::new();
        run_with_writer(vec![command.to_string(), "--help".to_string()], &mut output).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.starts_with(&format!("Usage: related {command}")));
        assert!(text.contains("-h, --help"));
    }
}

#[test]
fn cli_deprecated_on_demand_flag_remains_compatible() {
    let repo = new_test_repo();
    write_commit(&repo, "pair", &[("a.md", "a\n"), ("b.md", "b\n")]);
    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
            "--on-demand".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("1 b.md co=1"));
    assert!(text.contains("--on-demand is redundant"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn cli_default_text_output_is_compact() {
    let repo = new_test_repo();
    write_commit(&repo, "pair", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair again", &[("a.md", "a2\n"), ("b.md", "b2\n")]);

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
            "--max-commits".to_string(),
            "20".to_string(),
            "--mode".to_string(),
            "direct".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("related a.md direct:on-demand:GitCli\n"));
    assert!(text.contains("1 b.md co=2\n"));
    assert!(!text.contains("seen="));
    assert!(!text.contains("cochanged="));
    assert!(!text.contains("direct_cochange"));

    fs::remove_dir_all(repo).ok();
}
