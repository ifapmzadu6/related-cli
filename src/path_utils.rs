use rustc_hash::FxHashMap as HashMap;
use std::path::Path;

pub(crate) fn normalize_git_path(path: &str) -> String {
    let mut path = path.trim().replace('\\', "/");
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

pub(crate) fn normalize_input_path(repo_root: &str, input: &str) -> String {
    let input = input.trim();
    let path = Path::new(input);
    if path.is_absolute()
        && let Ok(rel) = path.strip_prefix(repo_root)
    {
        return normalize_git_path(&rel.display().to_string());
    }
    normalize_git_path(input)
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
