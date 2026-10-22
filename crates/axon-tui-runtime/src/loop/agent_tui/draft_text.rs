//! Composer 草稿纯文本索引与光标工具。

use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerticalDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LineSpan {
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisualLineSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VisualCursor {
    pub(super) row: u16,
    pub(super) col: u16,
}

pub(super) fn shift_index(index: usize, original_len: usize, replacement_len: usize) -> usize {
    if replacement_len >= original_len {
        index.saturating_add(replacement_len - original_len)
    } else {
        index.saturating_sub(original_len - replacement_len)
    }
}

pub(super) fn vertical_cursor_target(
    value: &str,
    cursor: usize,
    direction: VerticalDirection,
    width: u16,
    target_column: u16,
) -> usize {
    let lines = visual_line_spans(value, width);
    let bounded_cursor = cursor.min(value.chars().count());
    let Some(current_index) = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_index, line)| bounded_cursor >= line.start && bounded_cursor <= line.end)
        .map(|(index, _line)| index)
    else {
        return cursor;
    };
    let target_index = match direction {
        VerticalDirection::Up => current_index.checked_sub(1),
        VerticalDirection::Down => (current_index + 1 < lines.len()).then_some(current_index + 1),
    };
    let Some(target_index) = target_index else {
        return cursor;
    };
    visual_line_column_target(value, lines[target_index], target_column)
}

pub(super) fn current_line_start(value: &str, cursor: usize) -> usize {
    line_spans(value)
        .into_iter()
        .find(|line| cursor >= line.start && cursor <= line.end)
        .map_or(0, |line| line.start)
}

pub(super) fn current_line_end(value: &str, cursor: usize) -> usize {
    line_spans(value)
        .into_iter()
        .find(|line| cursor >= line.start && cursor <= line.end)
        .map_or_else(|| value.chars().count(), |line| line.end)
}

pub(super) fn previous_word_start(value: &str, cursor: usize) -> usize {
    let chars = value.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());
    while index > 0 && chars[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !chars[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}

pub(super) fn next_word_end(value: &str, cursor: usize) -> usize {
    let chars = value.chars().collect::<Vec<_>>();
    let mut index = cursor.min(chars.len());
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    while index < chars.len() && !chars[index].is_whitespace() {
        index += 1;
    }
    index
}

pub(super) fn line_spans(value: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, ch) in value.chars().enumerate() {
        if ch == '\n' {
            spans.push(LineSpan { start, end: index });
            start = index + 1;
        }
    }
    spans.push(LineSpan {
        start,
        end: value.chars().count(),
    });
    spans
}

pub(crate) fn visual_line_spans(value: &str, width: u16) -> Vec<VisualLineSpan> {
    let content_width = width.max(1);
    let chars = value.chars().collect::<Vec<_>>();
    let mut spans = Vec::new();
    for line in line_spans(value) {
        if line.start == line.end {
            spans.push(VisualLineSpan {
                start: line.start,
                end: line.end,
            });
            continue;
        }

        let mut start = line.start;
        let mut col = 0_u16;
        for (index, ch) in chars.iter().enumerate().take(line.end).skip(line.start) {
            let ch_width = display_width(*ch);
            if col > 0 && col.saturating_add(ch_width) > content_width {
                spans.push(VisualLineSpan { start, end: index });
                start = index;
                col = 0;
            }
            col = col.saturating_add(ch_width).min(content_width);
        }
        spans.push(VisualLineSpan {
            start,
            end: line.end,
        });
        if col == content_width && line.end == chars.len() {
            spans.push(VisualLineSpan {
                start: line.end,
                end: line.end,
            });
        }
    }
    if spans.is_empty() {
        spans.push(VisualLineSpan { start: 0, end: 0 });
    }
    spans
}

pub(crate) fn has_multiple_visual_lines(value: &str, width: u16) -> bool {
    visual_line_spans(value, width).len() > 1
}

fn visual_line_column_target(value: &str, span: VisualLineSpan, target_column: u16) -> usize {
    let mut column = 0_u16;
    for (index, ch) in value.chars().enumerate().take(span.end).skip(span.start) {
        let next_column = column.saturating_add(display_width(ch));
        if next_column > target_column {
            return index;
        }
        column = next_column;
    }
    span.end
}

pub(super) fn visual_cursor(value: &str, cursor: usize, width: u16) -> VisualCursor {
    let bounded_cursor = cursor.min(value.chars().count());
    let spans = visual_line_spans(value, width);
    let row = spans
        .iter()
        .enumerate()
        .rev()
        .find(|(_index, span)| span.start <= bounded_cursor && bounded_cursor <= span.end)
        .map(|(index, _span)| u16::try_from(index).unwrap_or(u16::MAX))
        .unwrap_or(0);
    let span = spans
        .get(usize::from(row))
        .copied()
        .unwrap_or(VisualLineSpan { start: 0, end: 0 });
    let col = value
        .chars()
        .skip(span.start)
        .take(bounded_cursor.saturating_sub(span.start))
        .fold(0_u16, |width, ch| width.saturating_add(display_width(ch)));
    VisualCursor { row, col }
}

pub(super) fn byte_index_at(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

pub(super) fn find_char_range(text: &str, needle: &str) -> Option<usize> {
    text.find(needle)
        .map(|byte_index| text[..byte_index].chars().count())
}

pub(super) fn display_width(ch: char) -> u16 {
    u16::try_from(ch.width().unwrap_or(0)).unwrap_or(u16::MAX)
}
