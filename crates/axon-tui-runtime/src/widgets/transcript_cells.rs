//! Transcript cell 共享排版 primitives。

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    surface::{display_width::DisplayWidthText, visual_density::NEUTRAL_PREFIX},
    widgets::adaptive_wrap::adaptive_wrap_text,
    widgets::transcript_style::{CellVisual, startup_line_spans},
};

pub(super) const CELL_DETAIL_LIMIT: usize = 140;
pub(super) const DIFF_FILE_LIMIT: usize = 8;

pub(super) fn render_startup_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    text.lines()
        .flat_map(|line| {
            if line.trim().is_empty() {
                return vec![Line::from("")];
            }
            wrap_preserving_prefix(line, width)
                .into_iter()
                .map(|chunk| Line::from(startup_line_spans(chunk)))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn wrap_preserving_prefix(text: &str, width: u16) -> Vec<String> {
    textwrap::wrap(
        text,
        textwrap::Options::new(usize::from(width).max(1)).break_words(true),
    )
    .into_iter()
    .map(std::borrow::Cow::into_owned)
    .collect()
}

pub(super) fn normalize_and_truncate_cell_text(text: &str, limit: usize) -> String {
    truncate_cell_text(&normalize_cell_whitespace(text), limit)
}

pub(super) fn normalize_cell_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_cell_text(text: &str, limit: usize) -> String {
    DisplayWidthText::ellipsize(text, limit)
}

pub(super) fn render_prefixed_block(
    label: &str,
    text: &str,
    label_style: Style,
    text_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let label_prefix = if label.is_empty() {
        NEUTRAL_PREFIX.to_owned()
    } else {
        format!("{NEUTRAL_PREFIX}{label} ")
    };
    let continuation_prefix = " ".repeat(display_width(&label_prefix));
    render_wrapped_block(
        &label_prefix,
        &continuation_prefix,
        text,
        label_style,
        text_style,
        width,
    )
}

pub(super) fn render_detail_block(
    visual: CellVisual,
    prefix: &str,
    value: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let continuation = " ".repeat(display_width(prefix));
    render_wrapped_block(
        prefix,
        &continuation,
        value,
        visual.detail_prefix_style,
        visual.detail_text_style,
        width,
    )
}

pub(super) fn render_wrapped_block(
    first_prefix: &str,
    continuation_prefix: &str,
    text: &str,
    prefix_style: Style,
    text_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let body_width = usable_body_width(width, first_prefix);
    adaptive_wrap_text(text, body_width)
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let prefix = if index == 0 {
                first_prefix
            } else {
                continuation_prefix
            };
            Line::from(vec![
                Span::styled(prefix.to_owned(), prefix_style),
                Span::styled(chunk, text_style),
            ])
        })
        .collect()
}

fn usable_body_width(width: u16, prefix: &str) -> usize {
    usize::from(width)
        .saturating_sub(display_width(prefix))
        .max(1)
}

pub(super) fn display_width(value: &str) -> usize {
    DisplayWidthText::width(value)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::{render_detail_block, render_prefixed_block};
    use crate::widgets::transcript_style::CellVisual;

    #[test]
    fn prefixed_block_omits_empty_primary_text() {
        assert!(
            render_prefixed_block("修改", "", Style::default(), Style::default(), 80).is_empty()
        );
    }

    #[test]
    fn detail_block_omits_empty_text_without_placeholder() {
        let visual = CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        };

        assert!(render_detail_block(visual, "  · ", "", 80).is_empty());
    }
}
