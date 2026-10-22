//! Axon 展示层通用格式化与渠道无关的内容投影。

mod markdown_fences;
mod markdown_normalize;
mod markdown_table;
mod tool_labels;
mod tool_row;

pub use tool_labels::builtin_tool_display_label;
pub use tool_row::{
    TOOL_INTENT_FIELD, TOOL_INTENT_FIELD_DESCRIPTION, TOOL_INTENT_MAX_CHARS,
    TOOL_ROW_TARGET_MAX_CHARS, ToolRowTarget, bare_tool_name, mcp_tool_row_target,
    tool_call_intent, tool_declares_intent, tool_intent_tool_names, tool_row_target,
    tool_row_title_carries_primary,
};

pub use markdown_normalize::normalize_markdown_source;
pub use markdown_table::{
    FenceKind, FenceTracker, is_table_delimiter_line, is_table_header_line, parse_fence_marker,
    strip_blockquote_prefix, table_candidate_text,
};

use std::path::Path;

const DECIMAL_UNITS: &[(u64, &str)] = &[
    (1_000_000_000_000, "T"),
    (1_000_000_000, "B"),
    (1_000_000, "M"),
    (1_000, "K"),
];
const DECIMAL_UNIT_SUFFIXES: [char; 4] = ['K', 'M', 'B', 'T'];
const DECIMAL_TEXT_UNIT_MIN_VALUE: f64 = 1_000.0;
const BASIS_POINTS_PER_PERCENT: u16 = 100;

const MILLIS_PER_SECOND: u64 = 1_000;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
const MILLIS_PER_MINUTE: u64 = MILLIS_PER_SECOND * SECONDS_PER_MINUTE;
const MILLIS_PER_HOUR: u64 = MILLIS_PER_MINUTE * MINUTES_PER_HOUR;
const MILLIS_PER_DAY: u64 = MILLIS_PER_HOUR * HOURS_PER_DAY;
const SECOND_DISPLAY_DECIMALS: u32 = 2;
const MILLIS_SUFFIX: &str = "ms";
const MILLIS_KEY_SUFFIXES: [&str; 2] = ["ms", "millis"];
const DURATION_KEY_MARKERS: [&str; 19] = [
    "duration",
    "elapsed",
    "timeout",
    "latency",
    "backoff",
    "remaining",
    "ttl",
    "delay",
    "cooldown",
    "retry",
    "interval",
    "耗时",
    "用时",
    "延迟",
    "超时",
    "等待",
    "剩余",
    "冷却",
    "间隔",
];
const MILLIS_KEY_SEPARATORS: [char; 2] = ['=', ':'];

/// 将路径格式化为面向用户和 JSON 投影的显示文本。
#[must_use]
pub fn display_path(path: &Path) -> String {
    path.display().to_string()
}

/// 将十进制计数量压缩为适合 TUI/CLI 的短文本。
#[must_use]
pub fn format_decimal_count(value: u64) -> String {
    for &(unit_value, suffix) in DECIMAL_UNITS {
        if value >= unit_value {
            let scaled = value as f64 / unit_value as f64;
            return format!(
                "{formatted}{suffix}",
                formatted = trim_decimal(scaled, decimal_places(scaled))
            );
        }
    }
    value.to_string()
}

/// 将 basis points 格式化为带两位小数的百分比。
#[must_use]
pub fn format_basis_points_percent(value: u16) -> String {
    format!(
        "{}.{:02}%",
        value / BASIS_POINTS_PER_PERCENT,
        value % BASIS_POINTS_PER_PERCENT
    )
}

/// 将毫秒时长压缩为适合 TUI/CLI 的短文本。
#[must_use]
pub fn format_duration_millis_compact(duration_ms: u64) -> String {
    match duration_ms {
        0..MILLIS_PER_SECOND => format!("{duration_ms}ms"),
        MILLIS_PER_SECOND..MILLIS_PER_MINUTE => format_seconds(duration_ms),
        MILLIS_PER_MINUTE..MILLIS_PER_HOUR => format_minutes(duration_ms),
        MILLIS_PER_HOUR..MILLIS_PER_DAY => format_hours(duration_ms),
        _ => format_days(duration_ms),
    }
}

/// 将已用/预算毫秒时长格式化为同一单位规则下的短文本。
#[must_use]
pub fn format_duration_millis_budget(used_ms: u64, budget_ms: Option<u64>) -> String {
    let used = format_duration_millis_compact(used_ms);
    match budget_ms {
        Some(budget_ms) => format!("{used}/{}", format_duration_millis_compact(budget_ms)),
        None => format!("{used}/-"),
    }
}

/// 将文本中的展示单位归一为紧凑单位，避免 UI 泄漏 `65000ms` / `1000k` 这类原始协议值。
#[must_use]
pub fn normalize_display_units_in_text(value: &str) -> String {
    normalize_decimal_units_in_text(&normalize_millis_units_in_text(value))
}

fn normalize_millis_units_in_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut remaining = value;
    while !remaining.is_empty() {
        if let Some(field_match) = find_millis_field_value(remaining) {
            normalized.push_str(&remaining[..field_match.value_start]);
            normalized.push_str(&format_duration_millis_compact(parse_digits_u64(
                &remaining[field_match.value_start..field_match.value_end],
            )));
            remaining =
                &remaining[millis_suffix_end_after_digits(remaining, field_match.value_end)..];
            continue;
        }
        let Some((match_start, match_end)) = find_millis_suffix(remaining) else {
            normalized.push_str(remaining);
            break;
        };
        let before_suffix = &remaining[..match_start];
        let (prefix, digits) = split_millis_candidate_prefix(before_suffix);
        if digits.is_empty() {
            normalized.push_str(&remaining[..match_end]);
            remaining = &remaining[match_end..];
            continue;
        }
        normalized.push_str(prefix);
        normalized.push_str(&format_duration_millis_compact(parse_digits_u64(digits)));
        remaining = &remaining[match_end..];
    }
    normalized
}

fn normalize_decimal_units_in_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut remaining = value;
    while !remaining.is_empty() {
        let Some(unit_match) = find_decimal_unit_match(remaining) else {
            normalized.push_str(remaining);
            break;
        };
        normalized.push_str(&remaining[..unit_match.start]);
        normalized.push_str(&format_decimal_count_f64(unit_match.absolute_value));
        remaining = &remaining[unit_match.end..];
    }
    normalized
}

fn decimal_places(scaled: f64) -> usize {
    if scaled < 10.0 {
        2
    } else {
        usize::from(scaled < 100.0)
    }
}

fn format_seconds(duration_ms: u64) -> String {
    let duration_ms = rounded_millis_for_seconds(duration_ms);
    if duration_ms >= MILLIS_PER_MINUTE {
        return format_minutes(duration_ms);
    }
    let seconds = duration_ms as f64 / MILLIS_PER_SECOND as f64;
    format!(
        "{}s",
        trim_decimal(seconds, SECOND_DISPLAY_DECIMALS as usize)
    )
}

fn rounded_millis_for_seconds(duration_ms: u64) -> u64 {
    let factor = 10_u64.pow(3 - SECOND_DISPLAY_DECIMALS);
    duration_ms
        .saturating_add(factor / 2)
        .saturating_div(factor)
        .saturating_mul(factor)
}

fn format_minutes(duration_ms: u64) -> String {
    let minutes = duration_ms / MILLIS_PER_MINUTE;
    let seconds = (duration_ms % MILLIS_PER_MINUTE) / MILLIS_PER_SECOND;
    format!("{minutes}m {seconds:02}s")
}

fn format_hours(duration_ms: u64) -> String {
    let hours = duration_ms / MILLIS_PER_HOUR;
    let minutes = (duration_ms % MILLIS_PER_HOUR) / MILLIS_PER_MINUTE;
    let seconds = (duration_ms % MILLIS_PER_MINUTE) / MILLIS_PER_SECOND;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

fn format_days(duration_ms: u64) -> String {
    let days = duration_ms / MILLIS_PER_DAY;
    let hours = (duration_ms % MILLIS_PER_DAY) / MILLIS_PER_HOUR;
    let minutes = (duration_ms % MILLIS_PER_HOUR) / MILLIS_PER_MINUTE;
    format!("{days}d {hours:02}h {minutes:02}m")
}

fn trim_decimal(value: f64, decimals: usize) -> String {
    let mut formatted = format!("{value:.decimals$}");
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }
    formatted
}

fn format_decimal_count_f64(value: f64) -> String {
    for &(unit_value, suffix) in DECIMAL_UNITS {
        let unit_value = unit_value as f64;
        if value >= unit_value {
            let scaled = value / unit_value;
            return format!(
                "{formatted}{suffix}",
                formatted = trim_decimal(scaled, decimal_places(scaled))
            );
        }
    }
    trim_decimal(value, 0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DecimalUnitMatch {
    start: usize,
    end: usize,
    absolute_value: f64,
}

fn find_decimal_unit_match(value: &str) -> Option<DecimalUnitMatch> {
    for (index, ch) in value.char_indices() {
        if !ch.is_ascii_digit() {
            continue;
        }
        if index > 0
            && value[..index]
                .chars()
                .last()
                .is_some_and(is_decimal_text_boundary_blocker)
        {
            continue;
        }
        let Some(unit_match) = parse_decimal_unit_match(value, index) else {
            continue;
        };
        if unit_match.absolute_value >= DECIMAL_TEXT_UNIT_MIN_VALUE {
            return Some(unit_match);
        }
    }
    None
}

fn parse_decimal_unit_match(value: &str, start: usize) -> Option<DecimalUnitMatch> {
    let number_end = decimal_number_end(value, start);
    let suffix_start = number_end
        + value[number_end..]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .map(char::len_utf8)
            .sum::<usize>();
    let suffix = value[suffix_start..].chars().next()?;
    let normalized_suffix = suffix.to_ascii_uppercase();
    if !is_decimal_count_suffix(suffix) {
        return None;
    }
    let end = suffix_start + suffix.len_utf8();
    if value[end..]
        .chars()
        .next()
        .is_some_and(is_decimal_text_boundary_blocker)
    {
        return None;
    }
    let number = value[start..number_end].parse::<f64>().ok()?;
    let multiplier = decimal_unit_multiplier(normalized_suffix)?;
    Some(DecimalUnitMatch {
        start,
        end,
        absolute_value: number * multiplier as f64,
    })
}

fn is_decimal_count_suffix(suffix: char) -> bool {
    suffix == 'k' || DECIMAL_UNIT_SUFFIXES.contains(&suffix)
}

fn decimal_number_end(value: &str, start: usize) -> usize {
    let mut end = start;
    let mut seen_dot = false;
    for (offset, ch) in value[start..].char_indices() {
        if ch.is_ascii_digit() {
            end = start + offset + ch.len_utf8();
            continue;
        }
        if ch == '.' && !seen_dot {
            seen_dot = true;
            end = start + offset + ch.len_utf8();
            continue;
        }
        break;
    }
    if value[..end].ends_with('.') {
        end - 1
    } else {
        end
    }
}

fn decimal_unit_multiplier(suffix: char) -> Option<u64> {
    DECIMAL_UNITS
        .iter()
        .find_map(|&(value, unit)| (unit.as_bytes()[0] as char == suffix).then_some(value))
}

fn is_decimal_text_boundary_blocker(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn split_millis_candidate_prefix(value: &str) -> (&str, &str) {
    let without_space = value.trim_end_matches(' ');
    let digit_start = without_space
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let prefix = &without_space[..digit_start];
    let digits = &without_space[digit_start..];
    if digits.is_empty()
        || prefix.ends_with(['_', '-'])
        || prefix
            .chars()
            .last()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
    {
        return (value, "");
    }
    (prefix, digits)
}

fn find_millis_suffix(value: &str) -> Option<(usize, usize)> {
    value.char_indices().find_map(|(index, _)| {
        value
            .get(index..index + MILLIS_SUFFIX.len())
            .filter(|candidate| candidate.eq_ignore_ascii_case(MILLIS_SUFFIX))
            .map(|_| (index, index + MILLIS_SUFFIX.len()))
    })
}

fn millis_suffix_end_after_digits(value: &str, digit_end: usize) -> usize {
    let suffix_start = digit_end
        + value[digit_end..]
            .chars()
            .take_while(char::is_ascii_whitespace)
            .map(char::len_utf8)
            .sum::<usize>();
    value
        .get(suffix_start..suffix_start + MILLIS_SUFFIX.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(MILLIS_SUFFIX))
        .map_or(digit_end, |_| suffix_start + MILLIS_SUFFIX.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MillisFieldMatch {
    value_start: usize,
    value_end: usize,
}

fn find_millis_field_value(value: &str) -> Option<MillisFieldMatch> {
    for (index, separator) in value.char_indices() {
        if !MILLIS_KEY_SEPARATORS.contains(&separator) {
            continue;
        }
        let Some(key) = millis_key_before_separator(&value[..index]) else {
            continue;
        };
        if !is_millis_key(key) {
            continue;
        }
        let value_start = index
            + separator.len_utf8()
            + value[index + separator.len_utf8()..]
                .chars()
                .take_while(char::is_ascii_whitespace)
                .map(char::len_utf8)
                .sum::<usize>();
        let value_end = value_start
            + value[value_start..]
                .chars()
                .take_while(char::is_ascii_digit)
                .map(char::len_utf8)
                .sum::<usize>();
        if value_end > value_start {
            return Some(MillisFieldMatch {
                value_start,
                value_end,
            });
        }
    }
    None
}

fn millis_key_before_separator(value: &str) -> Option<&str> {
    let trimmed = value.trim_end_matches(char::is_whitespace);
    let key_start = trimmed
        .char_indices()
        .rev()
        .find(|(_, ch)| !is_key_char(*ch))
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    let key = &trimmed[key_start..];
    (!key.is_empty()).then_some(key)
}

fn is_key_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

fn is_millis_key(key: &str) -> bool {
    let normalized = key.replace(['_', '-'], "").to_ascii_lowercase();
    MILLIS_KEY_SUFFIXES
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
        && DURATION_KEY_MARKERS
            .iter()
            .any(|marker| normalized.contains(marker))
}

fn parse_digits_u64(value: &str) -> u64 {
    value.parse::<u64>().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        format_basis_points_percent, format_decimal_count, format_duration_millis_compact,
        normalize_display_units_in_text,
    };

    #[test]
    fn decimal_count_uses_readable_units() {
        assert_eq!(format_decimal_count(999), "999");
        assert_eq!(format_decimal_count(1_200), "1.2K");
        assert_eq!(format_decimal_count(12_345), "12.3K");
        assert_eq!(format_decimal_count(128_000), "128K");
        assert_eq!(format_decimal_count(1_000_000), "1M");
        assert_eq!(format_decimal_count(12_345_678), "12.3M");
        assert_eq!(format_decimal_count(1_000_000_000), "1B");
        assert_eq!(format_decimal_count(1_234_567_890), "1.23B");
        assert_eq!(format_decimal_count(1_000_000_000_000), "1T");
    }

    #[test]
    fn basis_points_percent_keeps_two_decimal_places() {
        assert_eq!(format_basis_points_percent(9_000), "90.00%");
        assert_eq!(format_basis_points_percent(8_181), "81.81%");
    }

    #[test]
    fn millis_scale_to_readable_duration_units() {
        assert_eq!(format_duration_millis_compact(999), "999ms");
        assert_eq!(format_duration_millis_compact(1_000), "1s");
        assert_eq!(format_duration_millis_compact(1_250), "1.25s");
        assert_eq!(format_duration_millis_compact(59_994), "59.99s");
        assert_eq!(format_duration_millis_compact(59_995), "1m 00s");
        assert_eq!(format_duration_millis_compact(60_000), "1m 00s");
        assert_eq!(format_duration_millis_compact(65_000), "1m 05s");
        assert_eq!(format_duration_millis_compact(3_600_000), "1h 00m 00s");
        assert_eq!(format_duration_millis_compact(3_661_000), "1h 01m 01s");
        assert_eq!(format_duration_millis_compact(86_400_000), "1d 00h 00m");
        assert_eq!(format_duration_millis_compact(90_000_000), "1d 01h 00m");
    }

    #[test]
    fn text_millis_units_scale_when_values_are_large() {
        assert_eq!(
            normalize_display_units_in_text("elapsed=65000ms timeout 3600000 ms"),
            "elapsed=1m 05s timeout 1h 00m 00s"
        );
        assert_eq!(
            normalize_display_units_in_text("duration=1250ms retry=999ms"),
            "duration=1.25s retry=999ms"
        );
        assert_eq!(
            normalize_display_units_in_text("latency 65000MS interval 1250 Ms"),
            "latency 1m 05s interval 1.25s"
        );
    }

    #[test]
    fn text_millis_fields_scale_duration_values_without_touching_timestamps() {
        assert_eq!(
            normalize_display_units_in_text("duration_ms=65000 timeout_ms"),
            "duration_ms=1m 05s timeout_ms"
        );
        assert_eq!(
            normalize_display_units_in_text("durationMs=65000ms elapsed_ms: 1250 ms"),
            "durationMs=1m 05s elapsed_ms: 1.25s"
        );
        assert_eq!(normalize_display_units_in_text("id123ms"), "id123ms");
        assert_eq!(
            normalize_display_units_in_text("durationMs=65000 elapsed_ms: 1250 retryMillis=999"),
            "durationMs=1m 05s elapsed_ms: 1.25s retryMillis=999ms"
        );
        assert_eq!(
            normalize_display_units_in_text("耗时_ms=65000 延迟Millis:1250 超时_ms=3600000"),
            "耗时_ms=1m 05s 延迟Millis:1.25s 超时_ms=1h 00m 00s"
        );
        assert_eq!(
            normalize_display_units_in_text("recordedAtMs=100 startedAtMs=200 completedAtMs=65000"),
            "recordedAtMs=100 startedAtMs=200 completedAtMs=65000"
        );
    }

    #[test]
    fn text_decimal_units_scale_when_values_are_large() {
        assert_eq!(
            normalize_display_units_in_text("ctx:5.1k/1000k tokens 1500 K"),
            "ctx:5.1K/1M tokens 1.5M"
        );
        assert_eq!(
            normalize_display_units_in_text("input=1024k cached=999k output=1.25M"),
            "input=1.02M cached=999K output=1.25M"
        );
        assert_eq!(
            normalize_display_units_in_text("model gpt-4.1 endpoint key_1000k x1000k"),
            "model gpt-4.1 endpoint key_1000k x1000k"
        );
    }
}
