//! Markdown pipe table 结构检测。
//!
//! 渲染、fence 解包、输入规范化与 streaming holdback 共用这一套检测逻辑，
//! 避免各层对 `|` 表格语法作出不一致判断。

/// 当前 source 行所在的 fence 上下文。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceKind {
    /// 不在 fence 内。
    Outside,
    /// 在可继续解析 Markdown 的 fence 内。
    Markdown,
    /// 在代码或其他非 Markdown fence 内。
    Other,
}

/// 增量跟踪 Markdown fence 上下文，供规范化和流式表格扫描共用。
#[derive(Clone, Copy, Debug)]
pub struct FenceTracker {
    state: Option<(char, usize, FenceKind)>,
}

impl Default for FenceTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl FenceTracker {
    /// 创建一个新的 FenceTracker，初始状态为 Outside。
    #[must_use]
    pub const fn new() -> Self {
        Self { state: None }
    }

    /// 返回当前 FenceTracker 的 FenceKind。
    #[must_use]
    pub fn kind(self) -> FenceKind {
        self.state.map_or(FenceKind::Outside, |(_, _, kind)| kind)
    }

    /// 判断当前 FenceTracker 是否处于 Inside 状态。
    #[must_use]
    pub fn is_inside(self) -> bool {
        self.kind() != FenceKind::Outside
    }

    /// 根据输入行更新 FenceTracker 的状态。
    pub fn advance(&mut self, raw_line: &str) {
        let Some(stripped) = strip_fence_indent(raw_line) else {
            return;
        };
        let scan = strip_blockquote_prefix(stripped);
        let Some((marker, len)) = parse_fence_marker(scan) else {
            return;
        };

        if let Some((open_marker, open_len, _)) = self.state {
            if marker == open_marker && len >= open_len && fence_tail(scan, len).is_empty() {
                self.state = None;
            }
            return;
        }

        let kind = if is_markdown_fence_info(scan, len) {
            FenceKind::Markdown
        } else {
            FenceKind::Other
        };
        self.state = Some((marker, len, kind));
    }
}

/// 返回可作为 Markdown pipe table 行解析的规范文本。
#[must_use]
pub fn table_candidate_text(line: &str) -> Option<&str> {
    let stripped = strip_blockquote_prefix(line).trim();
    parse_table_segments(stripped).map(|_| stripped)
}

pub(super) fn source_contains_pipe_table(source: &str, blockquoted_fence: bool) -> bool {
    let mut previous: Option<&str> = None;
    for line in source.lines() {
        let text = if blockquoted_fence {
            strip_blockquote_prefix(line)
        } else {
            line
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            previous = None;
            continue;
        }
        if let Some(header) = previous
            && is_table_header_line(header)
            && !is_table_delimiter_line(header)
            && is_table_delimiter_line(trimmed)
        {
            return true;
        }
        previous = Some(trimmed);
    }
    false
}

pub(super) fn parse_table_segments(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let has_outer_pipe = trimmed.starts_with('|') || trimmed.ends_with('|');
    let content = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let content = content.strip_suffix('|').unwrap_or(content);
    let raw_segments = split_unescaped_pipe(content);
    if !has_outer_pipe && raw_segments.len() <= 1 {
        return None;
    }

    let segments = raw_segments.into_iter().map(str::trim).collect::<Vec<_>>();
    (!segments.is_empty()).then_some(segments)
}

/// 判断文本是否具有 Markdown pipe table 表头或数据行结构。
#[must_use]
pub fn is_table_header_line(line: &str) -> bool {
    parse_table_segments(line).is_some_and(|segments| segments.iter().any(|cell| !cell.is_empty()))
}

/// 判断文本是否为 Markdown pipe table 分隔行。
#[must_use]
pub fn is_table_delimiter_line(line: &str) -> bool {
    parse_table_segments(line)
        .is_some_and(|segments| segments.into_iter().all(is_table_delimiter_segment))
}

/// 移除一层或多层 blockquote 前缀并返回正文。
#[must_use]
pub fn strip_blockquote_prefix(line: &str) -> &str {
    let mut rest = line.trim_start();
    loop {
        let Some(stripped) = rest.strip_prefix('>') else {
            return rest;
        };
        rest = stripped.strip_prefix(' ').unwrap_or(stripped).trim_start();
    }
}

/// 解析反引号或波浪线 fence 的标记字符与连续长度。
#[must_use]
pub fn parse_fence_marker(line: &str) -> Option<(char, usize)> {
    let first = line.as_bytes().first().copied()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = line.bytes().take_while(|byte| *byte == first).count();
    if len < 3 {
        return None;
    }
    Some((first as char, len))
}

pub(super) fn is_markdown_fence_info(line: &str, marker_len: usize) -> bool {
    let info = fence_tail(line, marker_len)
        .split_whitespace()
        .next()
        .unwrap_or_default();
    info.eq_ignore_ascii_case("md") || info.eq_ignore_ascii_case("markdown")
}

fn split_unescaped_pipe(content: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let bytes = content.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            index = index.saturating_add(2);
        } else if bytes[index] == b'|' {
            segments.push(&content[start..index]);
            start = index + 1;
            index += 1;
        } else {
            index += 1;
        }
    }
    segments.push(&content[start..]);
    segments
}

fn is_table_delimiter_segment(segment: &str) -> bool {
    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return false;
    }
    let without_leading = trimmed.strip_prefix(':').unwrap_or(trimmed);
    let without_ends = without_leading.strip_suffix(':').unwrap_or(without_leading);
    without_ends.len() >= 3 && without_ends.chars().all(|ch| ch == '-')
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
    use super::*;

    #[test]
    fn detects_pipe_tables_without_outer_pipes_and_escaped_pipes() {
        assert_eq!(parse_table_segments("A | B | C"), Some(vec!["A", "B", "C"]));
        assert_eq!(
            parse_table_segments(r"| A \| B | C |"),
            Some(vec![r"A \| B", "C"])
        );
        assert!(is_table_header_line("A | B"));
        assert!(is_table_delimiter_line("--- | :---:"));
    }

    #[test]
    fn tracks_non_markdown_and_blockquoted_markdown_fences() {
        let mut tracker = FenceTracker::new();
        tracker.advance("> ```markdown");
        assert_eq!(tracker.kind(), FenceKind::Markdown);
        tracker.advance("> ```");
        assert_eq!(tracker.kind(), FenceKind::Outside);

        tracker.advance("```rust");
        assert!(tracker.is_inside());
        tracker.advance("```");
        assert_eq!(tracker.kind(), FenceKind::Outside);
    }

    #[test]
    fn source_table_detection_requires_adjacent_header_and_delimiter() {
        assert!(source_contains_pipe_table(
            "| A | B |\n| --- | --- |\n",
            false
        ));
        assert!(source_contains_pipe_table(
            "> | A | B |\n> | --- | --- |\n",
            true
        ));
        assert!(!source_contains_pipe_table(
            "| A | B |\n\n| --- | --- |\n",
            false
        ));
    }
}
