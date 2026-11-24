//! 工具输出有界缓存。
//!
//! 缓冲区承载两类内容：上游已按行分页的工具结果详情（`read_file` 等），以及运行时流式
//! 分片（stdout/stderr）。前者本身有界，内联详情必须逐行完整呈现；后者没有上游边界，
//! 才用头尾保留的中间省略兜底。

use crate::surface::ring_buffer::LineRingBuffer;

const OUTPUT_LINE_SEPARATOR: &str = " │ ";
const OUTPUT_CONTINUATION_PREFIX: &str = "         │ ";
const STREAM_STDERR: &str = "stderr";
const STREAM_STDOUT: &str = "stdout";
const ERROR_TOKEN: &str = "error";
const TOOL_OUTPUT_RING_LINES: usize = 2_000;
const TOOL_OUTPUT_CHUNK_PREVIEW_CHARS: usize = 64 * 1024;
const TOOL_OUTPUT_INLINE_MAX_SEGMENTS: usize = 24;
const TOOL_OUTPUT_INLINE_SEGMENT_CHARS: usize = 160;
const TOOL_OUTPUT_TRUNCATED_NOTICE_PREFIX: &str = "输出已截断";
const TOOL_OUTPUT_INLINE_OMITTED_NOTICE: &str = "内联详情已省略中间输出";
const ANSI_ESCAPE: char = '\x1b';
const ANSI_BELL: char = '\x07';

/// 工具结果详情通道：上游工具已按行分页投递，内容本身有界。
pub(crate) const TOOL_RESULT_DETAIL_STREAM: &str = "result";

/// 按 transcript part id 聚合的有界工具输出。
#[derive(Debug, Clone)]
pub(crate) struct ToolOutputBuffer {
    tool_name: String,
    lines: LineRingBuffer,
    total_bytes: usize,
    truncated_chunks: usize,
    streamed_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolOutputView {
    pub(crate) lines: Vec<String>,
    pub(crate) truncated: bool,
    pub(crate) omitted_lines: usize,
    pub(crate) total_bytes: usize,
    pub(crate) retained_bytes: usize,
    pub(crate) truncated_chunks: usize,
}

impl ToolOutputBuffer {
    pub(crate) fn new(tool_name: &str) -> Self {
        Self {
            tool_name: tool_name.to_owned(),
            lines: LineRingBuffer::new(TOOL_OUTPUT_RING_LINES),
            total_bytes: 0,
            truncated_chunks: 0,
            streamed_output: false,
        }
    }

    pub(crate) fn push_chunk(&mut self, stream: &str, sequence: u64, text: &str) {
        self.total_bytes = self.total_bytes.saturating_add(text.len());
        if !stream.eq_ignore_ascii_case(TOOL_RESULT_DETAIL_STREAM) {
            self.streamed_output = true;
        }
        let bounded = bounded_chunk_text(text);
        if bounded.truncated {
            self.truncated_chunks = self.truncated_chunks.saturating_add(1);
        }
        for line in render_chunk_lines(stream, sequence, &bounded.text) {
            self.lines.push(line);
        }
    }

    pub(crate) fn title(&self) -> String {
        format!("工具输出 · {}", self.tool_name)
    }

    pub(crate) fn view(&self) -> ToolOutputView {
        let mut lines = self.lines.retained_lines();
        let retained_bytes = lines.iter().map(String::len).sum();
        let omitted_lines = self.lines.dropped();
        let truncated = omitted_lines > 0 || self.truncated_chunks > 0;
        if truncated {
            lines.insert(
                0,
                format!(
                    "{TOOL_OUTPUT_TRUNCATED_NOTICE_PREFIX}：省略 {omitted_lines} 行，截断 {} 段，保留约 {} 字节 / 总计 {} 字节",
                    self.truncated_chunks, retained_bytes, self.total_bytes
                ),
            );
        }
        ToolOutputView {
            lines,
            truncated,
            omitted_lines,
            total_bytes: self.total_bytes,
            retained_bytes,
            truncated_chunks: self.truncated_chunks,
        }
    }

    /// 渲染内联详情。
    ///
    /// 工具结果详情（例如 `read_file`）已由上游按行分页，内容本身有界，再次做中间省略
    /// 只会让用户点开详情却看不到正文；流式分片没有上游边界，继续按头尾保留兜底。
    pub(crate) fn inline_text(&self) -> Option<String> {
        let lines = self
            .view()
            .lines
            .into_iter()
            .map(|line| clean_inline_line(&line))
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return None;
        }
        if !self.streamed_output {
            return Some(lines.join("\n"));
        }

        let segments = lines
            .into_iter()
            .flat_map(|line| split_inline_segments(&line))
            .collect::<Vec<_>>();
        if segments.len() <= TOOL_OUTPUT_INLINE_MAX_SEGMENTS {
            return Some(segments.join("\n"));
        }

        let head_count = TOOL_OUTPUT_INLINE_MAX_SEGMENTS / 2;
        let tail_count = TOOL_OUTPUT_INLINE_MAX_SEGMENTS - head_count;
        let omitted_count = segments.len() - head_count - tail_count;
        let mut bounded = segments[..head_count].to_vec();
        bounded.push(format!(
            "{TOOL_OUTPUT_INLINE_OMITTED_NOTICE}：省略 {omitted_count} 段"
        ));
        bounded.extend_from_slice(&segments[segments.len() - tail_count..]);
        Some(bounded.join("\n"))
    }
}

fn split_inline_segments(line: &str) -> Vec<String> {
    let characters = line.chars().collect::<Vec<_>>();
    if characters.is_empty() {
        return vec![String::new()];
    }
    characters
        .chunks(TOOL_OUTPUT_INLINE_SEGMENT_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn clean_inline_line(line: &str) -> String {
    let content = line.split_once(OUTPUT_LINE_SEPARATOR).map_or_else(
        || {
            line.strip_prefix(OUTPUT_CONTINUATION_PREFIX)
                .unwrap_or(line)
        },
        |(_, content)| content,
    );
    let content = content
        .strip_prefix(OUTPUT_CONTINUATION_PREFIX)
        .unwrap_or(content);
    strip_ansi_control_sequences(content)
}

fn strip_ansi_control_sequences(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != ANSI_ESCAPE {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        index += 1;
        let Some(sequence_type) = chars.get(index).copied() else {
            break;
        };
        index += 1;
        match sequence_type {
            '[' => {
                while index < chars.len() {
                    let current = chars[index];
                    index += 1;
                    if ('@'..='~').contains(&current) {
                        break;
                    }
                }
            }
            ']' => {
                while index < chars.len() {
                    if chars[index] == ANSI_BELL {
                        index += 1;
                        break;
                    }
                    if chars[index] == ANSI_ESCAPE && chars.get(index + 1).copied() == Some('\\') {
                        index += 2;
                        break;
                    }
                    index += 1;
                }
            }
            _ => {}
        }
    }

    output
}

#[derive(Debug, Clone)]
struct BoundedChunkText {
    text: String,
    truncated: bool,
}

fn bounded_chunk_text(text: &str) -> BoundedChunkText {
    if text.len() <= TOOL_OUTPUT_CHUNK_PREVIEW_CHARS {
        return BoundedChunkText {
            text: text.to_owned(),
            truncated: false,
        };
    }
    let mut end = TOOL_OUTPUT_CHUNK_PREVIEW_CHARS;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    BoundedChunkText {
        text: text[..end].to_owned(),
        truncated: true,
    }
}

fn render_chunk_lines(stream: &str, sequence: u64, text: &str) -> Vec<String> {
    let label = stream_label(stream);
    let mut lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if index == 0 {
            lines.push(format!("{label}#{sequence}{OUTPUT_LINE_SEPARATOR}{line}"));
        } else {
            lines.push(format!("{OUTPUT_CONTINUATION_PREFIX}{line}"));
        }
    }
    if lines.is_empty() {
        lines.push(format!("{label}#{sequence}{OUTPUT_LINE_SEPARATOR}"));
    }
    lines
}

fn stream_label(stream: &str) -> &'static str {
    if stream.eq_ignore_ascii_case(STREAM_STDERR)
        || stream.to_ascii_lowercase().contains(ERROR_TOKEN)
    {
        "stderr"
    } else if stream.eq_ignore_ascii_case(STREAM_STDOUT) {
        "stdout"
    } else {
        "output"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ANSI_ESCAPE, TOOL_OUTPUT_CHUNK_PREVIEW_CHARS, TOOL_OUTPUT_INLINE_MAX_SEGMENTS,
        TOOL_OUTPUT_INLINE_OMITTED_NOTICE, TOOL_OUTPUT_RING_LINES, TOOL_RESULT_DETAIL_STREAM,
        ToolOutputBuffer,
    };

    #[test]
    fn output_buffer_renders_streams_as_scannable_sections() {
        let mut buffer = ToolOutputBuffer::new("builtin.shell_command");
        buffer.push_chunk("stderr", 2, "error: failed\ncaused by mismatch");
        buffer.push_chunk("stdout", 1, "checking");

        let rendered = buffer.view().lines.join("\n");

        assert!(rendered.contains("stdout#1 │ checking"));
        assert!(rendered.contains("stderr#2 │ error: failed"));
        assert!(rendered.contains("         │ caused by mismatch"));
    }

    #[test]
    fn output_buffer_keeps_bounded_tail_and_reports_truncation() {
        let mut buffer = ToolOutputBuffer::new("builtin.shell_command");
        let mut expected_total_bytes = 0usize;
        for index in 0..(TOOL_OUTPUT_RING_LINES + 5) {
            let line = format!("line {index}");
            expected_total_bytes = expected_total_bytes.saturating_add(line.len());
            buffer.push_chunk("stdout", index as u64, &line);
        }
        let huge = "x".repeat(TOOL_OUTPUT_CHUNK_PREVIEW_CHARS + 1024);
        expected_total_bytes = expected_total_bytes.saturating_add(huge.len());
        buffer.push_chunk("stderr", 9_999, &huge);

        let view = buffer.view();
        let rendered = view.lines.join("\n");

        assert!(view.truncated);
        assert!(view.omitted_lines > 0);
        assert_eq!(view.truncated_chunks, 1);
        assert!(view.lines.len() <= TOOL_OUTPUT_RING_LINES + 1);
        assert!(view.lines[0].contains("输出已截断"));
        assert!(!rendered.contains("line 0"));
        assert_eq!(view.total_bytes, expected_total_bytes);
    }

    #[test]
    fn streamed_inline_text_keeps_real_head_and_tail_without_overloading_transcript() {
        let mut buffer = ToolOutputBuffer::new("builtin.shell_command");
        for index in 0..40 {
            buffer.push_chunk("stdout", index, &format!("result-{index}"));
        }

        let inline = buffer.inline_text().expect("应有内联详情");

        assert!(inline.contains("result-0"));
        assert!(inline.contains("result-39"));
        assert!(inline.contains(TOOL_OUTPUT_INLINE_OMITTED_NOTICE));
        assert!(inline.lines().count() <= TOOL_OUTPUT_INLINE_MAX_SEGMENTS + 1);
    }

    #[test]
    fn bounded_result_detail_keeps_every_paginated_line_inline() {
        let mut buffer = ToolOutputBuffer::new("builtin.read_file");
        let detail = (0..40)
            .map(|index| format!("{index}:result-{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        buffer.push_chunk(TOOL_RESULT_DETAIL_STREAM, 0, &detail);

        let inline = buffer.inline_text().expect("应有内联详情");

        assert!(inline.contains("0:result-0"));
        assert!(inline.contains("39:result-39"));
        assert!(!inline.contains(TOOL_OUTPUT_INLINE_OMITTED_NOTICE));
        assert_eq!(inline.lines().count(), 40);
    }

    #[test]
    fn inline_text_removes_chunk_labels_and_ansi_sequences() {
        let mut buffer = ToolOutputBuffer::new("builtin.shell_command");
        buffer.push_chunk("stdout", 0, "\x1b[32;1mName\x1b[0m\nMode");
        buffer.push_chunk("stderr", 1, "\x1b[31mfailed\x1b[0m");

        let inline = buffer.inline_text().expect("inline output");

        assert_eq!(inline, "Name\nMode\nfailed");
        assert!(!inline.contains("stdout#"));
        assert!(!inline.contains("stderr#"));
        assert!(!inline.contains(ANSI_ESCAPE));
    }
}
