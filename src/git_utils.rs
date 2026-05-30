use crate::AnyResult;
use crate::path_utils::{literal_pathspec, normalize_git_path};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub(crate) fn run_git(repo: impl AsRef<Path>, args: &[&str]) -> AnyResult<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo.as_ref())
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "git -C {} {} failed: {}\n{}{}",
            repo.as_ref().display(),
            args.join(" "),
            output.status,
            stdout.trim(),
            stderr.trim()
        )
        .into())
    }
}

pub(crate) fn run_git_with_stdin(
    repo: impl AsRef<Path>,
    args: &[&str],
    input: &[u8],
) -> AnyResult<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo.as_ref())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or("failed to open git stdin")?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "git -C {} {} failed: {}\n{}{}",
            repo.as_ref().display(),
            args.join(" "),
            output.status,
            stdout.trim(),
            stderr.trim()
        )
        .into())
    }
}

pub(crate) fn git_path_is_tracked(repo: &str, path: &str) -> AnyResult<bool> {
    if path.is_empty() {
        return Ok(false);
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(literal_pathspec(path))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(output.success())
}

pub(crate) fn git_diff_names(repo: &str, staged: bool) -> AnyResult<Vec<String>> {
    let args = if staged {
        vec!["diff", "--name-only", "--cached"]
    } else {
        vec!["diff", "--name-only"]
    };
    let out = run_git(repo, &args)?;
    Ok(String::from_utf8(out)?
        .lines()
        .map(normalize_git_path)
        .filter(|path| !path.is_empty())
        .collect())
}
