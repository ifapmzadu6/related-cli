use crate::AnyResult;
use rustc_hash::FxHashMap as HashMap;
use std::path::{Component, Path, PathBuf};

pub(crate) fn normalize_git_path(path: &str) -> String {
    let mut path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_string()
    };
    while path.starts_with("./") {
        path = path[2..].to_string();
    }
    let parts: Vec<&str> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let normalized = parts.join("/");
    if normalized == "." {
        String::new()
    } else {
        normalized
    }
}

pub(crate) fn decode_git_path(raw: &[u8]) -> AnyResult<String> {
    let path = std::str::from_utf8(raw)
        .map_err(|_| "Git path is not valid UTF-8; this path is not supported")?;
    Ok(normalize_git_path(path))
}

pub(crate) fn normalize_input_path(
    repo_root: &Path,
    input_base: &Path,
    input: &str,
) -> AnyResult<String> {
    if input.is_empty() {
        return Err("file path must not be empty".into());
    }

    let input = Path::new(input);
    let absolute = if input.is_absolute() {
        lexical_normalize(input)
    } else {
        lexical_normalize(&input_base.join(input))
    };
    let relative = absolute.strip_prefix(repo_root).map_err(|_| {
        format!(
            "path {} is outside repository {}",
            absolute.display(),
            repo_root.display()
        )
    })?;
    let relative = relative
        .to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", relative.display()))?;
    let normalized = normalize_git_path(relative);
    if normalized.is_empty() {
        return Err("file path must refer to a file inside the repository".into());
    }
    Ok(normalized)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[inline]
pub(crate) fn ordered_pair<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    if a <= b { (a, b) } else { (b, a) }
}

#[inline]
pub(crate) fn pair_key(a: &str, b: &str) -> String {
    let (left, right) = ordered_pair(a, b);
    format!("{left}\0{right}")
}

#[inline]
pub(crate) fn literal_pathspec(path: &str) -> String {
    format!(":(literal){path}")
}

#[inline]
pub(crate) fn path_basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[inline]
fn path_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or(".", |(dir, _)| dir)
}

#[inline]
fn path_ext(path: &str) -> Option<&str> {
    let basename = path_basename(path);
    basename
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty())
}

pub(crate) fn path_tokens(path: &str) -> HashMap<String, f64> {
    let lower = path.to_lowercase();
    let mut tokens = HashMap::default();
    for part in lower.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if part.len() >= 2 {
            *tokens.entry(part.to_string()).or_default() += 1.0;
        }
    }
    let dir = path_dir(&lower);
    if dir != "." {
        *tokens.entry(format!("dir:{dir}")).or_default() += 3.0;
    }
    if let Some(ext) = path_ext(&lower) {
        *tokens.entry(format!("ext:{ext}")).or_default() += 0.5;
    }
    tokens
}

pub(crate) fn path_similarity(a: &str, b: &str, a_tokens: &HashMap<String, f64>) -> f64 {
    let b_tokens = path_tokens(b);
    let mut dot = 0.0;
    let mut a_norm = 0.0;
    let mut b_norm = 0.0;
    for (token, value) in a_tokens {
        a_norm += value * value;
        if let Some(other) = b_tokens.get(token) {
            dot += value * other;
        }
    }
    for value in b_tokens.values() {
        b_norm += value * value;
    }
    if a_norm == 0.0 || b_norm == 0.0 {
        return 0.0;
    }
    let mut score = dot / (a_norm * b_norm).sqrt();
    if path_dir(a) == path_dir(b) {
        score += 0.25;
    }
    score
}
