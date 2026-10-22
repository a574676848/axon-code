//! Codex 风格的 transcript 自适应换行。

use crate::surface::display_width::{DisplayWidthText, WrapPolicy};

const URL_SCHEMES: [&str; 4] = ["http://", "https://", "file://", "ssh://"];

pub(super) fn adaptive_wrap_text(text: &str, width: usize) -> Vec<String> {
    adaptive_wrap_text_with_hanging_indent(text, width, 0)
}

pub(super) fn adaptive_wrap_text_with_hanging_indent(
    text: &str,
    width: usize,
    continuation_indent: usize,
) -> Vec<String> {
    adaptive_wrap_text_with_policy(text, width, continuation_indent, WrapPolicy::TRANSCRIPT)
}

pub(super) fn adaptive_wrap_reflow_text_with_hanging_indent(
    text: &str,
    width: usize,
    continuation_indent: usize,
) -> Vec<String> {
    adaptive_wrap_text_with_policy(
        text,
        width,
        continuation_indent,
        WrapPolicy {
            preserve_preformatted: false,
            preserve_url: false,
            ..WrapPolicy::TRANSCRIPT
        },
    )
}

fn adaptive_wrap_text_with_policy(
    text: &str,
    width: usize,
    continuation_indent: usize,
    policy: WrapPolicy,
) -> Vec<String> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines()
        .flat_map(|line| wrap_line(line, width.max(1), continuation_indent, policy))
        .collect()
}

fn wrap_line(
    line: &str,
    width: usize,
    continuation_indent: usize,
    policy: WrapPolicy,
) -> Vec<String> {
    if line.trim().is_empty() {
        return vec![String::new()];
    }
    if policy.preserve_preformatted && is_preformatted_line(line) {
        return vec![line.to_owned()];
    }
    let trimmed = if policy.trim { line.trim() } else { line };
    if policy.preserve_url && line_has_standalone_url(trimmed) {
        return vec![trimmed.to_owned()];
    }
    let continuation_indent = continuation_indent.min(width.saturating_sub(1));
    let continuation_prefix = " ".repeat(continuation_indent);
    textwrap::wrap(
        trimmed,
        textwrap::Options::new(width)
            .subsequent_indent(&continuation_prefix)
            .break_words(policy.break_words)
            .word_splitter(textwrap::WordSplitter::NoHyphenation),
    )
    .into_iter()
    .enumerate()
    .flat_map(|(index, chunk)| {
        let chunk = if index == 0 {
            chunk.as_ref()
        } else {
            chunk
                .strip_prefix(&continuation_prefix)
                .unwrap_or(chunk.as_ref())
        };
        let chunk_width = if index == 0 {
            width
        } else {
            width.saturating_sub(continuation_indent).max(1)
        };
        wrap_overwide_chunk(chunk, chunk_width)
    })
    .collect()
}

fn wrap_overwide_chunk(chunk: &str, width: usize) -> Vec<String> {
    if DisplayWidthText::width(chunk) <= width {
        vec![chunk.to_owned()]
    } else {
        DisplayWidthText::hard_wrap(chunk, width)
    }
}

fn is_preformatted_line(line: &str) -> bool {
    line.starts_with("    ") || line.starts_with('\t')
}

fn line_has_standalone_url(line: &str) -> bool {
    line.split_whitespace()
        .any(|token| is_url_token(trim_wrapping_punctuation(token)))
}

pub(super) fn is_url_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    URL_SCHEMES.iter().any(|scheme| lower.starts_with(scheme)) || lower.contains("://")
}

fn trim_wrapping_punctuation(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '"' | '\''
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        adaptive_wrap_reflow_text_with_hanging_indent, adaptive_wrap_text,
        adaptive_wrap_text_with_hanging_indent,
    };
    use crate::surface::display_width::DisplayWidthText;

    #[test]
    fn keeps_url_tokens_unbroken() {
        let url = "https://github.com/a574676848/axon-code/issues/123456789";
        let lines = adaptive_wrap_text(url, 20);

        assert_eq!(lines, vec![url]);
    }

    #[test]
    fn reflow_hard_wraps_url_token_that_exceeds_width() {
        const WIDTH: usize = 20;
        let url = "https://github.com/a574676848/axon-code/issues/123456789";
        let lines = adaptive_wrap_reflow_text_with_hanging_indent(url, WIDTH, 0);

        assert!(lines.len() > 1, "超宽 URL 必须换行：{lines:?}");
        assert!(
            lines
                .iter()
                .all(|line| DisplayWidthText::width(line) <= WIDTH),
            "URL 换行后每行都必须处于显示宽度预算内：{lines:?}"
        );
        assert_eq!(lines.concat(), url);
    }

    #[test]
    fn keeps_preformatted_indentation() {
        let lines = adaptive_wrap_text("    let value = call();", 10);

        assert_eq!(lines, vec!["    let value = call();"]);
    }

    #[test]
    fn hanging_indent_uses_full_first_line_width() {
        let lines = adaptive_wrap_text_with_hanging_indent("- alpha beta gamma delta", 16, 2);

        assert_eq!(lines, vec!["- alpha beta", "gamma delta"]);
    }

    #[test]
    fn reflow_wraps_indented_list_content() {
        const WIDTH: usize = 16;
        let lines = adaptive_wrap_reflow_text_with_hanging_indent(
            "    - 嵌套列表内容必须在窄宽度下完整展示",
            WIDTH,
            4,
        );

        assert!(lines.len() > 1, "嵌套列表重排行必须触发换行：{lines:?}");
        assert!(
            lines
                .iter()
                .all(|line| DisplayWidthText::width(line) <= WIDTH),
            "重排行不得返回超宽内容：{lines:?}"
        );
    }
}
