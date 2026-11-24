//! Status transcript cell 渲染。

use ratatui::{style::Style, text::Line};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::{
        AgentTuiLifecyclePhase, display_units::normalize_display_units_in_text,
        duration_format::format_duration_millis_compact,
    },
    surface::{
        cell::{
            RenderCellData, RenderStatusCell, RenderStatusItem, STATUS_KIND_INTERRUPT_REQUESTED,
        },
        visual_density::BULLET_PREFIX,
    },
    widgets::{
        transcript_cells::{
            CELL_DETAIL_LIMIT, normalize_and_truncate_cell_text, render_wrapped_block,
        },
        transcript_style::{CellVisual, role_style},
    },
};

const STATUS_PREFIX: &str = BULLET_PREFIX;
const STATUS_EXPANDED_LIMIT: usize = usize::MAX;
const MIN_VISIBLE_DURATION_MS: u64 = 1;

pub(super) fn render_status_items(
    visual: CellVisual,
    cell: &RenderStatusCell,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    render_status_item_slice(visual, &cell.items, width, limit)
}

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    render_status_body(visual, data, width, STATUS_EXPANDED_LIMIT)
}

fn render_status_body(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    match data {
        Some(RenderCellData::Status(cell)) => render_status_items(visual, cell, width, limit),
        _ => Vec::new(),
    }
}

fn render_status_item_slice(
    visual: CellVisual,
    items: &[RenderStatusItem],
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    let visible_items = items
        .iter()
        .filter(|item| !status_item_text(item).is_empty())
        .collect::<Vec<_>>();
    let original_count = visible_items.len();
    let mut lines = visible_items
        .into_iter()
        .take(limit)
        .flat_map(|item| render_typed_status_item(visual, item, width))
        .collect::<Vec<_>>();
    append_omitted(
        &mut lines,
        visual,
        STATUS_PREFIX,
        original_count,
        limit,
        width,
    );
    lines
}

fn render_typed_status_item(
    visual: CellVisual,
    item: &RenderStatusItem,
    width: u16,
) -> Vec<Line<'static>> {
    let text = normalize_and_truncate_cell_text(&status_item_text(item), CELL_DETAIL_LIMIT);
    if text.is_empty() {
        return Vec::new();
    }
    render_wrapped_block(
        status_prefix_for_item(item),
        &" ".repeat(status_prefix_for_item(item).len()),
        &text,
        status_prefix_style(item, visual),
        status_text_style(item, visual),
        width,
    )
}

fn status_prefix_for_item(item: &RenderStatusItem) -> &'static str {
    match item.phase {
        Some(AgentTuiLifecyclePhase::Command) => "  › ",
        _ => STATUS_PREFIX,
    }
}

fn status_prefix_style(item: &RenderStatusItem, visual: CellVisual) -> Style {
    if status_item_is_interrupt(item) {
        return role_style(TranscriptColorRole::Interrupted);
    }
    match item.phase {
        Some(AgentTuiLifecyclePhase::Generation) => role_style(TranscriptColorRole::Loading),
        Some(AgentTuiLifecyclePhase::Command) => role_style(TranscriptColorRole::DiffHunk),
        Some(AgentTuiLifecyclePhase::Notice) => role_style(TranscriptColorRole::Notice),
        _ => visual.detail_prefix_style,
    }
}

fn status_text_style(item: &RenderStatusItem, visual: CellVisual) -> Style {
    if status_item_is_interrupt(item) {
        role_style(TranscriptColorRole::Interrupted)
    } else {
        visual.detail_text_style
    }
}

fn status_item_is_interrupt(item: &RenderStatusItem) -> bool {
    item.kind == STATUS_KIND_INTERRUPT_REQUESTED
}

fn status_item_text(item: &RenderStatusItem) -> String {
    if status_item_is_transient_runtime_noise(item) {
        return String::new();
    }
    let mut parts = Vec::new();
    if let Some(label) = status_kind_label(&item.kind) {
        parts.push(label.to_owned());
    }
    if let Some(tool) = item.tool_name.as_deref().filter(|value| !value.is_empty()) {
        parts.push(tool.to_owned());
    }
    if let Some(duration) = item
        .duration_ms
        .filter(|duration| *duration >= MIN_VISIBLE_DURATION_MS)
    {
        parts.push(format_duration_millis_compact(duration));
    }
    let summary = status_summary_text(item);
    if !summary.is_empty() {
        parts.push(summary);
    }
    parts.join(" · ")
}

fn status_item_is_transient_runtime_noise(item: &RenderStatusItem) -> bool {
    matches!(
        item.kind.as_str(),
        "StreamingStarted" | "streaming_started" | "Generation" | "generation"
    )
}

fn status_summary_text(item: &RenderStatusItem) -> String {
    let summary = item.summary.trim();
    if summary.is_empty() {
        return String::new();
    }
    normalize_display_units_in_text(summary)
}

fn status_kind_label(kind: &str) -> Option<&'static str> {
    match kind {
        "HookStatus" | "hook_status" => Some("Hook"),
        "omitted" => Some("省略"),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use ratatui::style::Style;
    use ratatui::text::Line;

    use crate::{
        AgentTuiLifecyclePhase,
        app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
        surface::cell::{
            RenderCellData, RenderCellRecord, RenderStatusCell, RenderStatusItem,
            STATUS_KIND_INTERRUPT_REQUESTED,
        },
    };

    use super::{render_standalone, render_status_items};

    #[test]
    fn status_cell_without_typed_data_omits_generic_status_text() {
        let line = RenderCellRecord::status_with_data(RenderStatusCell {
            id: "test:status:empty".to_owned(),
            title: "状态".to_owned(),
            items: Vec::new(),
        });

        let rendered =
            crate::widgets::TranscriptRenderGateway::render_cell_hyperlink_lines(&line, false, 80);

        assert!(rendered.is_empty());
    }

    #[test]
    fn status_standalone_renders_items_without_outer_status_header() {
        let data = RenderCellData::Status(RenderStatusCell {
            id: "test:status:standalone".to_owned(),
            title: "状态".to_owned(),
            items: vec![status_item(
                "HookStatus",
                "权限检查",
                Some(AgentTuiLifecyclePhase::Notice),
                None,
                None,
                None,
            )],
        });

        let text = status_lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.contains("Hook · 权限检查"));
        assert!(!text.contains("状态 Hook"));
        assert!(!text.starts_with("状态"));
    }

    #[test]
    fn interrupt_status_uses_interrupted_color_for_prefix_and_text() {
        let data = RenderCellData::Status(RenderStatusCell {
            id: "test:status:interrupt".to_owned(),
            title: "中断".to_owned(),
            items: vec![status_item(
                STATUS_KIND_INTERRUPT_REQUESTED,
                "已停止，可继续输入",
                Some(AgentTuiLifecyclePhase::Notice),
                None,
                None,
                None,
            )],
        });

        let lines = render_standalone(test_visual(), false, Some(&data), 80);
        let expected = transcript_palette().color(TranscriptColorRole::Interrupted);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].style.fg, Some(expected));
        assert_eq!(lines[0].spans[1].style.fg, Some(expected));
    }

    #[test]
    fn status_items_format_large_duration_with_compact_units() {
        let cell = RenderStatusCell {
            id: "test:status:large-duration".to_owned(),
            title: "状态".to_owned(),
            items: vec![status_item(
                "HookStatus",
                "完成",
                Some(AgentTuiLifecyclePhase::Notice),
                Some("shell"),
                Some(65_000),
                None,
            )],
        };
        let text = status_lines_text(render_status_items(test_visual(), &cell, 80, 4));

        assert!(text.contains("Hook · shell · 1m 05s · 完成"));
        assert!(!text.contains("65000ms"));
    }

    #[test]
    fn status_items_omit_zero_duration_noise() {
        let cell = RenderStatusCell {
            id: "test:status:zero-duration".to_owned(),
            title: "状态".to_owned(),
            items: vec![status_item(
                "ToolExecutionStarted",
                "运行工具",
                Some(AgentTuiLifecyclePhase::Notice),
                Some("builtin.search"),
                Some(0),
                None,
            )],
        };
        let text = status_lines_text(render_status_items(test_visual(), &cell, 80, 4));

        assert!(text.contains("builtin.search · 运行工具"));
        assert!(!text.contains("0ms"));
    }

    #[test]
    fn status_summary_normalizes_raw_millisecond_units() {
        let cell = RenderStatusCell {
            id: "test:status:normalize-ms".to_owned(),
            title: "状态".to_owned(),
            items: vec![status_item(
                "HookStatus",
                "耗时 65000ms",
                Some(AgentTuiLifecyclePhase::Notice),
                None,
                None,
                None,
            )],
        };
        let text = status_lines_text(render_status_items(test_visual(), &cell, 80, 4));

        assert!(text.contains("Hook · 耗时 1m 05s"));
        assert!(!text.contains("65000ms"));
    }

    #[test]
    fn status_items_use_neutral_prefix_for_tool_lifecycle_events() {
        let cell = RenderStatusCell {
            id: "test:status:tool-lifecycle".to_owned(),
            title: "状态".to_owned(),
            items: vec![
                status_item(
                    "ToolExecutionStarted",
                    "运行 shell",
                    Some(AgentTuiLifecyclePhase::Notice),
                    Some("shell"),
                    Some(12),
                    Some("⠙"),
                ),
                status_item(
                    "ToolCallCompleted",
                    "完成",
                    Some(AgentTuiLifecyclePhase::Notice),
                    Some("shell"),
                    Some(18),
                    None,
                ),
            ],
        };

        let lines = render_status_items(test_visual(), &cell, 80, 8);
        let text = status_lines_text(lines.clone());

        assert_eq!(lines[0].spans[0].content.as_ref(), "  · ");
        assert_eq!(lines[1].spans[0].content.as_ref(), "  · ");
        assert!(text.contains("shell · 12ms · 运行 shell"));
        assert!(text.contains("shell · 18ms · 完成"));
        assert!(!text.contains("调用"));
        assert!(!text.contains("ToolExecutionStarted"));
        assert!(!text.contains("ToolCallCompleted"));
        assert!(!text.contains("⠙"));
    }

    #[test]
    fn status_items_omit_transient_streaming_and_generation_noise() {
        let cell = RenderStatusCell {
            id: "test:status:omit-generation".to_owned(),
            title: "运行".to_owned(),
            items: vec![
                status_item(
                    "StreamingStarted",
                    "local/model",
                    Some(AgentTuiLifecyclePhase::Notice),
                    None,
                    None,
                    Some("⠋"),
                ),
                status_item(
                    "Generation",
                    "生成回复",
                    Some(AgentTuiLifecyclePhase::Notice),
                    None,
                    None,
                    None,
                ),
            ],
        };
        let text = status_lines_text(render_status_items(test_visual(), &cell, 80, 4));

        assert!(text.is_empty());
        assert!(!text.contains("运行 · local/model"));
        assert!(!text.contains("生成 · 生成回复"));
        assert!(!text.contains("StreamingStarted"));
        assert!(!text.contains("Generation"));
        assert!(!text.contains("⠋"));
    }

    #[test]
    fn status_items_render_hook_label_and_duration() {
        let cell = RenderStatusCell {
            id: "test:status:hook-label".to_owned(),
            title: "状态".to_owned(),
            items: vec![
                status_item(
                    "HookStatus",
                    "权限检查",
                    Some(AgentTuiLifecyclePhase::Notice),
                    None,
                    None,
                    None,
                ),
                status_item(
                    "HookStatus",
                    "完成",
                    Some(AgentTuiLifecyclePhase::Notice),
                    Some("shell"),
                    Some(18),
                    None,
                ),
            ],
        };
        let text = status_lines_text(render_status_items(test_visual(), &cell, 80, 4));

        assert!(text.contains("Hook · 权限检查"));
        assert!(text.contains("Hook · shell · 18ms · 完成"));
        assert!(!text.contains("ToolCallCompleted"));
    }

    fn status_item(
        kind: &str,
        summary: &str,
        phase: Option<AgentTuiLifecyclePhase>,
        tool_name: Option<&str>,
        duration_ms: Option<u64>,
        indicator: Option<&str>,
    ) -> RenderStatusItem {
        RenderStatusItem {
            kind: kind.to_owned(),
            summary: summary.to_owned(),
            phase,
            indicator: indicator.map(str::to_owned),
            tool_name: tool_name.map(str::to_owned),
            duration_ms,
            turn_id: None,
            attempt_index: None,
        }
    }

    fn test_visual() -> crate::widgets::transcript_style::CellVisual {
        crate::widgets::transcript_style::CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    fn status_lines_text(lines: Vec<Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
