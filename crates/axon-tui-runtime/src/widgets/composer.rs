//! Composer (输入区) Widget - 极简设计，避免视觉噪音。

use std::ops::Range;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Widget},
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::{
        AGENT_TUI_PROMPT_INDICATOR, agent_tui_prompt_width, commands::AgentTuiCommandSuggestion,
        composer_popup::ComposerPopupItem, draft_text::visual_line_spans,
    },
};

use super::{
    CommandPalette, ComposerPopupPalette,
    transcript_style::{role_style, strong_role_style},
};

#[cfg(test)]
const EMPTY_DRAFT_ELEMENTS: &[ComposerDraftElement] = &[];
const COMPOSER_FRAME_MIN_HEIGHT: u16 = 3;

/// Composer 可见草稿投影。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ComposerDraft<'a> {
    pub(crate) text: &'a str,
    pub(crate) elements: &'a [ComposerDraftElement],
}

impl ComposerDraft<'_> {
    #[cfg(test)]
    pub(crate) fn plain(text: &str) -> ComposerDraft<'_> {
        ComposerDraft {
            text,
            elements: EMPTY_DRAFT_ELEMENTS,
        }
    }
}

/// 草稿中的结构化可见元素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerDraftElement {
    pub(crate) range: Range<usize>,
    pub(crate) kind: ComposerDraftElementKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerDraftElementKind {
    Mention(char),
    File,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerDraftLine {
    segments: Vec<ComposerDraftSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerDraftSegment {
    text: String,
    kind: ComposerDraftSegmentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerDraftSegmentKind {
    Plain,
    Mention(char),
    File,
    Image,
}

/// Composer Widget（极简风格）。
pub(crate) struct Composer<'a> {
    /// 用户输入的草稿。
    pub draft: ComposerDraft<'a>,
    /// 空输入提示。
    pub placeholder: &'a str,
    /// 命令建议列表。
    pub command_suggestions: &'a [AgentTuiCommandSuggestion],
    /// 结构化 mention popup 候选项。
    pub popup_items: &'a [ComposerPopupItem],
    /// 命令、文件、Skill 与 connector 弹层标题。
    pub suggestion_title: &'a str,
    /// 当前选中的命令建议索引。
    pub selected_command_suggestion: usize,
    /// 运行中已排队的下一轮输入预览。
    pub queued_preview: Option<&'a str>,
    /// 草稿顶部滚动偏移（行数）；光标移出可见区时动态调整。
    pub draft_scroll_offset: u16,
}

impl<'a> Composer<'a> {
    /// 构建输入行（带提示符）。
    fn build_input_line(
        line: &ComposerDraftLine,
        placeholder: &'a str,
        first_line: bool,
    ) -> Line<'static> {
        let prompt = Span::styled(
            if first_line {
                format!("{AGENT_TUI_PROMPT_INDICATOR} ")
            } else {
                " ".repeat(usize::from(agent_tui_prompt_width()))
            },
            strong_role_style(TranscriptColorRole::ToolCompleted),
        );

        let mut spans = vec![prompt];
        if first_line && line.segments.is_empty() {
            spans.push(Span::styled(
                placeholder.to_owned(),
                role_style(TranscriptColorRole::Muted),
            ));
        } else {
            spans.extend(
                line.segments
                    .iter()
                    .map(|segment| Span::styled(segment.text.clone(), segment_style(segment.kind))),
            );
        }

        Line::from(spans)
    }

    fn command_suggestions_open(&self) -> bool {
        self.draft.text.trim_start().starts_with('/') && !self.command_suggestions.is_empty()
    }

    fn queued_preview_height(&self) -> u16 {
        self.queued_preview
            .map(|preview| 1 + u16::try_from(preview.lines().count()).unwrap_or(u16::MAX))
            .unwrap_or(0)
    }

    fn render_queued_preview(&self, area: Rect, buf: &mut Buffer) {
        let Some(preview) = self.queued_preview else {
            return;
        };
        if area.height == 0 {
            return;
        }
        let title = Line::from(vec![
            Span::styled("待发送", role_style(TranscriptColorRole::Muted)),
            Span::styled(" · ", role_style(TranscriptColorRole::Muted)),
            Span::styled("Alt+E 编辑最近一条", role_style(TranscriptColorRole::Muted)),
        ]);
        buf.set_line(area.x, area.y, &title, area.width);
        for (index, preview_line) in preview
            .lines()
            .take(area.height.saturating_sub(1) as usize)
            .enumerate()
        {
            let body = Line::from(vec![
                Span::styled("↳ ", role_style(TranscriptColorRole::Muted)),
                Span::styled(
                    preview_line.to_owned(),
                    role_style(TranscriptColorRole::Assistant).add_modifier(Modifier::ITALIC),
                ),
            ]);
            buf.set_line(
                area.x,
                area.y
                    .saturating_add(1 + u16::try_from(index).unwrap_or(u16::MAX)),
                &body,
                area.width,
            );
        }
    }
}

impl Widget for Composer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        if area.height < COMPOSER_FRAME_MIN_HEIGHT {
            return;
        }

        let queued_height = self
            .queued_preview_height()
            .min(area.height.saturating_sub(COMPOSER_FRAME_MIN_HEIGHT));
        let queued_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: queued_height,
        };
        let input_area = Rect {
            x: area.x,
            y: area.y.saturating_add(queued_height),
            width: area.width,
            height: area.height.saturating_sub(queued_height),
        };
        self.render_queued_preview(queued_area, buf);

        let separator = "─".repeat(area.width as usize);
        buf.set_string(
            input_area.x,
            input_area.y,
            separator.clone(),
            role_style(TranscriptColorRole::Muted),
        );
        buf.set_string(
            input_area.x,
            input_area.bottom().saturating_sub(1),
            separator,
            role_style(TranscriptColorRole::Muted),
        );

        let body = Rect {
            x: input_area.x,
            y: input_area.y.saturating_add(1),
            width: input_area.width,
            height: input_area.height.saturating_sub(2),
        };
        let command_height = if self.command_suggestions_open() {
            CommandPalette::required_height(self.command_suggestions.len())
        } else if !self.popup_items.is_empty() {
            ComposerPopupPalette::required_height(self.popup_items.len())
        } else {
            0
        };
        let draft_height = body.height.saturating_sub(command_height).max(1);
        let command_area = Rect {
            x: body.x,
            y: body.y,
            width: body.width,
            height: command_height,
        };
        let draft_area = Rect {
            x: body.x,
            y: body.y.saturating_add(command_height),
            width: body.width,
            height: draft_height,
        };

        if !self.popup_items.is_empty() && command_height > 0 {
            ComposerPopupPalette {
                title: self.suggestion_title,
                items: self.popup_items,
                selected: self.selected_command_suggestion,
            }
            .render(command_area, buf);
        } else if command_height > 0 {
            CommandPalette {
                title: self.suggestion_title,
                commands: self.command_suggestions,
                selected: self.selected_command_suggestion,
            }
            .render(command_area, buf);
        }

        let visible_lines = visible_draft_lines(
            self.draft,
            draft_height,
            draft_area.width,
            self.draft_scroll_offset,
        );
        for (index, line) in visible_lines.iter().enumerate() {
            let input_line = Self::build_input_line(line, self.placeholder, index == 0);
            buf.set_line(
                draft_area.x,
                draft_area.y + u16::try_from(index).unwrap_or(u16::MAX),
                &input_line,
                draft_area.width,
            );
        }
    }
}

fn visible_draft_lines(
    draft: ComposerDraft<'_>,
    height: u16,
    area_width: u16,
    scroll_offset: u16,
) -> Vec<ComposerDraftLine> {
    let max_lines = usize::from(height.max(1));
    let spans = visual_line_spans(draft.text, draft_content_width(area_width));
    let total = spans.len();
    if total <= max_lines {
        let start = 0;
        return spans[start..]
            .iter()
            .map(|span| ComposerDraftLine {
                segments: line_segments(draft.text, span.start..span.end, draft.elements),
            })
            .collect();
    }
    let max_offset = total.saturating_sub(max_lines);
    let start = (usize::from(scroll_offset)).min(max_offset);
    spans[start..start + max_lines]
        .iter()
        .map(|span| ComposerDraftLine {
            segments: line_segments(draft.text, span.start..span.end, draft.elements),
        })
        .collect()
}

fn draft_content_width(area_width: u16) -> u16 {
    area_width.saturating_sub(agent_tui_prompt_width()).max(1)
}

fn line_segments(
    text: &str,
    line_range: Range<usize>,
    elements: &[ComposerDraftElement],
) -> Vec<ComposerDraftSegment> {
    let mut segments = Vec::new();
    let mut cursor = line_range.start;
    for element in elements.iter().filter(|element| {
        element.range.start < line_range.end && element.range.end > line_range.start
    }) {
        let element_start = element.range.start.max(line_range.start);
        let element_end = element.range.end.min(line_range.end);
        if cursor < element_start {
            segments.push(ComposerDraftSegment::plain(slice_chars(
                text,
                cursor..element_start,
            )));
        }
        if element_start < element_end {
            segments.push(ComposerDraftSegment {
                text: slice_chars(text, element_start..element_end),
                kind: match element.kind {
                    ComposerDraftElementKind::Mention(sigil) => {
                        ComposerDraftSegmentKind::Mention(sigil)
                    }
                    ComposerDraftElementKind::File => ComposerDraftSegmentKind::File,
                    ComposerDraftElementKind::Image => ComposerDraftSegmentKind::Image,
                },
            });
        }
        cursor = element_end;
    }
    if cursor < line_range.end {
        segments.push(ComposerDraftSegment::plain(slice_chars(
            text,
            cursor..line_range.end,
        )));
    }
    segments
}

impl ComposerDraftSegment {
    fn plain(text: String) -> Self {
        Self {
            text,
            kind: ComposerDraftSegmentKind::Plain,
        }
    }
}

fn segment_style(kind: ComposerDraftSegmentKind) -> Style {
    match kind {
        ComposerDraftSegmentKind::Plain => role_style(TranscriptColorRole::Assistant),
        ComposerDraftSegmentKind::Mention('$') => role_style(TranscriptColorRole::Muted),
        ComposerDraftSegmentKind::Mention('@') => strong_role_style(TranscriptColorRole::Accent),
        ComposerDraftSegmentKind::Mention('#') => strong_role_style(TranscriptColorRole::Loading),
        ComposerDraftSegmentKind::Mention('%') => strong_role_style(TranscriptColorRole::Accent),
        ComposerDraftSegmentKind::Mention(_) => strong_role_style(TranscriptColorRole::Assistant),
        ComposerDraftSegmentKind::File => strong_role_style(TranscriptColorRole::Accent),
        ComposerDraftSegmentKind::Image => strong_role_style(TranscriptColorRole::Loading),
    }
}

fn slice_chars(text: &str, range: Range<usize>) -> String {
    text.chars()
        .skip(range.start)
        .take(range.end.saturating_sub(range.start))
        .collect()
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
