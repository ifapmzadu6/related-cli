use super::support::{git, new_test_repo, write_commit};
use crate::commands::run_with_writer;
use std::fs;

#[test]
fn top_level_help_focuses_on_omission_auditing() {
    let mut output = Vec::new();
    run_with_writer(vec!["--help".to_string()], &mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("related audit"));
    assert!(text.contains("related eval"));
    assert!(text.contains("changed-set omission"));
    assert!(!text.contains("related query"));
    assert!(!text.contains("related explain"));
    assert!(!text.contains("related diff"));
}

#[test]
fn worktree_audit_includes_staged_changes_and_preserves_rename_history() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    fs::write(repo.join("a.md"), "staged\n").unwrap();
    git(&repo, &["add", "a.md"]);

    let audit = |accuracy: &str| {
        let mut output = Vec::new();
        run_with_writer(
            vec![
                "audit".into(),
                "--repo".into(),
                repo.display().to_string(),
                "--accuracy".into(),
                accuracy.into(),
                "--format".into(),
                "json".into(),
            ],
            &mut output,
        )
        .unwrap();
        serde_json::from_slice::<serde_json::Value>(&output).unwrap()
    };
    for accuracy in ["fast", "exact"] {
        let result = audit(accuracy);
        assert_eq!(result["seeds"], serde_json::json!(["a.md"]));
        assert_eq!(result["candidates"][0]["path"], "b.md");
    }

    git(&repo, &["checkout", "HEAD", "--", "a.md"]);
    git(&repo, &["mv", "a.md", "renamed.md"]);
    fs::write(
        repo.join("renamed.md"),
        "unstaged edit after staged rename\n",
    )
    .unwrap();
    fs::write(repo.join("new.md"), "untracked\n").unwrap();
    for accuracy in ["fast", "exact"] {
        let result = audit(accuracy);
        assert_eq!(result["seeds"], serde_json::json!(["new.md", "renamed.md"]));
        assert_eq!(result["candidates"][0]["path"], "b.md");
        assert_eq!(result["candidates"][0]["cochanges"], 2);
        assert_eq!(
            result["candidates"][0]["supported_by"],
            serde_json::json!(["renamed.md"])
        );
    }
    // A staged companion must be counted as changed, not reported as omitted.
    fs::write(repo.join("b.md"), "staged companion\n").unwrap();
    git(&repo, &["add", "b.md"]);
    for accuracy in ["fast", "exact"] {
        let result = audit(accuracy);
        assert_eq!(
            result["seeds"],
            serde_json::json!(["b.md", "new.md", "renamed.md"])
        );
        assert_eq!(result["candidates"], serde_json::json!([]));
    }
    fs::remove_dir_all(repo).ok();
}

#[test]
fn audit_excludes_deleted_candidates_but_respects_historical_range_endpoints() {
    let repo = new_test_repo();
    for i in 0..25 {
        let content = format!("revision {i}\n");
        write_commit(
            &repo,
            "paired change",
            &[("a.md", &content), ("b.md", &content)],
        );
    }
    git(&repo, &["tag", "audit-base"]);
    write_commit(&repo, "historical change", &[("a.md", "historical\n")]);
    git(&repo, &["tag", "audit-head"]);
    git(&repo, &["rm", "b.md"]);
    git(&repo, &["commit", "-m", "delete companion"]);
    fs::write(repo.join("a.md"), "current change\n").unwrap();
    git(&repo, &["add", "a.md"]);

    for accuracy in ["fast", "exact"] {
        for scope in [
            vec![],
            vec!["--staged"],
            vec!["--range", "audit-base..HEAD"],
            vec!["--range", "audit-base.."],
            vec!["--range", "audit-base...HEAD"],
            vec!["--range", "audit-base..audit-head"],
            vec!["--range", "audit-base...audit-head"],
        ] {
            let historical = scope
                .last()
                .is_some_and(|value| value.ends_with("audit-head"));
            let mut args = vec![
                "audit".into(),
                "--repo".into(),
                repo.display().to_string(),
                "--accuracy".into(),
                accuracy.into(),
                "--format".into(),
                "json".into(),
                "--fail-on-confidence".into(),
                "high".into(),
            ];
            args.extend(scope.iter().map(|value| value.to_string()));
            let mut output = Vec::new();
            let result = run_with_writer(args, &mut output);
            let audit: serde_json::Value = serde_json::from_slice(&output).unwrap();
            if historical {
                assert_eq!(crate::exit_code_for_error(result.unwrap_err().as_ref()), 3);
                assert_eq!(audit["candidates"][0]["path"], "b.md");
                assert_eq!(audit["candidates"][0]["confidence"], "high");
            } else {
                result.unwrap();
                assert_eq!(
                    audit["candidates"],
                    serde_json::json!([]),
                    "{accuracy} {scope:?}"
                );
                assert_eq!(audit["enforcement"]["triggered"], false);
            }
        }
    }
    fs::remove_dir_all(repo).ok();
}

#[test]
fn audit_includes_untracked_seeds_and_abstains_from_weak_candidates() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    write_commit(&repo, "one-off", &[("a.md", "a3\n"), ("weak.md", "w1\n")]);
    fs::write(repo.join("a.md"), "worktree\n").unwrap();
    fs::write(repo.join("new.md"), "untracked\n").unwrap();

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "audit".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let audit: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(audit["scope"], "worktree");
    assert_eq!(audit["seeds"], serde_json::json!(["a.md", "new.md"]));
    assert_eq!(audit["candidates"][0]["path"], "b.md");
    assert!(
        audit["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|candidate| candidate["path"] != "weak.md")
    );
    assert_eq!(audit["abstained"], false);
    assert!(
        audit["hints"][0]
            .as_str()
            .unwrap()
            .contains("lower-confidence")
    );

    let mut enforced_output = Vec::new();
    let finding = run_with_writer(
        vec![
            "audit".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--fail-on-confidence".to_string(),
            "medium".to_string(),
        ],
        &mut enforced_output,
    )
    .unwrap_err();
    assert_eq!(crate::exit_code_for_error(finding.as_ref()), 3);
    let enforced: serde_json::Value = serde_json::from_slice(&enforced_output).unwrap();
    assert_eq!(enforced["enforcement"]["threshold"], "medium");
    assert_eq!(enforced["enforcement"]["finding_count"], 1);
    assert_eq!(enforced["enforcement"]["triggered"], true);
    assert_eq!(enforced["enforcement"]["exit_code"], 3);

    let mut no_finding_output = Vec::new();
    run_with_writer(
        vec![
            "audit".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--fail-on-confidence".to_string(),
            "high".to_string(),
        ],
        &mut no_finding_output,
    )
    .unwrap();
    let no_finding: serde_json::Value = serde_json::from_slice(&no_finding_output).unwrap();
    assert_eq!(no_finding["enforcement"]["triggered"], false);

    let invalid_threshold = run_with_writer(
        vec![
            "audit".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--min-confidence".to_string(),
            "high".to_string(),
            "--fail-on-confidence".to_string(),
            "medium".to_string(),
        ],
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(invalid_threshold.contains("cannot be lower"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn audit_supports_revision_ranges_and_public_accuracy_levels() {
    let repo = new_test_repo();
    write_commit(&repo, "pair one", &[("a.md", "a1\n"), ("b.md", "b1\n")]);
    write_commit(&repo, "pair two", &[("a.md", "a2\n"), ("b.md", "b2\n")]);
    write_commit(&repo, "only a", &[("a.md", "a3\n")]);

    let mut output = Vec::new();
    run_with_writer(
        vec![
            "audit".to_string(),
            "--range".to_string(),
            "HEAD~1..HEAD".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.starts_with("audit scope=range:HEAD~1..HEAD"));
    assert!(text.contains("1 b.md confidence=medium"));
    assert!(text.contains("supported_by a.md"));

    let conflict = run_with_writer(
        vec![
            "query".to_string(),
            "a.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "exact".to_string(),
            "--history-backend".to_string(),
            "git".to_string(),
        ],
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(conflict.contains("cannot be used together"));

    fs::remove_dir_all(repo).ok();
}

#[test]
fn staged_rename_audit_uses_the_old_path_history_in_fast_and_exact_modes() {
    let repo = new_test_repo();
    write_commit(
        &repo,
        "pair before staged rename one",
        &[
            ("src/old.md", "old 1\n"),
            ("tests/companion.md", "test 1\n"),
        ],
    );
    write_commit(
        &repo,
        "pair before staged rename two",
        &[
            ("src/old.md", "old 2\n"),
            ("tests/companion.md", "test 2\n"),
        ],
    );
    git(&repo, &["mv", "src/old.md", "src/new.md"]);

    for (accuracy, expected_tracking) in [
        ("fast", "exact-blob-renames+diff-renames"),
        ("exact", "git-follow+diff-renames"),
    ] {
        let mut output = Vec::new();
        run_with_writer(
            vec![
                "audit".to_string(),
                "--staged".to_string(),
                "--repo".to_string(),
                repo.display().to_string(),
                "--accuracy".to_string(),
                accuracy.to_string(),
                "--format".to_string(),
                "json".to_string(),
            ],
            &mut output,
        )
        .unwrap();
        let audit: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(audit["seeds"], serde_json::json!(["src/new.md"]));
        assert_eq!(audit["candidates"][0]["path"], "tests/companion.md");
        assert_eq!(audit["candidates"][0]["cochanges"], 2);
        assert_eq!(
            audit["candidates"][0]["supported_by"],
            serde_json::json!(["src/new.md"])
        );
        assert_eq!(
            audit["history_coverage"]["rename_tracking"],
            expected_tracking
        );
        assert!(
            audit["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|candidate| candidate["path"] != "src/old.md")
        );
    }

    fs::remove_dir_all(repo).ok();
}
