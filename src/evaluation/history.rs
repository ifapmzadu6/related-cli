use crate::AnyResult;
use crate::model::{Commit, RenameAwareCommit};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(crate) struct PreparedAuditHistory {
    pub(crate) train: Vec<Commit>,
    pub(crate) test: Vec<Commit>,
    pub(crate) training_renames: usize,
    pub(crate) test_diff_renames: usize,
}

pub(crate) fn prepare_rename_aware_audit_history(
    commits: &[RenameAwareCommit],
    test_commits: usize,
) -> AnyResult<PreparedAuditHistory> {
    if commits.len() <= test_commits {
        return Err(format!("not enough commits for evaluation: got {}", commits.len()).into());
    }
    let (test, train) = commits.split_at(test_commits);
    let mut aliases: HashMap<String, String> = HashMap::default();
    let mut training_renames = 0usize;
    for record in train {
        for rename in &record.renames {
            aliases.insert(rename.old_path.clone(), rename.new_path.clone());
            training_renames += 1;
        }
    }

    let train = train
        .iter()
        .map(|record| canonicalize_commit(&record.commit, &aliases, &HashMap::default()))
        .collect();
    let mut test_diff_renames = 0usize;
    let test = test
        .iter()
        .map(|record| {
            let mut current_diff_aliases = HashMap::default();
            for rename in &record.renames {
                current_diff_aliases.insert(
                    rename.new_path.clone(),
                    canonical_path(&rename.old_path, &aliases),
                );
                test_diff_renames += 1;
            }
            canonicalize_commit(&record.commit, &aliases, &current_diff_aliases)
        })
        .collect();
    Ok(PreparedAuditHistory {
        train,
        test,
        training_renames,
        test_diff_renames,
    })
}

fn canonicalize_commit(
    commit: &Commit,
    aliases: &HashMap<String, String>,
    current_diff_aliases: &HashMap<String, String>,
) -> Commit {
    let mut commit = commit.clone();
    let mut seen = HashSet::default();
    commit.files = commit
        .files
        .iter()
        .map(|file| {
            current_diff_aliases
                .get(file)
                .cloned()
                .unwrap_or_else(|| canonical_path(file, aliases))
        })
        .filter(|file| seen.insert(file.clone()))
        .collect();
    commit
}

fn canonical_path(path: &str, aliases: &HashMap<String, String>) -> String {
    let mut current = path;
    let mut seen = HashSet::default();
    while let Some(next) = aliases.get(current) {
        if !seen.insert(current) {
            break;
        }
        current = next;
    }
    current.to_string()
}
