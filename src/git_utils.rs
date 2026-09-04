use crate::AnyResult;
use crate::path_utils::{decode_git_path, literal_pathspec};
use rustc_hash::FxHashSet as HashSet;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;

const MAX_GIT_STDOUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_GIT_STDERR_BYTES: usize = 1024 * 1024;

struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

pub(crate) fn run_git(repo: impl AsRef<Path>, args: &[&str]) -> AnyResult<Vec<u8>> {
    run_git_bounded(repo.as_ref(), args, None)
}

pub(crate) fn run_git_with_stdin(
    repo: impl AsRef<Path>,
    args: &[&str],
    input: &[u8],
) -> AnyResult<Vec<u8>> {
    run_git_bounded(repo.as_ref(), args, Some(input))
}

fn run_git_bounded(repo: &Path, args: &[&str], input: Option<&[u8]>) -> AnyResult<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().ok_or("failed to open git stdout")?;
    let stderr = child.stderr.take().ok_or("failed to open git stderr")?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, MAX_GIT_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_GIT_STDERR_BYTES));

    let input_result = if let Some(input) = input {
        let mut stdin = child.stdin.take().ok_or("failed to open git stdin")?;
        stdin.write_all(input)
    } else {
        Ok(())
    };
    let status = child.wait();
    let stdout = stdout_reader
        .join()
        .map_err(|_| "git stdout reader panicked")??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "git stderr reader panicked")??;
    input_result?;
    let status = status?;

    if stdout.exceeded {
        return Err(format!(
            "git -C {} {} produced more than {} MiB of output; narrow the history request",
            repo.display(),
            args.join(" "),
            MAX_GIT_STDOUT_BYTES / (1024 * 1024)
        )
        .into());
    }
    if status.success() {
        Ok(stdout.bytes)
    } else {
        let stderr_text = String::from_utf8_lossy(&stderr.bytes);
        let stdout_text = String::from_utf8_lossy(&stdout.bytes);
        let stderr_suffix = if stderr.exceeded {
            "\n[git stderr truncated]"
        } else {
            ""
        };
        Err(format!(
            "git -C {} {} failed: {}\n{}{}",
            repo.display(),
            args.join(" "),
            status,
            stdout_text.trim(),
            format_args!("{}{stderr_suffix}", stderr_text.trim())
        )
        .into())
    }
}

fn read_bounded(mut reader: impl Read, limit: usize) -> io::Result<BoundedOutput> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0u8; 16 * 1024];
    let mut exceeded = false;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let keep = read.min(limit.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..keep]);
        exceeded |= keep < read;
    }
    Ok(BoundedOutput { bytes, exceeded })
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
    Ok(git_diff_audit_paths(repo, staged)?
        .into_iter()
        .map(|path| path.path)
        .collect())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuditPath {
    pub(crate) path: String,
    pub(crate) history_path: String,
}

pub(crate) fn git_diff_audit_paths(repo: &str, staged: bool) -> AnyResult<Vec<AuditPath>> {
    let args: Vec<&str> = if staged {
        vec![
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            "--cached",
            "--",
        ]
    } else {
        vec!["diff", "--name-status", "-z", "--find-renames", "--"]
    };
    parse_audit_paths(&run_git(repo, &args)?, true)
}

pub(crate) fn git_worktree_audit_paths(repo: &str) -> AnyResult<Vec<AuditPath>> {
    let mut paths = git_diff_audit_paths(repo, true)?;
    for mut path in git_diff_audit_paths(repo, false)? {
        // The unstaged diff is relative to the index. Preserve the HEAD path
        // when an indexed rename is edited or renamed again in the worktree.
        if let Some(index) = paths
            .iter()
            .position(|staged| staged.path == path.history_path)
        {
            path.history_path = paths.remove(index).history_path;
        }
        paths.push(path);
    }
    let out = run_git(
        repo,
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
    )?;
    for raw_path in out.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        let path = decode_git_path(raw_path)?;
        if !path.is_empty() {
            paths.push(AuditPath {
                history_path: path.clone(),
                path,
            });
        }
    }
    paths.sort_by(|left, right| left.path.cmp(&right.path));
    paths.dedup_by(|left, right| left.path == right.path);
    Ok(paths)
}

pub(crate) fn git_audit_candidate_paths(
    repo: &str,
    range: Option<&str>,
) -> AnyResult<HashSet<String>> {
    let out = if let Some((_, endpoint)) = range.and_then(|range| range.rsplit_once("..")) {
        // Both A..B and A...B describe changes ending at B. An omitted B is HEAD.
        let endpoint = if endpoint.is_empty() {
            "HEAD"
        } else {
            endpoint
        };
        let tree = run_git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{endpoint}^{{tree}}"),
            ],
        )?;
        let tree = std::str::from_utf8(&tree)?.trim();
        run_git(repo, &["ls-tree", "-r", "--name-only", "-z", tree, "--"])?
    } else {
        // Index membership also works with sparse checkouts and broken symlinks.
        // Worktree deletions are already excluded through the changed set.
        run_git(repo, &["ls-files", "--cached", "-z", "--"])?
    };
    out.split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(decode_git_path)
        .collect()
}

pub(crate) fn git_diff_audit_paths_for_range(repo: &str, range: &str) -> AnyResult<Vec<AuditPath>> {
    if range.is_empty() || range.starts_with('-') || range.chars().any(char::is_whitespace) {
        return Err("--range must be a non-empty Git revision range without whitespace".into());
    }
    let out = run_git(
        repo,
        &["diff", "--name-status", "-z", "--find-renames", range, "--"],
    )?;
    parse_audit_paths(&out, false)
}

fn parse_audit_paths(out: &[u8], use_rename_source: bool) -> AnyResult<Vec<AuditPath>> {
    let tokens: Vec<&[u8]> = out
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .collect();
    let mut paths = Vec::new();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let status = tokens[idx];
        idx += 1;
        match status.first() {
            Some(b'R') => {
                let old = tokens.get(idx).ok_or("truncated rename source")?;
                let new = tokens.get(idx + 1).ok_or("truncated rename destination")?;
                idx += 2;
                let old = decode_git_path(old)?;
                let new = decode_git_path(new)?;
                if !new.is_empty() {
                    paths.push(AuditPath {
                        history_path: if use_rename_source { old } else { new.clone() },
                        path: new,
                    });
                }
            }
            Some(b'C') => {
                let _source = tokens.get(idx).ok_or("truncated copy source")?;
                let new = tokens.get(idx + 1).ok_or("truncated copy destination")?;
                idx += 2;
                let new = decode_git_path(new)?;
                if !new.is_empty() {
                    paths.push(AuditPath {
                        history_path: new.clone(),
                        path: new,
                    });
                }
            }
            Some(b'A' | b'D' | b'M' | b'T' | b'U') => {
                let raw_path = tokens.get(idx).ok_or("truncated changed path")?;
                idx += 1;
                let path = decode_git_path(raw_path)?;
                if !path.is_empty() {
                    paths.push(AuditPath {
                        history_path: path.clone(),
                        path,
                    });
                }
            }
            _ => {
                return Err(format!(
                    "unsupported git diff name-status token {:?}",
                    String::from_utf8_lossy(status)
                )
                .into());
            }
        }
    }
    paths.sort_by(|left, right| left.path.cmp(&right.path));
    paths.dedup_by(|left, right| left.path == right.path);
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_reader_keeps_the_prefix_and_drains_the_rest() {
        let exact = read_bounded(Cursor::new(b"abcd"), 4).unwrap();
        assert_eq!(exact.bytes, b"abcd");
        assert!(!exact.exceeded);

        let truncated = read_bounded(Cursor::new(b"abcdef"), 4).unwrap();
        assert_eq!(truncated.bytes, b"abcd");
        assert!(truncated.exceeded);
    }
}
