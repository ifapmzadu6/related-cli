use crate::BROAD_CHANGE_EXCLUDE_SUGGESTION;
use crate::commands::{explain_relationship, merge_diff_result, run_with_writer};
use crate::evaluation::{evaluate_global, evaluate_on_demand};
use crate::graph::{build_graph_data, query_direct_from_commits};
use crate::history::{
    git_diff_tree_direct_from_hash_input, git_diff_tree_selected_commits, git_log,
    git_log_direct_for_target, git_target_commit_hash_input, git_target_commit_seeds,
};
use crate::model::*;
use crate::pack::git_pack_scan_direct_for_target;
use crate::path_utils::pair_key;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

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
    assert!(
        graph
            .pairs
            .contains_key(&pair_key("src/auth.ts", "tests/auth.test.ts"))
    );

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
fn cli_on_demand_default_query() {
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

    fs::remove_dir_all(repo).ok();
}

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

    let eval = run_json(vec![
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
    assert_eq!(eval["schema_version"], 1);
    assert_eq!(eval["query_shape"], "on-demand");
    assert_eq!(eval["metrics"][0]["mode"], "direct");

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
    for command in ["query", "explain", "diff", "eval"] {
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

#[test]
fn cli_query_resolves_paths_from_repo_argument_base() {
    let repo = new_test_repo();
    write_commit(
        &repo,
        "nested pair",
        &[("src/a.md", "a\n"), ("src/b.md", "b\n")],
    );
    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            repo.join("src").display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("related src/a.md direct:on-demand:GitCli\n"));
    assert!(text.contains("1 src/b.md co=1"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn cli_query_rejects_missing_targets_and_non_finite_decay() {
    let repo = new_test_repo();
    write_commit(&repo, "pair", &[("a.md", "a\n"), ("b.md", "b\n")]);

    let missing = run_with_writer(
        vec![
            "query".to_string(),
            "missing.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
        ],
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(missing.contains("is not tracked in the repository"));

    let nan = run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--half-life-days".to_string(),
            "NaN".to_string(),
        ],
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(nan.contains("must be a finite positive number"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn cli_eval_defaults_to_on_demand_and_keeps_global_available() {
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
fn git_backend_and_diff_support_unicode_paths() {
    let repo = new_test_repo();
    write_commit(
        &repo,
        "unicode pair",
        &[("café.md", "one\n"), ("other.md", "other\n")],
    );

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "café.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("1 other.md co=1"));

    fs::write(repo.join("café.md"), "two\n").unwrap();
    git(&repo, &["add", "café.md"]);
    let mut output = Vec::new();
    run_with_writer(
        vec![
            "diff".to_string(),
            "--staged".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("related café.md direct\n"));
    assert!(text.contains("1 other.md co=1"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn default_backend_falls_back_for_sha256_repositories() {
    let repo = temp_dir();
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--object-format=sha256"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test User"]);
    write_commit(&repo, "pair", &[("a.md", "a\n"), ("b.md", "b\n")]);

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("related a.md direct:on-demand:GitCli\n"));
    assert!(text.contains("1 b.md co=1"));
    assert!(text.contains("used the git backend instead"));

    let explicit_pack = run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "pack-scan".to_string(),
        ],
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(explicit_pack.contains("does not support Git object format \"sha256\""));
    assert!(explicit_pack.contains("use --history-backend git"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn default_backend_falls_back_when_pack_storage_is_unavailable() {
    let source = new_test_repo();
    write_commit(&source, "pair", &[("a.md", "a\n"), ("b.md", "b\n")]);
    let shared = temp_dir();
    let output = Command::new("git")
        .args(["clone", "--quiet", "--shared"])
        .arg(&source)
        .arg(&shared)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git clone --shared failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            shared.display().to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("related a.md direct:on-demand:GitCli\n"));
    assert!(text.contains("1 b.md co=1"));
    assert!(text.contains("pack-fast could not read this repository; used the git backend"));

    let explicit_pack = run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            shared.display().to_string(),
            "--history-backend".to_string(),
            "pack-fast".to_string(),
        ],
        &mut Vec::new(),
    );
    assert!(explicit_pack.is_err());

    fs::remove_dir_all(shared).ok();
    fs::remove_dir_all(source).ok();
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

    assert_eq!(global.query_shape, "global");
    assert_eq!(on_demand.query_shape, "on-demand");
    assert_eq!(global.metrics[0].hit_rate_at_k, 1.0);
    assert_eq!(on_demand.metrics[0].hit_rate_at_k, 0.0);
}

#[test]
fn cli_query_supports_exclude_patterns() {
    let repo = new_test_repo();
    write_commit(
        &repo,
        "pair with lockfile",
        &[
            ("a.md", "a1\n"),
            ("b.md", "b1\n"),
            ("Cargo.lock", "lock1\n"),
            ("package-lock.json", "package lock 1\n"),
            ("pnpm-lock.yaml", "pnpm lock 1\n"),
            ("bun.lockb", "bun lock 1\n"),
        ],
    );
    write_commit(
        &repo,
        "pair with lockfile again",
        &[
            ("a.md", "a2\n"),
            ("b.md", "b2\n"),
            ("Cargo.lock", "lock2\n"),
            ("package-lock.json", "package lock 2\n"),
            ("pnpm-lock.yaml", "pnpm lock 2\n"),
            ("bun.lockb", "bun lock 2\n"),
        ],
    );

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
            "--exclude".to_string(),
            BROAD_CHANGE_EXCLUDE_SUGGESTION.to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("1 b.md co=2"));
    assert!(!text.contains("Cargo.lock"));
    assert!(!text.contains("package-lock.json"));
    assert!(!text.contains("pnpm-lock.yaml"));
    assert!(!text.contains("bun.lockb"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn cli_exclusions_do_not_hide_lower_ranked_results() {
    let repo = new_test_repo();
    fs::write(repo.join("target.md"), "target\n").unwrap();
    fs::write(repo.join("z-keep.md"), "keep\n").unwrap();
    for idx in 1..=25 {
        fs::write(repo.join(format!("{idx:02}.lock")), "lock\n").unwrap();
    }
    git(&repo, &["add", "."]);
    git(
        &repo,
        &["commit", "-m", "many lockfiles and one useful file"],
    );

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "target.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
            "--top".to_string(),
            "1".to_string(),
            "--exclude".to_string(),
            "*.lock".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("1 z-keep.md co=1"));
    assert!(!text.contains("no related files found"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn cli_explain_supports_text_and_unrelated_files() {
    let repo = new_test_repo();
    write_commit(&repo, "pair", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "other pair", &[("c.md", "c1\n"), ("d.md", "d1\n")]);
    let config = OnDemandConfig {
        backend: OnDemandBackend::GitCli,
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

    let related =
        explain_relationship(repo.to_str().unwrap(), &repo, "a.md", "b.md", &config).unwrap();
    assert!(related.related);
    assert_eq!(related.a, "a.md");
    assert_eq!(related.b, "b.md");
    assert_eq!(related.cochanges, 1);
    assert_eq!(related.evidence[0].subject, "pair");

    let unrelated =
        explain_relationship(repo.to_str().unwrap(), &repo, "a.md", "c.md", &config).unwrap();
    assert!(!unrelated.related);
    assert_eq!(unrelated.a, "a.md");
    assert_eq!(unrelated.b, "c.md");
    assert_eq!(unrelated.cochanges, 0);

    let missing =
        explain_relationship(repo.to_str().unwrap(), &repo, "a.md", "missing.md", &config)
            .unwrap_err()
            .to_string();
    assert!(missing.contains("not tracked in the repository"));

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "explain".to_string(),
            "a.md".to_string(),
            "b.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("a.md <-> b.md"));
    assert!(text.contains("cochanged=1"));
    assert!(text.contains("files=2"));

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "explain".to_string(),
            "a.md".to_string(),
            "c.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("a.md and c.md have no direct co-change evidence"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn target_pathspec_is_literal() {
    let repo = new_test_repo();
    write_commit(
        &repo,
        "literal special path",
        &[
            ("src/literal[1].ts", "special\n"),
            ("tests/literal.test.ts", "test\n"),
        ],
    );
    write_commit(
        &repo,
        "similar glob match",
        &[("src/literal1.ts", "plain\n"), ("docs/plain.md", "plain\n")],
    );
    let config = OnDemandConfig {
        backend: OnDemandBackend::GitCli,
        backend_explicit: true,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 0,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };

    let results =
        git_log_direct_for_target(repo.to_str().unwrap(), "src/literal[1].ts", &config, 5).unwrap();
    assert_eq!(
        results
            .iter()
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>(),
        vec!["tests/literal.test.ts"]
    );

    fs::remove_dir_all(repo).ok();
}

#[test]
fn pack_scan_backend_matches_git_direct_query() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    write_commit(&repo, "other", &[("c.md", "c\n"), ("d.md", "d\n")]);
    git(&repo, &["gc", "--quiet"]);

    let mut config = OnDemandConfig {
        backend: OnDemandBackend::PackScan,
        backend_explicit: true,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 0,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let pack_results =
        git_pack_scan_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
    config.backend = OnDemandBackend::GitCli;
    let git_results =
        git_log_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();

    assert_eq!(
        pack_results
            .iter()
            .map(|item| (item.path.as_str(), item.cochanges))
            .collect::<Vec<_>>(),
        git_results
            .iter()
            .map(|item| (item.path.as_str(), item.cochanges))
            .collect::<Vec<_>>()
    );

    config.backend = OnDemandBackend::PackScan;
    config.evidence_limit = 2;
    let pack_results =
        git_pack_scan_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
    config.backend = OnDemandBackend::GitCli;
    let git_results =
        git_log_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
    assert_eq!(
        pack_results[0].evidence.len(),
        git_results[0].evidence.len()
    );
    assert_eq!(pack_results[0].evidence[0].subject, "pair two");

    fs::remove_dir_all(repo).ok();
}

#[test]
fn pack_scan_backend_matches_git_across_generated_history_shapes() {
    let repo = new_test_repo();
    git(&repo, &["config", "core.filemode", "true"]);
    write_commit(
        &repo,
        "generated base",
        &[
            ("target.md", "target-0\n"),
            ("src/a.md", "a-0\n"),
            ("src/b.md", "b-0\n"),
            ("src/c.md", "c-0\n"),
            ("src/d.md", "d-0\n"),
            ("src/e.md", "e-0\n"),
            ("deep/a/b/c/d/e.md", "deep-0\n"),
        ],
    );

    let mut candidates = vec![
        "src/a.md",
        "src/b.md",
        "src/c.md",
        "src/d.md",
        "src/e.md",
        "deep/a/b/c/d/e.md",
    ];
    let mut state = 0x5eed_u64;
    for idx in 1..=32 {
        let message = format!("generated {idx:02}");
        match idx {
            6 => {
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                git(&repo, &["mv", "src/c.md", "src/c-renamed.md"]);
                candidates[2] = "src/c-renamed.md";
            }
            11 => {
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                fs::remove_file(repo.join("src/b.md")).unwrap();
            }
            15 => {
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                fs::write(repo.join("src/b.md"), format!("b-{idx}\n")).unwrap();
            }
            19 => {
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                git(&repo, &["update-index", "--chmod=+x", "--", "src/d.md"]);
            }
            23 => {
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                for bulk_idx in 0..12 {
                    let path = repo.join(format!("bulk/{bulk_idx:02}.txt"));
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, format!("bulk-{idx}-{bulk_idx}\n")).unwrap();
                }
            }
            27 => {
                fs::remove_file(repo.join("target.md")).unwrap();
                fs::write(repo.join("src/e.md"), format!("e-{idx}\n")).unwrap();
            }
            28 => {
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                fs::write(repo.join("src/e.md"), format!("e-{idx}\n")).unwrap();
            }
            _ => {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                let first = (state as usize) % candidates.len();
                fs::write(repo.join("target.md"), format!("target-{idx}\n")).unwrap();
                fs::write(
                    repo.join(candidates[first]),
                    format!("candidate-{first}-{idx}\n"),
                )
                .unwrap();
                if idx % 4 == 0 {
                    let second = (first + 3) % candidates.len();
                    fs::write(
                        repo.join(candidates[second]),
                        format!("candidate-{second}-{idx}\n"),
                    )
                    .unwrap();
                }
            }
        }
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", &message]);
    }
    git(&repo, &["commit", "--allow-empty", "-m", "generated empty"]);
    git(&repo, &["gc", "--quiet"]);

    let mut config = OnDemandConfig {
        backend: OnDemandBackend::PackScan,
        backend_explicit: true,
        max_commits: 100,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 5,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let pack_results =
        git_pack_scan_direct_for_target(repo.to_str().unwrap(), "target.md", &config, 20).unwrap();
    config.backend = OnDemandBackend::GitCli;
    let git_results =
        git_log_direct_for_target(repo.to_str().unwrap(), "target.md", &config, 20).unwrap();

    assert_eq!(pack_results.len(), git_results.len());
    for (pack, git) in pack_results.iter().zip(&git_results) {
        assert_eq!(pack.path, git.path);
        assert_eq!(pack.cochanges, git.cochanges, "path={}", pack.path);
        assert!((pack.score - git.score).abs() < 1e-12, "path={}", pack.path);
        assert!(
            (pack.weight - git.weight).abs() < 1e-12,
            "path={}",
            pack.path
        );
        assert_eq!(pack.last_seen, git.last_seen, "path={}", pack.path);
        assert_eq!(pack.reason, git.reason, "path={}", pack.path);
        assert_eq!(
            pack.evidence.len(),
            git.evidence.len(),
            "path={}",
            pack.path
        );
        for (pack_evidence, git_evidence) in pack.evidence.iter().zip(&git.evidence) {
            assert_eq!(pack_evidence.hash, git_evidence.hash);
            assert_eq!(pack_evidence.subject, git_evidence.subject);
            assert_eq!(pack_evidence.file_count, git_evidence.file_count);
            assert!((pack_evidence.weight - git_evidence.weight).abs() < 1e-12);
        }
    }
    assert!(
        pack_results
            .iter()
            .all(|item| !item.path.starts_with("bulk/"))
    );
    assert!(
        pack_results
            .iter()
            .any(|item| item.path == "src/c-renamed.md")
    );

    fs::remove_dir_all(repo).ok();
}

#[test]
fn pack_scan_backend_matches_git_for_merge_history() {
    let repo = new_test_repo();
    let base = "one\nkeep two\nkeep three\nkeep four\nfive\n";
    write_commit(&repo, "base", &[("a.md", base), ("b.md", base)]);
    let base_branch = git_output(&repo, &["branch", "--show-current"]);

    git(&repo, &["checkout", "-b", "side"]);
    let side = "side one\nkeep two\nkeep three\nkeep four\nfive\n";
    write_commit(&repo, "side", &[("a.md", side), ("b.md", side)]);

    git(&repo, &["checkout", &base_branch]);
    let main = "one\nkeep two\nkeep three\nkeep four\nmain five\n";
    write_commit(&repo, "main", &[("a.md", main), ("b.md", main)]);
    git(&repo, &["merge", "--no-ff", "side", "-m", "merge"]);
    git(&repo, &["gc", "--quiet"]);

    let mut config = OnDemandConfig {
        backend: OnDemandBackend::PackScan,
        backend_explicit: true,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 10,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let pack_results =
        git_pack_scan_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();
    config.backend = OnDemandBackend::GitCli;
    let git_results =
        git_log_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5).unwrap();

    assert_eq!(
        pack_results
            .iter()
            .map(|item| (item.path.as_str(), item.cochanges))
            .collect::<Vec<_>>(),
        git_results
            .iter()
            .map(|item| (item.path.as_str(), item.cochanges))
            .collect::<Vec<_>>()
    );
    assert_eq!(pack_results[0].cochanges, 3);
    assert_ne!(pack_results[0].evidence[0].subject, "merge");

    fs::remove_dir_all(repo).ok();
}

#[test]
fn pack_scan_backend_matches_git_from_linked_worktree() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    git(&repo, &["gc", "--quiet"]);

    let worktree = temp_dir();
    git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_str().unwrap(),
            "HEAD",
        ],
    );

    let mut config = OnDemandConfig {
        backend: OnDemandBackend::PackScan,
        backend_explicit: true,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 2,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let pack_results =
        git_pack_scan_direct_for_target(worktree.to_str().unwrap(), "a.md", &config, 5).unwrap();
    config.backend = OnDemandBackend::GitCli;
    let git_results =
        git_log_direct_for_target(worktree.to_str().unwrap(), "a.md", &config, 5).unwrap();

    assert_eq!(
        pack_results
            .iter()
            .map(|item| (item.path.as_str(), item.cochanges))
            .collect::<Vec<_>>(),
        git_results
            .iter()
            .map(|item| (item.path.as_str(), item.cochanges))
            .collect::<Vec<_>>()
    );
    assert_eq!(pack_results[0].evidence[0].subject, "pair two");

    git(
        &repo,
        &["worktree", "remove", "--force", worktree.to_str().unwrap()],
    );
    fs::remove_dir_all(repo).ok();
}

#[cfg(target_os = "linux")]
#[test]
fn pack_scan_backend_rejects_non_utf8_related_paths() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let repo = new_test_repo();
    fs::write(repo.join("a.md"), "a\n").unwrap();
    let invalid_path = OsString::from_vec(b"invalid-\xff.md".to_vec());
    fs::write(repo.join(invalid_path), "invalid\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "invalid utf8 companion"]);
    git(&repo, &["gc", "--quiet"]);

    let config = OnDemandConfig {
        backend: OnDemandBackend::PackScan,
        backend_explicit: true,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 0,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let error = git_pack_scan_direct_for_target(repo.to_str().unwrap(), "a.md", &config, 5)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Git path is not valid UTF-8"));

    fs::remove_dir_all(repo).ok();
}

fn new_test_repo() -> PathBuf {
    let repo = temp_dir();
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test User"]);
    repo
}

fn temp_dir() -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("related-test-{}-{id}", std::process::id()))
}

fn write_commit(repo: &Path, message: &str, files: &[(&str, &str)]) {
    for (path, content) in files {
        let full = repo.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", message]);
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout={}\nstderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed\nstdout={}\nstderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
