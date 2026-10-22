//! git apply 输出与 diff header 解析。

use once_cell::sync::Lazy;
use regex::Regex;

/// 解析 git apply 输出为 applied/skipped/conflicted 路径分组。
pub fn parse_git_apply_output(
    stdout: &str,
    stderr: &str,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let combined = [stdout, stderr]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<&str>>()
        .join("\n");

    let mut applied = std::collections::BTreeSet::new();
    let mut skipped = std::collections::BTreeSet::new();
    let mut conflicted = std::collections::BTreeSet::new();
    let mut last_seen_path: Option<String> = None;

    for raw_line in combined.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(c) = CHECKING_PATCH.captures(line) {
            if let Some(m) = c.name("path") {
                last_seen_path = Some(m.as_str().to_string());
            }
            continue;
        }

        if let Some(c) = APPLIED_CLEAN.captures(line) {
            if let Some(m) = c.name("path") {
                add_path(&mut applied, m.as_str());
                if let Some(path) = applied.iter().next_back().cloned() {
                    conflicted.remove(&path);
                    skipped.remove(&path);
                    last_seen_path = Some(path);
                }
            }
            continue;
        }
        if let Some(c) = APPLIED_CONFLICTS.captures(line) {
            if let Some(m) = c.name("path") {
                add_path(&mut conflicted, m.as_str());
                if let Some(path) = conflicted.iter().next_back().cloned() {
                    applied.remove(&path);
                    skipped.remove(&path);
                    last_seen_path = Some(path);
                }
            }
            continue;
        }
        if let Some(c) = APPLYING_WITH_REJECTS.captures(line) {
            if let Some(m) = c.name("path") {
                add_path(&mut conflicted, m.as_str());
                if let Some(path) = conflicted.iter().next_back().cloned() {
                    applied.remove(&path);
                    skipped.remove(&path);
                    last_seen_path = Some(path);
                }
            }
            continue;
        }

        if let Some(c) = UNMERGED_LINE.captures(line) {
            if let Some(m) = c.name("path") {
                add_path(&mut conflicted, m.as_str());
                if let Some(path) = conflicted.iter().next_back().cloned() {
                    applied.remove(&path);
                    skipped.remove(&path);
                    last_seen_path = Some(path);
                }
            }
            continue;
        }

        if PATCH_FAILED.is_match(line) || DOES_NOT_APPLY.is_match(line) {
            if let Some(c) = PATCH_FAILED
                .captures(line)
                .or_else(|| DOES_NOT_APPLY.captures(line))
                && let Some(m) = c.name("path")
            {
                add_path(&mut skipped, m.as_str());
                last_seen_path = Some(m.as_str().to_string());
            }
            continue;
        }

        if THREE_WAY_FAILED.is_match(line) || LACKS_BLOB.is_match(line) {
            if let Some(path) = last_seen_path.clone() {
                add_path(&mut skipped, &path);
                applied.remove(&path);
                conflicted.remove(&path);
            }
            continue;
        }

        if let Some(c) = INDEX_MISMATCH
            .captures(line)
            .or_else(|| NOT_IN_INDEX.captures(line))
            .or_else(|| ALREADY_EXISTS_WT.captures(line))
            .or_else(|| SKIPPED_PATCH.captures(line))
        {
            if let Some(m) = c.name("path") {
                add_path(&mut skipped, m.as_str());
                if let Some(path) = skipped.iter().next_back().cloned() {
                    applied.remove(&path);
                    conflicted.remove(&path);
                    last_seen_path = Some(path);
                }
            }
        }
    }

    for path in conflicted.iter() {
        applied.remove(path);
        skipped.remove(path);
    }
    for path in applied.iter() {
        skipped.remove(path);
    }

    (
        applied.into_iter().collect(),
        skipped.into_iter().collect(),
        conflicted.into_iter().collect(),
    )
}

/// 从补丁的 diff --git 头部收集所有引用的路径。
#[must_use]
pub fn extract_paths_from_patch(diff_text: &str) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    for raw_line in diff_text.lines() {
        let line = raw_line.trim();
        let Some(rest) = line.strip_prefix("diff --git ") else {
            continue;
        };
        let Some((a, b)) = parse_diff_git_paths(rest) else {
            continue;
        };
        if let Some(a) = normalize_diff_path(&a, "a/") {
            set.insert(a);
        }
        if let Some(b) = normalize_diff_path(&b, "b/") {
            set.insert(b);
        }
    }
    set.into_iter().collect()
}

fn add_path(set: &mut std::collections::BTreeSet<String>, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let first = trimmed.chars().next().unwrap_or('\0');
    let last = trimmed.chars().last().unwrap_or('\0');
    let unquoted = if (first == '"' || first == '\'') && last == first && trimmed.len() >= 2 {
        unescape_c_string(&trimmed[1..trimmed.len() - 1])
    } else {
        trimmed.to_string()
    };
    if !unquoted.is_empty() {
        set.insert(unquoted);
    }
}

fn parse_diff_git_paths(line: &str) -> Option<(String, String)> {
    let mut chars = line.chars().peekable();
    let first = read_diff_git_token(&mut chars)?;
    let second = read_diff_git_token(&mut chars)?;
    Some((first, second))
}

fn read_diff_git_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
        chars.next();
    }
    let quote = match chars.peek().copied() {
        Some('"') | Some('\'') => chars.next(),
        _ => None,
    };
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if let Some(quote_char) = quote {
            if ch == quote_char {
                break;
            }
            if ch == '\\' {
                out.push('\\');
                if let Some(next) = chars.next() {
                    out.push(next);
                }
                continue;
            }
        } else if ch.is_whitespace() {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() && quote.is_none() {
        None
    } else {
        Some(match quote {
            Some(_) => unescape_c_string(&out),
            None => out,
        })
    }
}

fn normalize_diff_path(raw: &str, prefix: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "/dev/null" || trimmed == format!("{prefix}dev/null") {
        return None;
    }
    let trimmed = trimmed.strip_prefix(prefix).unwrap_or(trimmed);
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

fn unescape_c_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            out.push('\\');
            break;
        };
        match next {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'a' => out.push('\u{0007}'),
            'v' => out.push('\u{000B}'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            '0'..='7' => {
                let mut value = next.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek() {
                        Some('0'..='7') => {
                            if let Some(digit) = chars.next() {
                                value = value * 8 + digit.to_digit(8).unwrap_or(0);
                            } else {
                                break;
                            }
                        }
                        _ => break,
                    }
                }
                if let Some(ch) = std::char::from_u32(value) {
                    out.push(ch);
                }
            }
            other => out.push(other),
        }
    }
    out
}

static APPLIED_CLEAN: Lazy<Regex> =
    Lazy::new(|| regex_ci("^Applied patch(?: to)?\\s+(?P<path>.+?)\\s+cleanly\\.?$"));
static APPLIED_CONFLICTS: Lazy<Regex> =
    Lazy::new(|| regex_ci("^Applied patch(?: to)?\\s+(?P<path>.+?)\\s+with conflicts\\.?$"));
static APPLYING_WITH_REJECTS: Lazy<Regex> = Lazy::new(|| {
    regex_ci("^Applying patch\\s+(?P<path>.+?)\\s+with\\s+\\d+\\s+rejects?\\.{0,3}$")
});
static CHECKING_PATCH: Lazy<Regex> =
    Lazy::new(|| regex_ci("^Checking patch\\s+(?P<path>.+?)\\.\\.\\.$"));
static UNMERGED_LINE: Lazy<Regex> = Lazy::new(|| regex_ci("^U\\s+(?P<path>.+)$"));
static PATCH_FAILED: Lazy<Regex> =
    Lazy::new(|| regex_ci("^error:\\s+patch failed:\\s+(?P<path>.+?)(?::\\d+)?(?:\\s|$)"));
static DOES_NOT_APPLY: Lazy<Regex> =
    Lazy::new(|| regex_ci("^error:\\s+(?P<path>.+?):\\s+patch does not apply$"));
static THREE_WAY_FAILED: Lazy<Regex> =
    Lazy::new(|| regex_ci("^Failed to perform three-way merge\\.\\.\\.$"));
static LACKS_BLOB: Lazy<Regex> = Lazy::new(|| {
    regex_ci(
        "^(?:error: )?repository lacks the necessary blob to (?:perform|fall back on) 3-?way merge\\.?$",
    )
});
static INDEX_MISMATCH: Lazy<Regex> =
    Lazy::new(|| regex_ci("^error:\\s+(?P<path>.+?):\\s+does not match index\\b"));
static NOT_IN_INDEX: Lazy<Regex> =
    Lazy::new(|| regex_ci("^error:\\s+(?P<path>.+?):\\s+does not exist in index\\b"));
static ALREADY_EXISTS_WT: Lazy<Regex> = Lazy::new(|| {
    regex_ci("^error:\\s+(?P<path>.+?)\\s+already exists in (?:the )?working directory\\b")
});
static SKIPPED_PATCH: Lazy<Regex> =
    Lazy::new(|| regex_ci("^Skipped patch\\s+['\\\"]?(?P<path>.+?)['\\\"]\\.$"));

fn regex_ci(pat: &str) -> Regex {
    Regex::new(&format!("(?i){pat}")).unwrap_or_else(|err| panic!("invalid regex: {err}"))
}
