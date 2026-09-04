use super::support::{git, new_test_repo, temp_dir, write_commit};
use crate::commands::run_with_writer;
use crate::history::git_log_direct_for_target;
use crate::model::{OnDemandBackend, OnDemandConfig};
use std::fs;
use std::process::Command;

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
fn exact_history_follows_target_renames_for_query_and_audit() {
    let repo = new_test_repo();
    let old_v1 = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
    let old_v2 = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten v2\n";
    write_commit(
        &repo,
        "pair before rename one",
        &[("src/old.md", old_v1), ("tests/companion.md", "test 1\n")],
    );
    write_commit(
        &repo,
        "pair before rename two",
        &[("src/old.md", old_v2), ("tests/companion.md", "test 2\n")],
    );

    git(&repo, &["mv", "src/old.md", "src/new.md"]);
    fs::write(repo.join("src/new.md"), format!("{old_v2}eleven\n")).unwrap();
    fs::write(repo.join("tests/companion.md"), "test 3\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "rename target with companion"]);
    write_commit(
        &repo,
        "pair after rename",
        &[
            ("src/new.md", &format!("{old_v2}eleven\ntwelve\n")),
            ("tests/companion.md", "test 4\n"),
        ],
    );

    let config = OnDemandConfig {
        backend: OnDemandBackend::GitCli,
        backend_explicit: false,
        max_commits: 20,
        since: None,
        max_files_per_commit: 10,
        half_life_days: 365.0,
        evidence_limit: 5,
        jobs: 1,
        jobs_explicit: false,
        scan_commits: 0,
    };
    let results =
        git_log_direct_for_target(repo.to_str().unwrap(), "src/new.md", &config, 10).unwrap();
    let companion = results
        .iter()
        .find(|item| item.path == "tests/companion.md")
        .unwrap();
    assert_eq!(companion.cochanges, 4);
    assert_eq!(companion.evidence.len(), 4);
    assert!(results.iter().all(|item| item.path != "src/old.md"));

    let mut pagerank_output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "src/new.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
            "--mode".to_string(),
            "pagerank".to_string(),
        ],
        &mut pagerank_output,
    )
    .unwrap();
    let pagerank_text = String::from_utf8(pagerank_output).unwrap();
    assert!(pagerank_text.contains("tests/companion.md co=4"));
    assert!(!pagerank_text.contains("src/old.md"));

    fs::write(repo.join("src/new.md"), "staged audit change\n").unwrap();
    git(&repo, &["add", "src/new.md"]);
    let mut audit_output = Vec::new();
    run_with_writer(
        vec![
            "audit".to_string(),
            "--staged".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
        &mut audit_output,
    )
    .unwrap();
    let audit: serde_json::Value = serde_json::from_slice(&audit_output).unwrap();
    assert_eq!(audit["candidates"][0]["path"], "tests/companion.md");
    assert_eq!(audit["candidates"][0]["cochanges"], 4);
    assert_eq!(
        audit["candidates"][0]["supported_by"],
        serde_json::json!(["src/new.md"])
    );
    assert_eq!(audit["history_coverage"]["rename_tracking"], "git-follow");

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
