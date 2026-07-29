use crate::AnyResult;
use crate::git_utils::run_git;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct RepoContext {
    pub(crate) root: PathBuf,
    pub(crate) input_base: PathBuf,
}

impl RepoContext {
    pub(crate) fn discover(repo: &str) -> AnyResult<Self> {
        let input_base = fs::canonicalize(repo)?;
        let root = if looks_like_worktree_root(&input_base) {
            input_base.clone()
        } else {
            let out = run_git(&input_base, &["rev-parse", "--show-toplevel"])?;
            let root = std::str::from_utf8(&out)?.trim_end_matches(['\r', '\n']);
            PathBuf::from(root)
        };
        Ok(Self { root, input_base })
    }

    pub(crate) fn root_str(&self) -> AnyResult<&str> {
        self.root.to_str().ok_or_else(|| {
            format!(
                "repository path is not valid UTF-8: {}",
                self.root.display()
            )
            .into()
        })
    }

    pub(crate) fn object_format(&self) -> AnyResult<String> {
        let out = run_git(&self.root, &["rev-parse", "--show-object-format"])?;
        Ok(std::str::from_utf8(&out)?.trim().to_string())
    }
}

fn looks_like_worktree_root(path: &Path) -> bool {
    let git = path.join(".git");
    if git.is_file() {
        return true;
    }
    git.is_dir() && git.join("HEAD").is_file()
}
