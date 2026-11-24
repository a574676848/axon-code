//! apply_patch 上下文模糊匹配。
//!
//! 在目标文件行中按递减严格度定位补丁上下文/旧行序列，对齐 Codex
//! `seek_sequence` 的四级回退：精确匹配 → 忽略尾随空白 → 忽略首尾空白 →
//! Unicode 标点归一化。

/// 在 `lines` 中从 `start` 起查找 `pattern` 行序列，返回匹配起始下标。
///
/// 匹配按严格度递减尝试：精确、忽略尾随空白、忽略首尾空白、Unicode 归一化。
/// 当 `eof` 为 true 时优先从文件末尾对齐（用于匹配文件结尾的补丁）。
///
/// 边界语义：
/// - `pattern` 为空 → 返回 `Some(start)`（空匹配）。
/// - `pattern.len() > lines.len()` → 返回 `None`（无法匹配）。
#[must_use]
pub(crate) fn seek_sequence(
    lines: &[String],
    pattern: &[String],
    start: usize,
    eof: bool,
) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }

    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };
    let last_start = lines.len().saturating_sub(pattern.len());

    // 第 1 级：精确匹配。
    for i in search_start..=last_start {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    // 第 2 级：忽略尾随空白。
    if let Some(idx) = match_with(lines, pattern, search_start, last_start, |line, pat| {
        line.trim_end() == pat.trim_end()
    }) {
        return Some(idx);
    }
    // 第 3 级：忽略首尾空白。
    if let Some(idx) = match_with(lines, pattern, search_start, last_start, |line, pat| {
        line.trim() == pat.trim()
    }) {
        return Some(idx);
    }
    // 第 4 级：Unicode 标点归一化后匹配。
    if let Some(idx) = match_with(lines, pattern, search_start, last_start, |line, pat| {
        normalise(line) == normalise(pat)
    }) {
        return Some(idx);
    }
    // 第 5 级：弹性连续空行匹配。
    if let Some(idx) = match_with(lines, pattern, search_start, last_start, |line, pat| {
        if line.trim().is_empty() && pat.trim().is_empty() {
            true
        } else {
            normalise(line) == normalise(pat)
        }
    }) {
        return Some(idx);
    }
    // 第 6 级：高置信度模糊相似度对齐（仅当 pattern >= 2 行且最佳匹配点显著无歧义时）
    if pattern.len() >= 2
        && let Some((best_idx, confidence)) =
            find_best_matching_window(lines, pattern, search_start)
        && confidence >= 0.88
    {
        return Some(best_idx);
    }
    None
}

/// 在 lines 中搜索与 pattern 最接近的窗口，返回 (起始行下标, 平均相似度 0.0~1.0)。
pub fn find_best_matching_window(
    lines: &[String],
    pattern: &[String],
    search_start: usize,
) -> Option<(usize, f32)> {
    if pattern.is_empty() || lines.len() < pattern.len() {
        return None;
    }
    let last_start = lines.len().saturating_sub(pattern.len());
    let mut best_idx = 0;
    let mut best_score = 0.0f32;
    let mut second_best_score = 0.0f32;

    for i in search_start..=last_start {
        let score = window_similarity(&lines[i..i + pattern.len()], pattern);
        if score > best_score {
            second_best_score = best_score;
            best_score = score;
            best_idx = i;
        } else if score > second_best_score {
            second_best_score = score;
        }
    }

    if best_score > 0.0 {
        let gap = best_score - second_best_score;
        let final_confidence = if second_best_score > 0.6 && gap < 0.12 {
            best_score * 0.8
        } else {
            best_score
        };
        Some((best_idx, final_confidence))
    } else {
        None
    }
}

fn window_similarity(window: &[String], pattern: &[String]) -> f32 {
    let mut total = 0.0;
    for (w, p) in window.iter().zip(pattern.iter()) {
        total += line_similarity(w, p);
    }
    total / (pattern.len() as f32)
}

pub(crate) fn line_similarity(a: &str, b: &str) -> f32 {
    let a_trim = a.trim();
    let b_trim = b.trim();
    if a_trim == b_trim {
        return 1.0;
    }
    if a_trim.is_empty() || b_trim.is_empty() {
        return 0.0;
    }
    let max_len = a_trim.len().max(b_trim.len());
    let dist = levenshtein_distance(a_trim, b_trim);
    1.0 - (dist as f32 / max_len as f32)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let (m, n) = (a_chars.len(), b_chars.len());
    let mut dp = vec![vec![0usize; n + 1]; 2];
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for (i, &ca) in a_chars.iter().enumerate() {
        let curr = (i + 1) % 2;
        let prev = i % 2;
        dp[curr][0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            dp[curr][j + 1] = (dp[prev][j + 1] + 1)
                .min(dp[curr][j] + 1)
                .min(dp[prev][j] + cost);
        }
    }
    dp[m % 2][n]
}

/// 用给定的逐行比较谓词在窗口内查找匹配。
fn match_with(
    lines: &[String],
    pattern: &[String],
    search_start: usize,
    last_start: usize,
    eq: impl Fn(&str, &str) -> bool,
) -> Option<usize> {
    for i in search_start..=last_start {
        let mut ok = true;
        for (p_idx, pat) in pattern.iter().enumerate() {
            if !eq(&lines[i + p_idx], pat) {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(i);
        }
    }
    None
}

/// 将常见 Unicode 标点归一化为 ASCII 等价字符，模拟 `git apply` 的宽容匹配。
fn normalise(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| match character {
            // 各类破折号/连字符 → ASCII '-'
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            // 花式单引号 → '
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // 花式双引号 → "
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // 不间断空格及各类异常空格 → 普通空格
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::seek_sequence;

    fn to_vec(strings: &[&str]) -> Vec<String> {
        strings.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn exact_match_finds_sequence() {
        let lines = to_vec(&["foo", "bar", "baz"]);
        let pattern = to_vec(&["bar", "baz"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(1));
    }

    #[test]
    fn rstrip_match_ignores_trailing_whitespace() {
        let lines = to_vec(&["foo   ", "bar\t\t"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn trim_match_ignores_leading_and_trailing_whitespace() {
        let lines = to_vec(&["    foo   ", "   bar\t"]);
        let pattern = to_vec(&["foo", "bar"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn pattern_longer_than_input_returns_none() {
        let lines = to_vec(&["just one line"]);
        let pattern = to_vec(&["too", "many", "lines"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), None);
    }

    #[test]
    fn unicode_dash_normalisation_matches() {
        let lines = to_vec(&["let x = a \u{2212} b;"]);
        let pattern = to_vec(&["let x = a - b;"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn eof_prefers_end_of_file_alignment() {
        let lines = to_vec(&["a", "x", "b", "a"]);
        let pattern = to_vec(&["a"]);
        // eof=true 时从文件末尾窗口开始匹配，命中末尾的 "a"（下标 3）。
        assert_eq!(seek_sequence(&lines, &pattern, 0, true), Some(3));
        // eof=false 时从 start=0 顺序匹配，命中首个 "a"（下标 0）。
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }
    #[test]
    fn elastic_blank_lines_match() {
        let lines = to_vec(&["fn foo() {", "", "    let x = 1;", "}"]);
        let pattern = to_vec(&["fn foo() {", "   ", "    let x = 1;", "}"]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }

    #[test]
    fn fuzzy_similarity_alignment_matches() {
        let lines = to_vec(&[
            "pub fn calculate_price(amount: f64, rate: f64) -> f64 {",
            "    let tax = amount * rate;",
            "    amount + tax",
            "}",
        ]);
        // pattern 中有轻微字符偏差（如漏了类型空格或小差异），但高置信唯一
        let pattern = to_vec(&[
            "pub fn calculate_price(amount:f64, rate:f64) -> f64 {",
            "    let tax = amount * rate;",
        ]);
        assert_eq!(seek_sequence(&lines, &pattern, 0, false), Some(0));
    }
}
