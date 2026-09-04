use super::support::{git, new_test_repo, write_commit};
use crate::BROAD_CHANGE_EXCLUDE_SUGGESTION;
use crate::commands::{explain_relationship, run_with_writer};
use crate::model::{OnDemandBackend, OnDemandConfig};
use std::fs;

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
