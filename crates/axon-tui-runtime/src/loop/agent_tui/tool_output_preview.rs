//! 工具参数与输出预览的截断、归一化和脱敏。

use axon_display::normalize_display_units_in_text;
use serde_json::Value;

pub(in crate::r#loop::agent_tui) const TOOL_OUTPUT_CHUNK_MAX_CHARS: usize = 1_200;
const TOOL_ARGUMENT_PREVIEW_MAX_CHARS: usize = 600;
const TOOL_OUTPUT_CHUNK_MAX_LINES: usize = 5;
const TOOL_OUTPUT_CHUNK_LINE_MAX_CHARS: usize = 240;
pub(in crate::r#loop::agent_tui) const REDACTED_VALUE: &str = "[已隐藏]";
const SENSITIVE_MARKERS: [&str; 8] = [
    "api_key",
    "apikey",
    "authorization",
    "password",
    "secret",
    "token",
    "access_key",
    "private_key",
];

pub(in crate::r#loop::agent_tui) fn tool_arguments_preview(arguments_preview: &str) -> String {
    let preview = sanitize_preview(arguments_preview, TOOL_ARGUMENT_PREVIEW_MAX_CHARS);
    if preview.trim().is_empty() {
        "{}".to_owned()
    } else {
        preview
    }
}

/// 将执行回执中的规范 shell argv 投影为用户真正关心的脚本文本。
///
/// runtime 注入的启动参数、UTF-8 前缀和 wrapper 只服务于进程启动，不应占据 transcript
/// 的首屏；无法确认结构时保留原文，避免误删用户命令。
pub(crate) fn executed_command_preview(value: &str) -> String {
    let command = command_value(value).unwrap_or_else(|| value.trim().to_owned());
    let command = strip_shell_wrapper(&command);
    let command = strip_powershell_output_prefix(&command);
    sanitize_preview(&command, TOOL_ARGUMENT_PREVIEW_MAX_CHARS)
}

fn command_value(value: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(value).ok()?;
    parsed
        .get("command")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn strip_shell_wrapper(value: &str) -> String {
    let tokens = value.split_whitespace().collect::<Vec<_>>();
    let Some(program) = tokens.first().map(|token| {
        token
            .trim_matches(['"', '\''])
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(token)
            .to_ascii_lowercase()
    }) else {
        return value.trim().to_owned();
    };
    let flag = match program.as_str() {
        "pwsh" | "powershell" => "-command",
        "bash" | "zsh" | "sh" => "-lc",
        "wsl.exe" | "wsl" => "-lc",
        _ => return value.trim().to_owned(),
    };
    let Some(index) = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case(flag))
    else {
        return value.trim().to_owned();
    };
    tokens[index + 1..].join(" ")
}

fn strip_powershell_output_prefix(value: &str) -> String {
    let trimmed = value.trim();
    let Some(prefix_end) = trimmed.find("catch {}") else {
        return trimmed.to_owned();
    };
    let prefix = &trimmed[..prefix_end + "catch {}".len()];
    if !prefix.contains("[Console]::OutputEncoding") {
        return trimmed.to_owned();
    }
    trimmed[prefix.len()..]
        .trim_start_matches([';', '\n', '\r', ' '])
        .to_owned()
}

pub(in crate::r#loop::agent_tui) fn compact_tool_output_chunk(value: &str) -> String {
    let redacted = redact_sensitive_segments(value);
    let text = normalize_display_units_in_text(&bounded_output_preview(&redacted));
    if text.is_empty() {
        "(empty)".to_owned()
    } else {
        text
    }
}

fn sanitize_preview(value: &str, max_chars: usize) -> String {
    let redacted = redact_sensitive_segments(value);
    let normalized = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_preview(&normalized, max_chars)
}

fn bounded_output_preview(value: &str) -> String {
    let lines = value
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(|line| truncate_preview(line, TOOL_OUTPUT_CHUNK_LINE_MAX_CHARS))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return String::new();
    }
    let bounded = bounded_output_lines(&lines, TOOL_OUTPUT_CHUNK_MAX_LINES);
    truncate_preview(&bounded.join("\n"), TOOL_OUTPUT_CHUNK_MAX_CHARS)
}

fn bounded_output_lines(lines: &[String], max_lines: usize) -> Vec<String> {
    if lines.len() <= max_lines {
        return lines.to_vec();
    }
    if max_lines <= 1 {
        return vec![omitted_output_line(lines.len())];
    }
    let head = max_lines / 2;
    let tail = max_lines.saturating_sub(head + 1);
    let omitted = lines.len().saturating_sub(head + tail);
    let mut output = Vec::with_capacity(max_lines);
    output.extend(lines.iter().take(head).cloned());
    output.push(omitted_output_line(omitted));
    output.extend(lines.iter().skip(lines.len().saturating_sub(tail)).cloned());
    output
}

fn omitted_output_line(omitted: usize) -> String {
    format!("… +{omitted} lines")
}

fn redact_sensitive_segments(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;

    while index < chars.len() {
        let Some(match_result) = SensitiveAssignment::parse(&chars, index) else {
            output.push(chars[index]);
            index += 1;
            continue;
        };

        if !is_sensitive_key(&chars[match_result.key.clone()]) {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        output.extend(chars[index..match_result.value_start].iter());
        index = append_redacted_value(
            &chars,
            match_result.value_start,
            is_sensitive_header_key(&chars[match_result.key.clone()]),
            &mut output,
        );
    }

    output
}

#[derive(Debug, Clone)]
struct SensitiveAssignment {
    key: std::ops::Range<usize>,
    value_start: usize,
}

impl SensitiveAssignment {
    fn parse(chars: &[char], start: usize) -> Option<Self> {
        let key = parse_key(chars, start)?;
        let separator = skip_ascii_whitespace(chars, key.after_key);
        if !matches!(chars.get(separator), Some(':' | '=')) {
            return None;
        }
        let value_start = skip_ascii_whitespace(chars, separator + 1);
        Some(Self {
            key: key.range,
            value_start,
        })
    }
}

#[derive(Debug, Clone)]
struct ParsedKey {
    range: std::ops::Range<usize>,
    after_key: usize,
}

fn parse_key(chars: &[char], start: usize) -> Option<ParsedKey> {
    match chars.get(start).copied()? {
        '"' | '\'' => parse_quoted_key(chars, start),
        ch if is_key_char(ch) => parse_bare_key(chars, start),
        _ => None,
    }
}

fn parse_quoted_key(chars: &[char], start: usize) -> Option<ParsedKey> {
    let quote = chars[start];
    let key_start = start + 1;
    let mut index = key_start;
    while index < chars.len() {
        if chars[index] == quote {
            return Some(ParsedKey {
                range: key_start..index,
                after_key: index + 1,
            });
        }
        index += 1;
    }
    None
}

fn parse_bare_key(chars: &[char], start: usize) -> Option<ParsedKey> {
    let mut end = start;
    while matches!(chars.get(end), Some(ch) if is_key_char(*ch)) {
        end += 1;
    }
    (end > start).then_some(ParsedKey {
        range: start..end,
        after_key: end,
    })
}

fn append_redacted_value(
    chars: &[char],
    value_start: usize,
    consume_line: bool,
    output: &mut String,
) -> usize {
    let Some(first) = chars.get(value_start).copied() else {
        output.push_str(REDACTED_VALUE);
        return value_start;
    };

    if matches!(first, '"' | '\'') {
        output.push(first);
        output.push_str(REDACTED_VALUE);
        output.push(first);
        return skip_quoted_value(chars, value_start, first);
    }

    output.push_str(REDACTED_VALUE);
    if consume_line {
        skip_line_value(chars, value_start)
    } else {
        skip_bare_value(chars, value_start)
    }
}

fn skip_quoted_value(chars: &[char], start: usize, quote: char) -> usize {
    let mut index = start + 1;
    let mut escaped = false;
    while index < chars.len() {
        let ch = chars[index];
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return index + 1;
        }
        index += 1;
    }
    chars.len()
}

fn skip_bare_value(chars: &[char], start: usize) -> usize {
    let mut index = start;
    while let Some(ch) = chars.get(index) {
        if ch.is_whitespace() || matches!(ch, ',' | '}' | ']' | ';') {
            break;
        }
        index += 1;
    }
    index
}

fn skip_line_value(chars: &[char], start: usize) -> usize {
    let mut index = start;
    while let Some(ch) = chars.get(index) {
        if matches!(ch, '\n' | '\r') {
            break;
        }
        index += 1;
    }
    index
}

fn skip_ascii_whitespace(chars: &[char], start: usize) -> usize {
    let mut index = start;
    while matches!(chars.get(index), Some(ch) if ch.is_ascii_whitespace()) {
        index += 1;
    }
    index
}

fn is_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')
}

fn is_sensitive_key(key: &[char]) -> bool {
    let normalized = key
        .iter()
        .collect::<String>()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .to_ascii_lowercase()
        .replace('-', "_");
    SENSITIVE_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn is_sensitive_header_key(key: &[char]) -> bool {
    let normalized = key
        .iter()
        .collect::<String>()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .to_ascii_lowercase()
        .replace('-', "_");
    normalized == "authorization"
}

fn truncate_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut text = value
        .chars()
        .take(max_chars.saturating_sub(8))
        .collect::<String>();
    text.push_str(" ...已截断");
    text
}

#[cfg(test)]
mod tests {
    use super::executed_command_preview;

    #[test]
    fn executed_command_preview_removes_powershell_bootstrap() {
        let value = r"pwsh -NoLogo -NoProfile -NonInteractive -Command try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {} $target = 'E:\zbg\axon-code'; git status --short";

        assert_eq!(
            executed_command_preview(value),
            "$target = 'E:\\zbg\\axon-code'; git status --short"
        );
    }

    #[test]
    fn executed_command_preview_extracts_script_from_tool_arguments_json() {
        let value = r#"{"command":"git diff --stat","cwd":"."}"#;

        assert_eq!(executed_command_preview(value), "git diff --stat");
    }

    #[test]
    fn executed_command_preview_keeps_unknown_commands_intact() {
        assert_eq!(executed_command_preview("echo ready"), "echo ready");
    }
}
