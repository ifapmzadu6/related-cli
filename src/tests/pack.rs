use super::support::{git, git_output, new_test_repo, temp_dir, write_commit};
use crate::commands::run_with_writer;
use crate::history::git_log_direct_for_target;
use crate::model::{OnDemandBackend, OnDemandConfig};
use crate::pack::{git_pack_fast_direct_for_target, git_pack_scan_direct_for_target};
use std::fs;

#[test]
fn pack_fast_follows_unambiguous_exact_blob_renames() {
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
    git(&repo, &["mv", "old.md", "middle.md"]);
    fs::write(repo.join("companion.md"), "three\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "first pure rename with companion"]);
    git(&repo, &["mv", "middle.md", "new.md"]);
    fs::write(repo.join("companion.md"), "four\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(
        &repo,
        &["commit", "-m", "second pure rename with companion"],
    );
    write_commit(
        &repo,
        "new pair",
        &[
            ("new.md", &format!("{old}after\n")),
            ("companion.md", "five\n"),
        ],
    );

    let config = OnDemandConfig {
        backend: OnDemandBackend::PackFast,
        backend_explicit: false,
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
        git_pack_fast_direct_for_target(repo.to_str().unwrap(), "new.md", &config, 10).unwrap();
    let companion = results
        .iter()
        .find(|result| result.path == "companion.md")
        .unwrap();
    assert_eq!(companion.cochanges, 5);
    assert!(results.iter().all(|result| result.path != "old.md"));

    let mut pagerank_output = Vec::new();
    run_with_writer(
        vec![
            "query".to_string(),
            "new.md".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "fast".to_string(),
            "--mode".to_string(),
            "pagerank".to_string(),
            "--top".to_string(),
            "10".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
        &mut pagerank_output,
    )
    .unwrap();
    let pagerank: serde_json::Value = serde_json::from_slice(&pagerank_output).unwrap();
    let companion = pagerank["related"]
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["path"] == "companion.md")
        .unwrap();
    assert_eq!(companion["cochanges"], 5);
    assert!(
        pagerank["related"]
            .as_array()
            .unwrap()
            .iter()
            .all(|result| result["path"] != "old.md")
    );

    fs::write(repo.join("new.md"), "staged\n").unwrap();
    git(&repo, &["add", "new.md"]);
    let mut output = Vec::new();
    run_with_writer(
        vec![
            "audit".to_string(),
            "--staged".to_string(),
            "--repo".to_string(),
            repo.display().to_string(),
            "--accuracy".to_string(),
            "fast".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
        &mut output,
    )
    .unwrap();
    let audit: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(audit["candidates"][0]["path"], "companion.md");
    assert_eq!(audit["candidates"][0]["cochanges"], 5);
    assert_eq!(
        audit["history_coverage"]["rename_tracking"],
        "exact-blob-renames"
    );

    fs::remove_dir_all(repo).ok();
}

#[test]
fn pack_fast_abstains_from_ambiguous_exact_blob_sources() {
    let repo = new_test_repo();
    let shared = "same content\nacross both paths\n";
    write_commit(
        &repo,
        "ambiguous pair one",
        &[
            ("old-one.md", shared),
            ("old-two.md", shared),
            ("companion.md", "one\n"),
        ],
    );
    write_commit(
        &repo,
        "ambiguous pair two",
        &[
            ("old-one.md", shared),
            ("old-two.md", shared),
            ("companion.md", "two\n"),
        ],
    );
    git(&repo, &["mv", "old-one.md", "new.md"]);
    git(&repo, &["rm", "old-two.md"]);
    fs::write(repo.join("companion.md"), "three\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "ambiguous rename sources"]);

    let config = OnDemandConfig {
        backend: OnDemandBackend::PackFast,
        backend_explicit: false,
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
        git_pack_fast_direct_for_target(repo.to_str().unwrap(), "new.md", &config, 10).unwrap();
    let companion = results
        .iter()
        .find(|result| result.path == "companion.md")
        .unwrap();
    assert_eq!(companion.cochanges, 1);

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

    let mut candidates = [
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
