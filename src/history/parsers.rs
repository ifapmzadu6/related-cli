//! Git output decoding and rename-path canonicalization.

use crate::AnyResult;
use crate::graph::{direct_pair_result, time_decay, truncate_top_direct_pairs};
use crate::model::{
    Commit, DirectPairStat, DirectScoredPair, Evidence, GixCommitSeed, HistoryRename,
    OnDemandConfig, RenameAwareCommit, ResultItem, direct_pair_capacity,
};
use crate::path_utils::decode_git_path;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

struct GitLogRecord<'a> {
    header: &'a str,
    file_bytes: &'a [u8],
}

pub(super) struct GitFollowHistory {
    pub(super) hash_input: Vec<u8>,
    pub(super) hashes: Vec<String>,
    pub(super) target_paths_by_hash: HashMap<String, HashSet<String>>,
}

pub(super) fn parse_commit_seeds(input: &[u8]) -> AnyResult<Vec<GixCommitSeed>> {
    std::str::from_utf8(input)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            Ok(GixCommitSeed {
                id: gix::hash::ObjectId::from_hex(line.trim().as_bytes())?,
                first_parent: None,
            })
        })
        .collect()
}

fn git_log_records(out: &[u8]) -> impl Iterator<Item = AnyResult<GitLogRecord<'_>>> {
    let mut remaining = out;
    std::iter::from_fn(move || {
        // Git inserts NUL between commits, in addition to our header framing.
        let start = remaining.iter().position(|byte| *byte != 0)?;
        remaining = &remaining[start..];
        let Some(header_end) = remaining.iter().position(|byte| *byte == 0) else {
            remaining = &[];
            return Some(Err("unterminated git log header".into()));
        };
        let header = &remaining[..header_end];
        remaining = &remaining[header_end + 1..];
        // Strip exactly Git's header/diff separator, preserving a leading LF
        // in the first path. Subsequent paths are already separated by NUL.
        remaining = remaining.strip_prefix(b"\n").unwrap_or(remaining);
        // An empty NUL token ends the paths, also covering empty commits.
        let file_end = remaining
            .split_inclusive(|byte| *byte == 0)
            .take_while(|token| *token != b"\0")
            .map(|token| token.len())
            .sum::<usize>();
        let file_bytes = &remaining[..file_end];
        remaining = &remaining[file_end..];
        Some(
            std::str::from_utf8(header)
                .map(|header| GitLogRecord { header, file_bytes })
                .map_err(Into::into),
        )
    })
}

fn parse_git_log_files(file_bytes: &[u8]) -> AnyResult<Vec<String>> {
    let mut files = Vec::new();
    for raw_path in file_bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = decode_git_path(raw_path)?;
        if !path.is_empty() {
            files.push(path);
        }
    }
    Ok(files)
}

pub(super) fn parse_git_follow_history(out: &[u8]) -> AnyResult<GitFollowHistory> {
    let mut hash_input = Vec::new();
    let mut hashes = Vec::new();
    let mut target_paths_by_hash = HashMap::default();
    for record in git_log_records(out) {
        let record = record?;
        let hash = record.header;
        if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid followed commit hash {hash:?}").into());
        }
        hash_input.extend_from_slice(hash.as_bytes());
        hash_input.push(b'\n');
        hashes.push(hash.to_string());

        let tokens: Vec<&[u8]> = record
            .file_bytes
            .split(|byte| *byte == 0)
            .filter(|token| !token.is_empty())
            .collect();
        let mut idx = 0usize;
        let mut target_paths = HashSet::default();
        while idx < tokens.len() {
            let status = tokens[idx];
            idx += 1;
            let path_count = if matches!(status.first(), Some(b'R' | b'C')) {
                2
            } else if matches!(status.first(), Some(b'A' | b'M' | b'T')) {
                1
            } else {
                return Err(format!(
                    "unsupported followed name-status token {:?}",
                    String::from_utf8_lossy(status)
                )
                .into());
            };
            if idx.saturating_add(path_count) > tokens.len() {
                return Err("truncated followed name-status record".into());
            }
            for raw_path in &tokens[idx..idx + path_count] {
                let path = decode_git_path(raw_path)?;
                if !path.is_empty() {
                    target_paths.insert(path);
                }
            }
            idx += path_count;
        }
        target_paths_by_hash.insert(hash.to_string(), target_paths);
    }
    Ok(GitFollowHistory {
        hash_input,
        hashes,
        target_paths_by_hash,
    })
}

pub(super) fn parse_git_log(out: &[u8]) -> AnyResult<Vec<Commit>> {
    let mut commits = Vec::new();
    for record in git_log_records(out) {
        let record = record?;
        let mut fields = record.header.splitn(4, '\x1f');
        let hash = fields.next().ok_or("missing commit hash")?.to_string();
        let unix_time: i64 = fields
            .next()
            .ok_or("missing commit unix time")?
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let date = normalize_git_iso8601_date(fields.next().ok_or("missing commit date")?);
        let subject = fields.next().unwrap_or_default().to_string();
        let mut seen = HashSet::default();
        let mut files = Vec::new();
        for file in parse_git_log_files(record.file_bytes)? {
            if file.is_empty() || !seen.insert(file.clone()) {
                continue;
            }
            files.push(file);
        }
        commits.push(Commit {
            hash,
            unix_time,
            date,
            subject,
            files,
        });
    }
    Ok(commits)
}

pub(super) fn parse_git_log_rename_aware(out: &[u8]) -> AnyResult<Vec<RenameAwareCommit>> {
    let mut commits = Vec::new();
    for record in git_log_records(out) {
        let record = record?;
        let mut fields = record.header.splitn(4, '\x1f');
        let hash = fields.next().ok_or("missing commit hash")?.to_string();
        let unix_time: i64 = fields
            .next()
            .ok_or("missing commit unix time")?
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let date = normalize_git_iso8601_date(fields.next().ok_or("missing commit date")?);
        let subject = fields.next().unwrap_or_default().to_string();

        let tokens: Vec<&[u8]> = record
            .file_bytes
            .split(|byte| *byte == 0)
            .filter(|token| !token.is_empty())
            .collect();
        let mut idx = 0usize;
        let mut files = Vec::new();
        let mut renames = Vec::new();
        let mut seen = HashSet::default();
        while idx < tokens.len() {
            let status = tokens[idx];
            idx += 1;
            match status.first() {
                Some(b'R') => {
                    let old = tokens.get(idx).ok_or("truncated rename source")?;
                    let new = tokens.get(idx + 1).ok_or("truncated rename destination")?;
                    idx += 2;
                    let old_path = decode_git_path(old)?;
                    let new_path = decode_git_path(new)?;
                    if !old_path.is_empty() && !new_path.is_empty() {
                        if seen.insert(new_path.clone()) {
                            files.push(new_path.clone());
                        }
                        renames.push(HistoryRename { old_path, new_path });
                    }
                }
                Some(b'C') => {
                    let _source = tokens.get(idx).ok_or("truncated copy source")?;
                    let new = tokens.get(idx + 1).ok_or("truncated copy destination")?;
                    idx += 2;
                    let new = decode_git_path(new)?;
                    if !new.is_empty() && seen.insert(new.clone()) {
                        files.push(new);
                    }
                }
                Some(b'A' | b'M' | b'T') => {
                    let path = tokens.get(idx).ok_or("truncated changed path")?;
                    idx += 1;
                    let path = decode_git_path(path)?;
                    if !path.is_empty() && seen.insert(path.clone()) {
                        files.push(path);
                    }
                }
                _ => {
                    return Err(format!(
                        "unsupported rename-aware name-status token {:?}",
                        String::from_utf8_lossy(status)
                    )
                    .into());
                }
            }
        }
        commits.push(RenameAwareCommit {
            commit: Commit {
                hash,
                unix_time,
                date,
                subject,
                files,
            },
            renames,
        });
    }
    Ok(commits)
}

pub(super) fn parse_git_log_direct(
    out: &[u8],
    target: &str,
    config: &OnDemandConfig,
    top: usize,
    evidence_limit: isize,
) -> AnyResult<Vec<ResultItem>> {
    let max_files = config.max_files_per_commit;
    let half_life = config.half_life_days;
    let mut latest = None;
    let mut target_weight = 0.0;
    let mut pairs: HashMap<String, DirectPairStat> =
        HashMap::with_capacity_and_hasher(direct_pair_capacity(top), Default::default());
    for record in git_log_records(out) {
        let record = record?;
        let (hash, unix_time_raw, date, subject) = if config.evidence_limit == 0 {
            let (unix_time_raw, date) = record
                .header
                .split_once('\x1f')
                .ok_or("missing compact commit header field")?;
            ("", unix_time_raw, date, "")
        } else {
            let mut fields = record.header.splitn(4, '\x1f');
            let hash = fields.next().ok_or("missing commit hash")?;
            let unix_time_raw = fields.next().ok_or("missing commit unix time")?;
            let date = fields.next().ok_or("missing commit date")?;
            let subject = fields.next().unwrap_or_default();
            (hash, unix_time_raw, date, subject)
        };
        let unix_time: i64 = unix_time_raw
            .parse()
            .map_err(|err| format!("invalid commit unix time: {err}"))?;
        let date = normalize_git_iso8601_date(date);
        let files = parse_git_log_files(record.file_bytes)?;
        let file_count = files.len();
        let has_target = files.iter().any(|file| file == target);
        if file_count == 0 || file_count > max_files || !has_target {
            continue;
        }

        let latest = *latest.get_or_insert(unix_time);
        let decay = time_decay(latest, unix_time, half_life);
        target_weight += decay;

        let pair_weight = decay / ((file_count + 1) as f64).log2();
        let mut evidence = None;
        for other in files.iter().filter(|file| file.as_str() != target) {
            let pair = pairs.entry(other.clone()).or_default();
            pair.cochanges += 1;
            pair.weight += pair_weight;
            pair.other_weight += decay;
            if pair.last_seen.is_empty() || date.as_str() > pair.last_seen.as_str() {
                pair.last_seen = date.clone();
            }
            if pair.evidence.len() < config.evidence_limit {
                let evidence = evidence.get_or_insert_with(|| Evidence {
                    hash: hash.to_string(),
                    date: date.clone(),
                    subject: subject.to_string(),
                    file_count,
                    weight: pair_weight,
                });
                pair.evidence.push(evidence.clone());
            }
        }
    }

    let mut scored = Vec::with_capacity(pairs.len());
    for (path, pair) in pairs {
        let score = if target_weight <= 0.0 || pair.other_weight <= 0.0 {
            pair.weight
        } else {
            pair.weight / (target_weight * pair.other_weight).sqrt()
        };
        scored.push(DirectScoredPair { path, pair, score });
    }
    truncate_top_direct_pairs(&mut scored, top);
    Ok(scored
        .into_iter()
        .map(|item| {
            direct_pair_result(
                item.pair,
                item.path,
                item.score,
                "direct_cochange",
                evidence_limit,
            )
        })
        .collect())
}

fn normalize_git_iso8601_date(date: &str) -> String {
    date.strip_suffix('Z')
        .map_or_else(|| date.to_string(), |prefix| format!("{prefix}+00:00"))
}

pub(super) fn canonicalize_followed_target_paths(
    commits: &mut [Commit],
    target: &str,
    target_paths_by_hash: &HashMap<String, HashSet<String>>,
) {
    for commit in commits {
        let Some(target_paths) = target_paths_by_hash.get(&commit.hash) else {
            continue;
        };
        let mut seen = HashSet::default();
        let mut canonical = Vec::with_capacity(commit.files.len());
        for file in commit.files.drain(..) {
            let file = if target_paths.contains(&file) {
                target.to_string()
            } else {
                file
            };
            if seen.insert(file.clone()) {
                canonical.push(file);
            }
        }
        commit.files = canonical;
    }
}

#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_parse_bytes(data: &[u8]) {
    let _ = parse_git_log(data);
    let _ = parse_git_log_rename_aware(data);
    let _ = parse_git_follow_history(data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_git_log_parser_preserves_newlines_in_paths() {
        let raw =
            b"\0hash\x1f1\x1f2026-01-01T00:00:00Z\x1fsubject\0\n\nline\nbreak.md\0\rother.md\0";
        let commits = parse_git_log(raw).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].files, vec!["\nline\nbreak.md", "\rother.md"]);
    }

    #[test]
    fn git_log_framing_preserves_control_characters_and_empty_commits() {
        let raw = concat!(
            "\0new\x1f3\x1f2026-01-03T00:00:00Z\x1fsubject\x1e\x1f\r\ntext\0",
            "\ncompanion\x1efile.txt\0\x1eheader\x1f1\x1fdate\npath\0\0",
            "\0empty\x1f2\x1f2026-01-02T00:00:00Z\x1f\0\0",
            "\0old\x1f1\x1f2026-01-01T00:00:00Z\0\n\nleading.md\0",
        );
        let commits = parse_git_log(raw.as_bytes()).unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0].subject, "subject\x1e\x1f\r\ntext");
        assert_eq!(
            commits[0].files,
            vec!["companion\x1efile.txt", "\x1eheader\x1f1\x1fdate\npath"]
        );
        assert_eq!(commits[1].hash, "empty");
        assert!(commits[1].subject.is_empty());
        assert!(commits[1].files.is_empty());
        assert!(commits[2].subject.is_empty());
        assert_eq!(commits[2].files, vec!["\nleading.md"]);
        assert!(parse_git_log(b"\0").unwrap().is_empty());
        assert!(parse_git_log(b"\0hash\x1f1").is_err());
    }

    #[test]
    fn nul_name_status_framing_preserves_rename_paths() {
        let files = "\nR100\0\nold\x1e.md\0\rnew\x1f.md\0M\0other\n.md\0";
        let raw = format!("\0abc\x1f1\x1f2026-01-01T00:00:00Z\x1frename\x1e\x1f\0{files}");
        let commits = parse_git_log_rename_aware(raw.as_bytes()).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].commit.subject, "rename\x1e\x1f");
        assert_eq!(commits[0].commit.files, vec!["\rnew\x1f.md", "other\n.md"]);
        assert_eq!(commits[0].renames[0].old_path, "\nold\x1e.md");
        assert_eq!(commits[0].renames[0].new_path, "\rnew\x1f.md");

        let raw = format!("\0abc\0{files}\0\0def\0\nA\0other\n.md\0");
        let history = parse_git_follow_history(raw.as_bytes()).unwrap();
        assert_eq!(history.hashes, vec!["abc", "def"]);
        assert_eq!(history.hash_input, b"abc\ndef\n");
        assert_eq!(history.target_paths_by_hash["abc"].len(), 3);
        assert!(history.target_paths_by_hash["abc"].contains("\nold\x1e.md"));
        assert!(history.target_paths_by_hash["abc"].contains("\rnew\x1f.md"));
        assert!(history.target_paths_by_hash["def"].contains("other\n.md"));
    }

    #[test]
    fn git_utc_dates_use_the_pack_backend_offset_format() {
        assert_eq!(
            normalize_git_iso8601_date("2026-01-01T00:00:00Z"),
            "2026-01-01T00:00:00+00:00"
        );
        assert_eq!(
            normalize_git_iso8601_date("2026-01-01T09:00:00+09:00"),
            "2026-01-01T09:00:00+09:00"
        );
    }
}
