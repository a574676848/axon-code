//! Transcript Markdown 视觉样式。

use pulldown_cmark::HeadingLevel;
use ratatui::style::{Color, Modifier, Style};

use crate::app_shell::{
    style_tokens::{TranscriptColorRole, transcript_palette},
    syntax_theme::table_separator_style,
};

#[derive(Debug, Clone)]
pub(super) struct MarkdownStyles {
    pub(super) body: Style,
    pub(super) heading: Style,
    pub(super) heading_secondary: Style,
    pub(super) heading_tertiary: Style,
    pub(super) heading_quaternary: Style,
    pub(super) blockquote: Style,
    pub(super) strong: Style,
    pub(super) emphasis: Style,
    pub(super) code: Style,
    pub(super) link: Style,
    pub(super) ordered_list_marker: Style,
    pub(super) unordered_list_marker: Style,
    pub(super) marker: Style,
}

impl MarkdownStyles {
    pub(super) fn new(body: Style) -> Self {
        let palette = transcript_palette();
        Self {
            body,
            heading: body
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
            heading_secondary: body.add_modifier(Modifier::BOLD),
            heading_tertiary: body
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::ITALIC),
            heading_quaternary: body.add_modifier(Modifier::ITALIC),
            blockquote: body,
            strong: body.add_modifier(Modifier::BOLD),
            emphasis: body.add_modifier(Modifier::ITALIC),
            code: body.fg(palette.color(TranscriptColorRole::InlineHighlight)),
            link: body.fg(Color::Cyan).add_modifier(Modifier::UNDERLINED),
            ordered_list_marker: body.fg(Color::LightBlue),
            unordered_list_marker: body,
            marker: table_separator_style(),
        }
    }

    pub(super) fn heading(&self, level: HeadingLevel) -> Style {
        match level {
            HeadingLevel::H1 => self.heading,
            HeadingLevel::H2 => self.heading_secondary,
            HeadingLevel::H3 => self.heading_tertiary,
            _ => self.heading_quaternary,
        }
    }
}

pub(super) fn heading_prefix(level: HeadingLevel) -> Option<&'static str> {
    let _ = level;
    None
}
