use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

pub(super) fn new_test_repo() -> PathBuf {
    let repo = temp_dir();
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test User"]);
    repo
}

pub(super) fn temp_dir() -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("related-test-{}-{id}", std::process::id()))
}

pub(super) fn write_commit(repo: &Path, message: &str, files: &[(&str, &str)]) {
    for (path, content) in files {
        let full = repo.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    git(repo, &["add", "."]);
    git(repo, &["commit", "-m", message]);
}

pub(super) fn git(repo: &Path, args: &[&str]) {
    checked_git(repo, args);
}

pub(super) fn git_output(repo: &Path, args: &[&str]) -> String {
    String::from_utf8(checked_git(repo, args).stdout)
        .unwrap()
        .trim()
        .to_string()
}

fn checked_git(repo: &Path, args: &[&str]) -> std::process::Output {
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
    output
}
