use crate::AnyResult;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Default)]
pub(crate) struct ParsedArgs {
    pub(crate) flags: HashMap<String, Option<String>>,
    pub(crate) positionals: Vec<String>,
}

pub(crate) fn parse_args(
    args: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> AnyResult<ParsedArgs> {
    let value_flags: HashSet<&str> = value_flags.iter().copied().collect();
    let bool_flags: HashSet<&str> = bool_flags.iter().copied().collect();
    let mut parsed = ParsedArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            parsed.positionals.extend(args[i + 1..].iter().cloned());
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            parsed.positionals.push(arg.clone());
            i += 1;
            continue;
        }

        let raw = arg.trim_start_matches('-');
        let (name, inline_value) = raw
            .split_once('=')
            .map_or((raw, None), |(name, value)| (name, Some(value.to_string())));
        if value_flags.contains(name) {
            let value = if let Some(value) = inline_value {
                value
            } else {
                i += 1;
                let Some(value) = args.get(i) else {
                    return Err(format!("flag needs an argument: --{name}").into());
                };
                value.clone()
            };
            parsed.flags.insert(name.to_string(), Some(value));
        } else if bool_flags.contains(name) {
            if inline_value.is_some() {
                return Err(format!("boolean flag --{name} does not take a value").into());
            }
            parsed.flags.insert(name.to_string(), None);
        } else {
            return Err(format!("unknown flag --{name}").into());
        }
        i += 1;
    }
    Ok(parsed)
}

pub(crate) fn flag_optional_string(parsed: &ParsedArgs, name: &str) -> Option<String> {
    parsed.flags.get(name).and_then(Clone::clone)
}

pub(crate) fn flag_string(parsed: &ParsedArgs, name: &str, default: &str) -> String {
    flag_optional_string(parsed, name).unwrap_or_else(|| default.to_string())
}

pub(crate) fn flag_bool(parsed: &ParsedArgs, name: &str) -> bool {
    parsed.flags.contains_key(name)
}

pub(crate) fn flag_usize(parsed: &ParsedArgs, name: &str, default: usize) -> AnyResult<usize> {
    let Some(value) = flag_optional_string(parsed, name) else {
        return Ok(default);
    };
    value
        .parse()
        .map_err(|err| format!("invalid --{name} value {value:?}: {err}").into())
}

pub(crate) fn flag_f64(parsed: &ParsedArgs, name: &str, default: f64) -> AnyResult<f64> {
    let Some(value) = flag_optional_string(parsed, name) else {
        return Ok(default);
    };
    value
        .parse()
        .map_err(|err| format!("invalid --{name} value {value:?}: {err}").into())
}

pub(crate) fn flag_positive_usize(
    parsed: &ParsedArgs,
    name: &str,
    default: usize,
) -> AnyResult<usize> {
    let value = flag_usize(parsed, name, default)?;
    if value == 0 {
        return Err(format!("--{name} must be positive").into());
    }
    Ok(value)
}

pub(crate) fn flag_positive_f64(parsed: &ParsedArgs, name: &str, default: f64) -> AnyResult<f64> {
    let value = flag_f64(parsed, name, default)?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("--{name} must be a finite positive number").into());
    }
    Ok(value)
}

pub(crate) fn parse_modes(input: &str) -> Vec<String> {
    let mut seen = HashSet::default();
    let mut modes = Vec::new();
    for raw in input.split(',') {
        let mode = raw.trim();
        if mode.is_empty() || !seen.insert(mode.to_string()) {
            continue;
        }
        modes.push(mode.to_string());
    }
    if modes.is_empty() {
        vec![
            "direct".to_string(),
            "pagerank".to_string(),
            "path".to_string(),
            "hot".to_string(),
        ]
    } else {
        modes
    }
}
