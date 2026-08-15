use crate::BROAD_CHANGE_EXCLUDE_SUGGESTION;
use crate::cli::{ParsedArgs, flag_optional_string};
use crate::graph::truncate_top_results;
use crate::model::ResultItem;
use crate::path_utils::path_basename;

pub(crate) fn parse_exclude_patterns(parsed: &ParsedArgs) -> Vec<String> {
    let Some(value) = flag_optional_string(parsed, "exclude") else {
        return Vec::new();
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub(crate) fn filtered_query_top(top: usize, exclude_patterns: &[String]) -> usize {
    if exclude_patterns.is_empty() {
        top
    } else {
        // Every query backend has already collected its candidate set before it
        // applies the top-K truncation. Keep that full set when exclusions are
        // present so filtering cannot hide a valid lower-ranked result.
        usize::MAX
    }
}

pub(crate) fn filter_related_results(
    results: &mut Vec<ResultItem>,
    exclude_patterns: &[String],
    top: usize,
) {
    if !exclude_patterns.is_empty() {
        results.retain(|item| !path_matches_any_pattern(&item.path, exclude_patterns));
    }
    truncate_top_results(results, top);
}

pub(crate) fn query_hints(results: &[ResultItem], exclude_patterns: &[String]) -> Vec<String> {
    let mut hints = Vec::new();
    let window = results.len().min(8);
    if window == 0 {
        return hints;
    }
    let broad_change_results = results
        .iter()
        .take(window)
        .filter(|item| looks_like_broad_change_path(&item.path))
        .count();
    if broad_change_results >= 4 && broad_change_results * 2 >= window {
        if exclude_patterns.is_empty() {
            hints.push(format!(
                "Top results include several lockfile, manifest, workflow, or release-doc paths. Retry with --max-files-per-commit 10 --exclude '{BROAD_CHANGE_EXCLUDE_SUGGESTION}' --evidence 3 before opening many files."
            ));
        } else {
            hints.push(
                "Top results still look broad-change heavy. Inspect --evidence 3 or lower --max-files-per-commit further."
                    .to_string(),
            );
        }
    }
    hints
}

fn path_matches_any_pattern(path: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| path_matches_pattern(path, pattern))
}

fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    if pattern.contains('*') {
        return wildcard_match(pattern, path) || wildcard_match(pattern, path_basename(path));
    }
    path == pattern || path.ends_with(&format!("/{pattern}")) || path_basename(path) == pattern
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let mut offset = 0usize;

    for (idx, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if idx == 0 && anchored_start {
            let Some(rest) = text.get(offset..) else {
                return false;
            };
            if !rest.starts_with(part) {
                return false;
            }
            offset += part.len();
            continue;
        }
        let Some(rest) = text.get(offset..) else {
            return false;
        };
        let Some(found) = rest.find(part) else {
            return false;
        };
        offset += found + part.len();
    }

    if anchored_end && let Some(last) = parts.iter().rev().find(|part| !part.is_empty()) {
        return text.ends_with(last);
    }
    true
}

fn looks_like_broad_change_path(path: &str) -> bool {
    let basename = path_basename(path);
    matches!(
        basename,
        "Cargo.lock"
            | "Cargo.toml"
            | "package-lock.json"
            | "package.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lockb"
            | "poetry.lock"
            | "uv.lock"
            | "README.md"
            | "CHANGELOG.md"
            | "rust-toolchain.toml"
    ) || path.starts_with(".github/workflows/")
}
