//! Reasoning transcript cell 渲染。

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    r#loop::agent_tui::duration_format::format_duration_millis_compact,
    surface::{
        cell::{RenderCellData, RenderReasoningCell},
        visual_density::STATUS_RUNNING_PREFIX,
    },
    widgets::{
        terminal_hyperlinks::{prefix_hyperlink_lines, visible_lines},
        transcript_cells::display_width,
        transcript_markdown::render_markdown_hyperlink_lines,
        transcript_style::CellVisual,
    },
};

const REASONING_PREFIX: &str = STATUS_RUNNING_PREFIX;
const REASONING_TITLE_RUNNING: &str = "思考中 / Thinking";
const REASONING_TITLE_DONE: &str = "思考 / Thinking";
const REASONING_BODY_PREFIX: &str = "  └ ";
const REASONING_BODY_CONTINUATION_PREFIX: &str = "    ";

pub(super) fn render_standalone(
    _visual: CellVisual,
    selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(RenderCellData::Reasoning(cell)) = data else {
        return Vec::new();
    };
    let text = reasoning_text(cell);
    let mut lines = vec![reasoning_title_line(cell)];
    if (!cell.done || selected) && !text.trim().is_empty() {
        lines.extend(render_reasoning_markdown(text, width));
    }
    lines
}

fn reasoning_title_line(cell: &RenderReasoningCell) -> Line<'static> {
    let mut title = if cell.done {
        REASONING_TITLE_DONE.to_owned()
    } else {
        REASONING_TITLE_RUNNING.to_owned()
    };
    if let Some(elapsed_ms) = cell.elapsed_ms {
        title.push_str(" · ");
        title.push_str(&format_duration_millis_compact(elapsed_ms));
    }
    Line::from(vec![
        Span::styled(
            REASONING_PREFIX.to_owned(),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            title,
            Style::default().add_modifier(Modifier::DIM | Modifier::BOLD),
        ),
    ])
}

fn reasoning_text(cell: &RenderReasoningCell) -> &str {
    cell.text.as_str()
}

fn render_reasoning_markdown(text: &str, width: u16) -> Vec<Line<'static>> {
    let body_prefix_width = u16::try_from(display_width(REASONING_BODY_PREFIX)).unwrap_or(u16::MAX);
    let body_width = width.saturating_sub(body_prefix_width).max(1);
    let markdown_lines = render_markdown_hyperlink_lines(
        text,
        Style::default().add_modifier(Modifier::DIM),
        body_width,
        None,
    );
    visible_lines(&prefix_hyperlink_lines(
        markdown_lines,
        Span::styled(
            REASONING_BODY_PREFIX,
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(
            REASONING_BODY_CONTINUATION_PREFIX,
            Style::default().add_modifier(Modifier::DIM),
        ),
    ))
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Modifier, Style};

    use super::render_standalone;
    use crate::{
        surface::cell::{RenderCellData, RenderReasoningCell},
        widgets::transcript_style::CellVisual,
    };

    fn visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    #[test]
    fn reasoning_cell_renders_stable_thinking_title_before_body() {
        let data = RenderCellData::Reasoning(RenderReasoningCell {
            id: "reasoning:1".to_owned(),
            text: "先分析调用链".to_owned(),
            done: false,
            elapsed_ms: None,
        });

        let lines = render_standalone(visual(), false, Some(&data), 80);

        assert_eq!(lines[0].to_string(), "  ◌ 思考中 / Thinking");
        assert!(
            lines
                .iter()
                .any(|line| line.to_string().contains("  └ 先分析调用链"))
        );
    }

    #[test]
    fn completed_reasoning_cell_keeps_title_without_running_state() {
        let data = RenderCellData::Reasoning(RenderReasoningCell {
            id: "reasoning:1".to_owned(),
            text: String::new(),
            done: true,
            elapsed_ms: None,
        });

        let lines = render_standalone(visual(), false, Some(&data), 80);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "  ◌ 思考 / Thinking");
    }

    #[test]
    fn completed_reasoning_cell_collapses_with_elapsed_and_expands_when_selected() {
        let data = RenderCellData::Reasoning(RenderReasoningCell {
            id: "reasoning:1".to_owned(),
            text: "完成后的思考正文".to_owned(),
            done: true,
            elapsed_ms: Some(1_250),
        });

        let collapsed = render_standalone(visual(), false, Some(&data), 80);
        let expanded = render_standalone(visual(), true, Some(&data), 80);

        assert_eq!(collapsed.len(), 1);
        assert_eq!(collapsed[0].to_string(), "  ◌ 思考 / Thinking · 1.25s");
        assert!(
            expanded
                .iter()
                .any(|line| line.to_string().contains("完成后的思考正文"))
        );
        assert!(
            expanded[0]
                .spans
                .iter()
                .all(|span| !span.style.add_modifier.contains(Modifier::REVERSED))
        );
    }

    #[test]
    fn reasoning_text_is_not_truncated_by_cell_detail_limit() {
        let source = "实现当前任务需要经过完整审计。".repeat(12);
        let data = RenderCellData::Reasoning(RenderReasoningCell {
            id: "reasoning:long".to_owned(),
            text: source.clone(),
            done: false,
            elapsed_ms: None,
        });

        let lines = render_standalone(visual(), false, Some(&data), 2_000);
        let rendered = lines[1].to_string();

        assert!(rendered.contains(&source));
        assert!(!rendered.contains("..."));
    }

    #[test]
    fn reasoning_markdown_uses_transcript_renderer() {
        let data = RenderCellData::Reasoning(RenderReasoningCell {
            id: "reasoning:markdown".to_owned(),
            text: "## 结论\n\n- **完整**展示 Thinking".to_owned(),
            done: false,
            elapsed_ms: None,
        });

        let lines = render_standalone(visual(), false, Some(&data), 80);
        let rendered = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("结论"));
        assert!(rendered.contains("完整"));
        assert!(!rendered.contains("##"));
        assert!(!rendered.contains("**"));
    }
}
