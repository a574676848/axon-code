//! Markdown 行级换行与 hyperlink 列区间重映射。

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::surface::display_width::DisplayWidthText;
use crate::widgets::{
    adaptive_wrap::{adaptive_wrap_reflow_text_with_hanging_indent, adaptive_wrap_text},
    terminal_hyperlinks::{HyperlinkLine, TerminalHyperlink},
    transcript_cells::display_width,
};

pub(super) fn wrap_line_with_prefix(
    line: HyperlinkLine,
    prefix: &str,
    wrap_prefix: &str,
    prefix_style: Style,
    body_width: usize,
) -> Vec<HyperlinkLine> {
    let plain = line
        .line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    if plain.trim().is_empty() {
        return vec![HyperlinkLine::from("")];
    }
    let content_width = content_width_for_prefixes(body_width, prefix, wrap_prefix);
    let source_chars = line_styled_chars(&line.line);
    let source_column_offsets = source_column_offsets(&plain);
    let mut source_index = 0usize;
    adaptive_wrap_text(&plain, content_width)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let rendered_prefix = if index == 0 { prefix } else { wrap_prefix }.to_owned();
            skip_source_whitespace(&source_chars, &mut source_index, &chunk);
            let source_start = source_column_offsets[source_index];
            let prefix_columns = display_width(&rendered_prefix);
            let mut spans = vec![Span::styled(rendered_prefix, prefix_style)];
            spans.extend(styled_chunk_spans(
                &source_chars,
                &mut source_index,
                &chunk,
                line_style(&line.line),
            ));
            let source_end = source_column_offsets[source_index];
            HyperlinkLine {
                line: Line::from(spans),
                hyperlinks: remap_hyperlinks(
                    &line.hyperlinks,
                    source_start..source_end,
                    prefix_columns,
                ),
            }
        })
        .collect()
}

pub(super) fn wrap_reflow_line(
    line: HyperlinkLine,
    width: usize,
    continuation_indent: usize,
) -> Vec<HyperlinkLine> {
    let plain = line_text(&line.line);
    if plain.trim().is_empty() {
        return vec![HyperlinkLine::from("")];
    }
    let continuation_indent = continuation_indent.min(width.saturating_sub(1));
    let source_chars = line_styled_chars(&line.line);
    let source_column_offsets = source_column_offsets(&plain);
    let mut source_index = 0usize;
    adaptive_wrap_reflow_text_with_hanging_indent(&plain, width, continuation_indent)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let rendered_prefix = if index == 0 {
                String::new()
            } else {
                " ".repeat(continuation_indent)
            };
            skip_source_whitespace(&source_chars, &mut source_index, &chunk);
            let source_start = source_column_offsets[source_index];
            let prefix_columns = display_width(&rendered_prefix);
            let mut spans = vec![Span::raw(rendered_prefix)];
            spans.extend(styled_chunk_spans(
                &source_chars,
                &mut source_index,
                &chunk,
                line_style(&line.line),
            ));
            let source_end = source_column_offsets[source_index];
            HyperlinkLine {
                line: Line::from(spans),
                hyperlinks: remap_hyperlinks(
                    &line.hyperlinks,
                    source_start..source_end,
                    prefix_columns,
                ),
            }
        })
        .collect()
}

pub(super) fn wrap_preformatted_line(
    line: HyperlinkLine,
    width: usize,
    continuation_indent: usize,
) -> Vec<HyperlinkLine> {
    let plain = line_text(&line.line);
    if plain.trim().is_empty() {
        return vec![HyperlinkLine::from("")];
    }
    let continuation_indent = continuation_indent.min(width.saturating_sub(1));
    let source_chars = line_styled_chars(&line.line);
    let source_column_offsets = source_column_offsets(&plain);
    let mut source_index = 0usize;
    hard_wrap_preformatted(&plain, width, continuation_indent)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let rendered_prefix = if index == 0 {
                String::new()
            } else {
                " ".repeat(continuation_indent)
            };
            let source_start = source_column_offsets[source_index];
            let prefix_columns = display_width(&rendered_prefix);
            let mut spans = vec![Span::raw(rendered_prefix)];
            spans.extend(styled_chunk_spans(
                &source_chars,
                &mut source_index,
                &chunk,
                line_style(&line.line),
            ));
            let source_end = source_column_offsets[source_index];
            HyperlinkLine {
                line: Line::from(spans),
                hyperlinks: remap_hyperlinks(
                    &line.hyperlinks,
                    source_start..source_end,
                    prefix_columns,
                ),
            }
        })
        .collect()
}

pub(super) fn wrap_lines_without_prefix(
    lines: Vec<HyperlinkLine>,
    width: usize,
) -> Vec<HyperlinkLine> {
    lines
        .into_iter()
        .flat_map(|line| {
            let leading_width = line_leading_width(&line.line);
            if leading_width > 0 {
                wrap_preformatted_line(line, width, leading_width)
            } else {
                wrap_line_with_prefix(line, "", "", Style::default(), width)
            }
        })
        .collect()
}

fn content_width_for_prefixes(body_width: usize, prefix: &str, wrap_prefix: &str) -> usize {
    let extra_wrap_prefix_width = display_width(wrap_prefix).saturating_sub(display_width(prefix));
    body_width.saturating_sub(extra_wrap_prefix_width).max(1)
}

fn hard_wrap_preformatted(
    text: &str,
    first_width: usize,
    continuation_indent: usize,
) -> Vec<String> {
    let continuation_width = first_width.saturating_sub(continuation_indent).max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut budget = first_width.max(1);
    for grapheme in text.graphemes(true) {
        let grapheme_width = DisplayWidthText::width(grapheme);
        if current_width > 0 && current_width.saturating_add(grapheme_width) > budget {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
            budget = continuation_width;
        }
        current.push_str(grapheme);
        current_width = current_width.saturating_add(grapheme_width);
        if current_width >= budget {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
            budget = continuation_width;
        }
    }
    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn line_leading_width(line: &Line<'_>) -> usize {
    let leading = line
        .spans
        .iter()
        .flat_map(|span| span.content.chars())
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    display_width(&leading)
}

fn line_styled_chars(line: &Line<'_>) -> Vec<(char, Style)> {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars().map(|ch| (ch, span.style)))
        .collect()
}

fn styled_chunk_spans(
    source_chars: &[(char, Style)],
    source_index: &mut usize,
    chunk: &str,
    fallback_style: Style,
) -> Vec<Span<'static>> {
    skip_source_whitespace_with_first(source_chars, source_index, chunk.chars().next());

    let mut spans = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;
    for chunk_char in chunk.chars() {
        let style = source_chars
            .get(*source_index)
            .and_then(|(source_char, source_style)| {
                if *source_char == chunk_char {
                    Some(*source_style)
                } else {
                    None
                }
            })
            .unwrap_or(fallback_style);
        if source_chars.get(*source_index).is_some() {
            *source_index = source_index.saturating_add(1);
        }
        push_styled_char(
            &mut spans,
            &mut current_text,
            &mut current_style,
            chunk_char,
            style,
        );
    }
    flush_styled_text(&mut spans, &mut current_text, current_style);
    spans
}

fn skip_source_whitespace(source_chars: &[(char, Style)], source_index: &mut usize, chunk: &str) {
    skip_source_whitespace_with_first(source_chars, source_index, chunk.chars().next());
}

fn skip_source_whitespace_with_first(
    source_chars: &[(char, Style)],
    source_index: &mut usize,
    first_chunk_char: Option<char>,
) {
    while let Some((source_char, _)) = source_chars.get(*source_index)
        && source_char.is_whitespace()
        && Some(*source_char) != first_chunk_char
    {
        *source_index = source_index.saturating_add(1);
    }
}

fn source_column_offsets(text: &str) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(text.chars().count().saturating_add(1));
    let mut columns = 0usize;
    offsets.push(columns);
    for grapheme in text.graphemes(true) {
        columns = columns.saturating_add(DisplayWidthText::width(grapheme));
        offsets.extend(std::iter::repeat_n(columns, grapheme.chars().count()));
    }
    offsets
}

fn remap_hyperlinks(
    hyperlinks: &[TerminalHyperlink],
    source: std::ops::Range<usize>,
    prefix_columns: usize,
) -> Vec<TerminalHyperlink> {
    hyperlinks
        .iter()
        .filter_map(|hyperlink| {
            let start = hyperlink.columns.start.max(source.start);
            let end = hyperlink.columns.end.min(source.end);
            (start < end).then(|| TerminalHyperlink {
                columns: prefix_columns + start.saturating_sub(source.start)
                    ..prefix_columns + end.saturating_sub(source.start),
                destination: hyperlink.destination.clone(),
            })
        })
        .collect()
}

fn push_styled_char(
    spans: &mut Vec<Span<'static>>,
    current_text: &mut String,
    current_style: &mut Option<Style>,
    ch: char,
    style: Style,
) {
    if current_style
        .as_ref()
        .is_some_and(|active_style| *active_style != style)
    {
        let active_style = current_style.take().unwrap_or_default();
        if !current_text.is_empty() {
            spans.push(Span::styled(std::mem::take(current_text), active_style));
        }
    }
    *current_style = Some(style);
    current_text.push(ch);
}

fn flush_styled_text(
    spans: &mut Vec<Span<'static>>,
    current_text: &mut String,
    current_style: Option<Style>,
) {
    if !current_text.is_empty() {
        spans.push(Span::styled(
            std::mem::take(current_text),
            current_style.unwrap_or_default(),
        ));
    }
}

fn line_style(line: &Line<'_>) -> Style {
    line.spans
        .first()
        .map(|span| span.style)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::source_column_offsets;

    const SOURCE_TEXT: &str = "a\t中e\u{301}";
    const EXPECTED_OFFSETS: [usize; 6] = [0, 1, 5, 7, 8, 8];

    #[test]
    fn source_column_offsets_preserve_grapheme_display_widths() {
        assert_eq!(source_column_offsets(SOURCE_TEXT), EXPECTED_OFFSETS);
    }
}
