//! Agent 回复 Markdown fence 预处理。

use std::borrow::Cow;

use super::markdown_table::{
    is_markdown_fence_info, parse_fence_marker, source_contains_pipe_table, strip_blockquote_prefix,
};

pub(super) fn unwrap_markdown_fences(source: &str) -> Cow<'_, str> {
    if !source.contains("```") && !source.contains("~~~") {
        return Cow::Borrowed(source);
    }
    if let Some(body) = single_outer_markdown_fence_body(source) {
        return Cow::Owned(body.to_owned());
    }

    let mut output = String::with_capacity(source.len());
    let mut lines = source.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let Some(opening) = parse_fence(line) else {
            output.push_str(line);
            continue;
        };

        if !opening.is_markdown {
            output.push_str(line);
            copy_fence_passthrough(&mut output, &mut lines, opening);
            continue;
        }

        let mut body = String::new();
        let mut closing = String::new();
        for candidate in lines.by_ref() {
            if is_closing_fence(candidate, opening) {
                closing.push_str(candidate);
                break;
            }
            body.push_str(candidate);
        }

        if source_contains_pipe_table(&body, opening.blockquoted) {
            output.push_str(&body);
        } else {
            output.push_str(line);
            output.push_str(&body);
            output.push_str(&closing);
        }
    }

    Cow::Owned(output)
}

fn single_outer_markdown_fence_body(source: &str) -> Option<&str> {
    let trimmed = source.trim();
    let mut lines = trimmed.split_inclusive('\n');
    let opening_line = lines.next()?;
    let opening = parse_fence(opening_line)?;
    if !opening.is_markdown || opening.blockquoted {
        return None;
    }

    let body_start = opening_line.len();
    let mut candidate_start = body_start;
    for candidate in lines {
        if is_closing_fence(candidate, opening) {
            let closing_end = candidate_start.saturating_add(candidate.len());
            return trimmed[closing_end..]
                .trim()
                .is_empty()
                .then_some(&trimmed[body_start..candidate_start]);
        }
        candidate_start = candidate_start.saturating_add(candidate.len());
    }
    None
}

#[derive(Debug, Clone, Copy)]
struct FenceOpening {
    marker: char,
    len: usize,
    is_markdown: bool,
    blockquoted: bool,
}

fn copy_fence_passthrough(
    output: &mut String,
    lines: &mut std::iter::Peekable<std::str::SplitInclusive<'_, char>>,
    opening: FenceOpening,
) {
    for line in lines.by_ref() {
        output.push_str(line);
        if is_closing_fence(line, opening) {
            break;
        }
    }
}

fn parse_fence(line: &str) -> Option<FenceOpening> {
    let trimmed = strip_fence_indent(line)?;
    let blockquoted = trimmed.trim_start().starts_with('>');
    let scan = strip_blockquote_prefix(trimmed);
    let (marker, len) = parse_fence_marker(scan)?;
    Some(FenceOpening {
        marker,
        len,
        is_markdown: is_markdown_fence_info(scan, len),
        blockquoted,
    })
}

fn is_closing_fence(line: &str, opening: FenceOpening) -> bool {
    let Some(trimmed) = strip_fence_indent(line) else {
        return false;
    };
    let scan = if opening.blockquoted {
        if !trimmed.trim_start().starts_with('>') {
            return false;
        }
        strip_blockquote_prefix(trimmed)
    } else {
        trimmed
    };
    parse_fence_marker(scan).is_some_and(|(marker, len)| {
        marker == opening.marker && len >= opening.len && fence_tail(scan, len).is_empty()
    })
}

fn strip_fence_indent(line: &str) -> Option<&str> {
    let without_newline = line.trim_end_matches(['\n', '\r']);
    let mut byte_index = 0usize;
    let mut columns = 0usize;
    for byte in without_newline.as_bytes() {
        match byte {
            b' ' => {
                byte_index += 1;
                columns += 1;
            }
            b'\t' => {
                byte_index += 1;
                columns += 4;
            }
            _ => break,
        }
        if columns >= 4 {
            return None;
        }
    }
    Some(&without_newline[byte_index..])
}

fn fence_tail(line: &str, marker_len: usize) -> &str {
    let marker_end = line
        .char_indices()
        .nth(marker_len)
        .map_or(line.len(), |(index, _)| index);
    line[marker_end..].trim()
}

#[cfg(test)]
mod tests {
    use super::unwrap_markdown_fences;

    #[test]
    fn unwraps_markdown_fence_when_body_contains_table() {
        let source = "```markdown\n| 文件 | 状态 |\n| --- | --- |\n| a.rs | ok |\n```\n";
        let normalized = unwrap_markdown_fences(source);

        assert!(normalized.starts_with("| 文件 | 状态 |"));
        assert!(!normalized.contains("```markdown"));
    }

    #[test]
    fn keeps_unclosed_markdown_fence_as_code_until_table_is_confirmed() {
        let source = "```markdown\n### 处理结果\n- 已完成\n";
        let normalized = unwrap_markdown_fences(source);

        assert_eq!(normalized.as_ref(), source);
    }

    #[test]
    fn unwraps_blockquoted_markdown_fence_table() {
        let source = "> ```markdown\n> | 文件 | 状态 |\n> | --- | --- |\n> | a.rs | ok |\n> ```\n";
        let normalized = unwrap_markdown_fences(source);

        assert!(!normalized.contains("```markdown"));
        assert!(normalized.contains("| 文件 | 状态 |"));
    }

    #[test]
    fn keeps_non_markdown_fence_as_code() {
        let source = "```rust\nfn main() {}\n```\n";
        let normalized = unwrap_markdown_fences(source);

        assert_eq!(normalized.as_ref(), source);
    }

    #[test]
    fn unwraps_single_outer_markdown_fence_without_table() {
        let source = "```md\n### 标题\n\n- 内容\n```";

        assert_eq!(unwrap_markdown_fences(source), "### 标题\n\n- 内容\n");
    }

    #[test]
    fn keeps_embedded_markdown_fence_as_code() {
        let source = "下面是 Markdown 示例：\n\n```markdown\n### 标题\n\n- 内容\n```";

        assert_eq!(unwrap_markdown_fences(source), source);
    }
}
