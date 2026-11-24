//! History transcript cell 渲染。

use ratatui::{style::Style, text::Line};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::display_units::normalize_display_units_in_text,
    surface::{
        cell::RenderCellData,
        cell_event_data::{AgentRenderHistoryCell, AgentRenderHistoryItem},
        visual_density::{
            NEUTRAL_CONTINUATION, NEUTRAL_PREFIX, STATUS_FAILED_PREFIX, STATUS_PENDING_PREFIX,
            STATUS_RUNNING_PREFIX,
        },
    },
    widgets::{
        transcript_cells::{
            CELL_DETAIL_LIMIT, normalize_and_truncate_cell_text, render_detail_block,
            render_wrapped_block,
        },
        transcript_style::{CellVisual, role_style, strong_role_style},
    },
};

const HISTORY_PREFIX: &str = NEUTRAL_PREFIX;
const HISTORY_PREFIX_CONTINUATION: &str = NEUTRAL_CONTINUATION;
const HISTORY_PREFIX_COMPLETED: &str = NEUTRAL_PREFIX;
const HISTORY_PREFIX_FAILED: &str = STATUS_FAILED_PREFIX;
const HISTORY_PREFIX_PENDING: &str = STATUS_PENDING_PREFIX;
const HISTORY_PREFIX_RUNNING: &str = STATUS_RUNNING_PREFIX;
const HISTORY_EXPANDED_LIMIT: usize = usize::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HistoryItemState {
    Completed,
    Failed,
    Pending,
    Running,
    Neutral,
}

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    render_history_items(visual, data, width, HISTORY_EXPANDED_LIMIT)
}

pub(super) fn render_history_items(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    if let Some(cell) = history_cell_data(data) {
        return render_typed_history_items(visual, cell, width, limit);
    }
    Vec::new()
}

fn history_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderHistoryCell> {
    match data {
        Some(RenderCellData::History(cell)) => Some(cell),
        _ => None,
    }
}

fn render_typed_history_items(
    visual: CellVisual,
    cell: &AgentRenderHistoryCell,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    if cell.items.is_empty() {
        return render_detail_block(visual, HISTORY_PREFIX, &cell.empty_text, width);
    }
    let original_count = cell.items.len();
    let mut lines = cell
        .items
        .iter()
        .take(limit)
        .flat_map(|item| render_typed_history_item(visual, item, width))
        .collect::<Vec<_>>();
    append_omitted(
        &mut lines,
        visual,
        HISTORY_PREFIX,
        original_count,
        limit,
        width,
    );
    lines
}

fn render_typed_history_item(
    visual: CellVisual,
    item: &AgentRenderHistoryItem,
    width: u16,
) -> Vec<Line<'static>> {
    let state = history_item_state(item.status_label.as_deref());
    let text = typed_history_item_text(item, state);
    let text = if is_compaction_history_item(item) {
        text
    } else {
        normalize_and_truncate_cell_text(&text, CELL_DETAIL_LIMIT)
    };
    render_wrapped_block(
        history_status_prefix(state),
        HISTORY_PREFIX_CONTINUATION,
        &text,
        history_status_prefix_style(state, visual),
        visual.detail_text_style,
        width,
    )
}

fn is_compaction_history_item(item: &AgentRenderHistoryItem) -> bool {
    item.kind_label.is_none() && item.name.to_ascii_lowercase().contains("compaction")
        || item.kind_label.is_none() && item.name.contains("压缩")
}

fn append_omitted(
    lines: &mut Vec<Line<'static>>,
    visual: CellVisual,
    prefix: &str,
    original_count: usize,
    limit: usize,
    width: u16,
) {
    let omitted = original_count.saturating_sub(limit);
    if omitted == 0 {
        return;
    }
    lines.extend(render_wrapped_block(
        prefix,
        &" ".repeat(prefix.len()),
        &format!("另 {omitted} 项"),
        visual.detail_prefix_style,
        visual.detail_text_style,
        width,
    ));
}

fn typed_history_item_text(item: &AgentRenderHistoryItem, state: HistoryItemState) -> String {
    let mut parts = vec![item.name.clone()];
    if let Some(kind) = item.kind_label.as_ref() {
        parts.push(match kind.as_str() {
            "Skill" | "skill" => "技能".to_owned(),
            _ => kind.clone(),
        });
    }
    if should_render_history_status(state)
        && let Some(status) = item.status_label.as_ref()
    {
        parts.push(status.clone());
    }
    normalize_display_units_in_text(&parts.join(" · "))
}

fn should_render_history_status(state: HistoryItemState) -> bool {
    matches!(state, HistoryItemState::Failed | HistoryItemState::Pending)
}

fn history_item_state(status: Option<&str>) -> HistoryItemState {
    match status.map(str::trim).filter(|value| !value.is_empty()) {
        Some("完成" | "已完成" | "成功" | "loaded" | "completed" | "success") => {
            HistoryItemState::Completed
        }
        Some("失败" | "错误" | "阻断" | "failed" | "error" | "blocked") => {
            HistoryItemState::Failed
        }
        Some("等待" | "待处理" | "pending") => HistoryItemState::Pending,
        Some("进行中" | "运行中" | "调用中" | "running" | "active" | "calling") => {
            HistoryItemState::Running
        }
        _ => HistoryItemState::Neutral,
    }
}

fn history_status_prefix(state: HistoryItemState) -> &'static str {
    match state {
        HistoryItemState::Completed => HISTORY_PREFIX_COMPLETED,
        HistoryItemState::Failed => HISTORY_PREFIX_FAILED,
        HistoryItemState::Pending => HISTORY_PREFIX_PENDING,
        HistoryItemState::Running => HISTORY_PREFIX_RUNNING,
        HistoryItemState::Neutral => HISTORY_PREFIX,
    }
}

fn history_status_prefix_style(state: HistoryItemState, visual: CellVisual) -> Style {
    match state {
        HistoryItemState::Completed => strong_role_style(TranscriptColorRole::ToolCompleted),
        HistoryItemState::Failed => strong_role_style(TranscriptColorRole::ToolFailed),
        HistoryItemState::Pending => role_style(TranscriptColorRole::Muted),
        HistoryItemState::Running => strong_role_style(TranscriptColorRole::Loading),
        HistoryItemState::Neutral => visual.detail_prefix_style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_shell::style_tokens::transcript_palette;
    use ratatui::style::Modifier;

    #[test]
    fn history_items_compact_kind_and_status_noise() {
        let visual = test_visual();
        let cell = RenderCellData::History(AgentRenderHistoryCell {
            id: "history:test:compact".to_owned(),
            title: "Skill".to_owned(),
            empty_text: "无条目".to_owned(),
            items: vec![
                AgentRenderHistoryItem {
                    name: "review-pr".to_owned(),
                    kind_label: Some("Skill".to_owned()),
                    status_label: None,
                },
                AgentRenderHistoryItem {
                    name: "run-42".to_owned(),
                    kind_label: Some("会话".to_owned()),
                    status_label: Some("进行中".to_owned()),
                },
            ],
        });
        let lines = render_history_items(visual, Some(&cell), 80, 8);
        let text = lines_text(lines);

        assert!(text.contains("review-pr · 技能"));
        assert!(!text.contains("loaded"));
        assert!(text.contains("run-42 · 会话"));
        assert!(!text.contains("run-42 · 会话 · 进行中"));
    }

    #[test]
    fn history_items_normalize_display_units_in_structured_text() {
        let visual = test_visual();
        let cell = RenderCellData::History(AgentRenderHistoryCell {
            id: "history:test:units".to_owned(),
            title: "会话".to_owned(),
            empty_text: "无条目".to_owned(),
            items: vec![AgentRenderHistoryItem {
                name: "cargo test · duration_ms=65000".to_owned(),
                kind_label: Some("tokens 1500 K".to_owned()),
                status_label: Some("失败".to_owned()),
            }],
        });
        let text = lines_text(render_history_items(visual, Some(&cell), 80, 8));

        assert!(text.contains("duration_ms=1m 05s"));
        assert!(text.contains("tokens 1.5M"));
        assert!(!text.contains("duration_ms=65000"));
    }

    #[test]
    fn history_items_use_status_prefix_colors() {
        let visual = test_visual();
        let cell = RenderCellData::History(AgentRenderHistoryCell {
            id: "history:test:status-colors".to_owned(),
            title: "会话".to_owned(),
            empty_text: "无条目".to_owned(),
            items: vec![
                AgentRenderHistoryItem {
                    name: "loaded-session".to_owned(),
                    kind_label: Some("会话".to_owned()),
                    status_label: Some("完成".to_owned()),
                },
                AgentRenderHistoryItem {
                    name: "active-session".to_owned(),
                    kind_label: Some("会话".to_owned()),
                    status_label: Some("进行中".to_owned()),
                },
                AgentRenderHistoryItem {
                    name: "failed-session".to_owned(),
                    kind_label: Some("会话".to_owned()),
                    status_label: Some("失败".to_owned()),
                },
            ],
        });
        let lines = render_history_items(visual, Some(&cell), 80, 8);

        assert_eq!(lines[0].spans[0].content.as_ref(), HISTORY_PREFIX_COMPLETED);
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(transcript_palette().color(TranscriptColorRole::ToolCompleted))
        );
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(lines[1].spans[0].content.as_ref(), HISTORY_PREFIX_RUNNING);
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(transcript_palette().color(TranscriptColorRole::Loading))
        );
        assert_eq!(lines[2].spans[0].content.as_ref(), HISTORY_PREFIX_FAILED);
        assert_eq!(
            lines[2].spans[0].style.fg,
            Some(transcript_palette().color(TranscriptColorRole::ToolFailed))
        );
    }

    #[test]
    fn standalone_history_renders_items_without_outer_title_header() {
        let visual = test_visual();
        let cell = RenderCellData::History(AgentRenderHistoryCell {
            id: "history:test:standalone".to_owned(),
            title: "会话".to_owned(),
            empty_text: "无条目".to_owned(),
            items: vec![AgentRenderHistoryItem {
                name: "run-42".to_owned(),
                kind_label: Some("会话".to_owned()),
                status_label: Some("进行中".to_owned()),
            }],
        });
        let text = lines_text(render_standalone(visual, false, Some(&cell), 80));

        assert!(text.contains("◌ run-42 · 会话"));
        assert!(!text.contains("• 会话\n"));
        assert!(!text.contains("历史 会话"));
    }

    fn test_visual() -> CellVisual {
        CellVisual {
            title_style: ratatui::style::Style::default(),
            action_text_style: ratatui::style::Style::default(),
            detail_prefix_style: ratatui::style::Style::default(),
            detail_text_style: ratatui::style::Style::default(),
        }
    }

    fn lines_text(lines: Vec<Line<'static>>) -> String {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
