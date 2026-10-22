use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    app_shell::style_tokens::{MESSAGE_CONTINUATION_PREFIX, MESSAGE_INITIAL_PREFIX},
    widgets::{
        terminal_hyperlinks::{prefix_hyperlink_lines, visible_lines},
        transcript_cells::display_width,
        transcript_markdown::render_markdown_hyperlink_lines,
    },
};

pub(super) fn render_message_markdown(
    source: &str,
    body_style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    let prefix_width = u16::try_from(display_width(MESSAGE_INITIAL_PREFIX)).unwrap_or(u16::MAX);
    let body_width = width.saturating_sub(prefix_width).max(1);
    visible_lines(&prefix_hyperlink_lines(
        render_markdown_hyperlink_lines(source, body_style, body_width, None),
        Span::raw(MESSAGE_INITIAL_PREFIX),
        Span::raw(MESSAGE_CONTINUATION_PREFIX),
    ))
}

pub(super) fn rendered_text(source: &str) -> String {
    rendered_plain_lines(source).join("\n")
}

pub(super) fn rendered_plain_lines(source: &str) -> Vec<String> {
    render_message_markdown(source, Style::default(), 120)
        .into_iter()
        .map(|line| line_text(&line))
        .collect()
}

pub(super) fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
