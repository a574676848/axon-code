//! MCP transcript cell 渲染。

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
    surface::{
        cell::RenderCellData,
        cell_event_data::{AgentRenderLifecycleStatus, AgentRenderMcpCell, AgentRenderMcpItem},
        render_policy::RenderPolicyDecision,
        visual_density::{
            DETAIL_CONTINUATION, DETAIL_PREFIX, NEUTRAL_PREFIX, STATUS_FAILED_PREFIX,
            STATUS_PENDING_PREFIX, STATUS_RUNNING_PREFIX,
        },
    },
    widgets::{
        terminal_hyperlinks::HyperlinkLine,
        transcript_cells::{
            CELL_DETAIL_LIMIT, display_width, normalize_and_truncate_cell_text,
            render_detail_block, render_wrapped_block,
        },
        transcript_markdown_wrap::wrap_line_with_prefix,
        transcript_style::{CellVisual, role_style, strong_role_style},
    },
};

const MCP_PREFIX: &str = NEUTRAL_PREFIX;
const MCP_PREFIX_CONTINUATION: &str = DETAIL_CONTINUATION;
const MCP_PREFIX_COMPLETED: &str = NEUTRAL_PREFIX;
const MCP_PREFIX_FAILED: &str = STATUS_FAILED_PREFIX;
const MCP_PREFIX_PENDING: &str = STATUS_PENDING_PREFIX;
const MCP_PREFIX_RUNNING: &str = STATUS_RUNNING_PREFIX;
const MCP_DETAIL_PREFIX: &str = DETAIL_PREFIX;
const MCP_DETAIL_CONTINUATION: &str = DETAIL_CONTINUATION;
const MCP_EXPANDED_LIMIT: usize = 6;
const MCP_SUMMARY_LIMIT: usize = 2;
const MCP_OUTPUT_LINE_LIMIT: usize = 160;
const MCP_OUTPUT_LINE_COUNT_LIMIT: usize = 24;
const MCP_ARGUMENT_SUMMARY_LIMIT: usize = 120;

pub(super) fn render_standalone(
    visual: CellVisual,
    decision: RenderPolicyDecision,
    selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let expanded = decision.render_expanded(selected);
    let limit = if expanded {
        MCP_EXPANDED_LIMIT
    } else {
        MCP_SUMMARY_LIMIT
    };
    render_mcp_items(visual, data, width, limit, expanded)
}

pub(super) fn render_mcp_items(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
    limit: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    if let Some(cell) = mcp_cell_data(data) {
        return render_typed_mcp_items(visual, cell, width, limit, expanded);
    }
    Vec::new()
}

fn mcp_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderMcpCell> {
    match data {
        Some(RenderCellData::Mcp(cell)) => Some(cell),
        _ => None,
    }
}

fn render_typed_mcp_items(
    visual: CellVisual,
    cell: &AgentRenderMcpCell,
    width: u16,
    limit: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if cell.is_catalog_inventory() {
        return render_catalog_inventory(visual, cell, width);
    }
    if cell.items.is_empty() {
        lines.extend(render_detail_block(
            visual,
            MCP_PREFIX,
            &cell.empty_text,
            width,
        ));
        return lines;
    }
    let original_count = cell.items.len();
    lines.extend(
        cell.items
            .iter()
            .take(limit)
            .enumerate()
            .flat_map(|(index, item)| {
                render_typed_mcp_item(visual, item, width, expanded, index == 0)
            }),
    );
    append_omitted(&mut lines, visual, MCP_PREFIX, original_count, limit, width);
    lines
}

fn render_catalog_inventory(
    visual: CellVisual,
    cell: &AgentRenderMcpCell,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "/mcp",
            role_style(TranscriptColorRole::Loading),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("MCP Tools", Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
    ];

    if cell.items.is_empty() {
        lines.extend(render_detail_block(
            visual,
            MCP_PREFIX,
            &cell.empty_text,
            width,
        ));
        return lines;
    }

    for group in catalog_inventory_groups(&cell.items) {
        let (group_prefix_style, group_text_style) = mcp_detail_styles(group.status, visual);
        lines.extend(render_wrapped_block(
            MCP_PREFIX,
            MCP_PREFIX_CONTINUATION,
            &group.server,
            group_prefix_style,
            group_text_style,
            width,
        ));
        match group.status {
            AgentRenderLifecycleStatus::Failed => {
                let failure_style = role_style(TranscriptColorRole::ToolFailed);
                lines.extend(render_wrapped_block(
                    "    • ",
                    "      ",
                    &format!(
                        "失败: {}",
                        group.summary.unwrap_or_else(|| "MCP 加载失败".to_owned())
                    ),
                    failure_style,
                    failure_style,
                    width,
                ));
            }
            AgentRenderLifecycleStatus::Running => {
                lines.extend(render_wrapped_block(
                    "    • ",
                    "      ",
                    "加载中",
                    visual.detail_prefix_style,
                    visual.detail_text_style,
                    width,
                ));
            }
            _ => {
                lines.extend(render_wrapped_block(
                    "    • ",
                    "      ",
                    &format!("Tools: {}", group.tools.join(", ")),
                    visual.detail_prefix_style,
                    visual.detail_text_style,
                    width,
                ));
            }
        }
        lines.push(Line::from(""));
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogInventoryGroup {
    server: String,
    status: AgentRenderLifecycleStatus,
    tools: Vec<String>,
    summary: Option<String>,
}

fn catalog_inventory_groups(items: &[AgentRenderMcpItem]) -> Vec<CatalogInventoryGroup> {
    let mut groups = std::collections::BTreeMap::<String, CatalogInventoryGroup>::new();
    for item in items {
        let group = groups
            .entry(item.server.clone())
            .or_insert_with(|| CatalogInventoryGroup {
                server: item.server.clone(),
                status: item.status,
                tools: Vec::new(),
                summary: item.summary.clone(),
            });
        if item.status == AgentRenderLifecycleStatus::Failed {
            group.status = AgentRenderLifecycleStatus::Failed;
            group.summary = item.summary.clone();
        } else if item.status == AgentRenderLifecycleStatus::Running
            && group.status != AgentRenderLifecycleStatus::Failed
        {
            group.status = AgentRenderLifecycleStatus::Running;
        }
        if let Some(tool) = item.tool.clone() {
            group.tools.push(tool);
        }
    }
    groups
        .into_values()
        .map(|mut group| {
            group.tools.sort();
            group
        })
        .collect()
}

fn typed_mcp_target_text(item: &AgentRenderMcpItem) -> String {
    item.display_target.clone().unwrap_or_else(|| {
        item.tool.as_ref().map_or_else(
            || item.server.clone(),
            |tool| format!("{}.{}", item.server, tool),
        )
    })
}

fn render_typed_mcp_item(
    visual: CellVisual,
    item: &AgentRenderMcpItem,
    width: u16,
    expanded: bool,
    show_toggle_hint: bool,
) -> Vec<Line<'static>> {
    let mut lines = render_typed_mcp_header(visual, item, width, expanded, show_toggle_hint);
    if expanded {
        if let Some(arguments) = item
            .arguments_preview
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            lines.extend(render_wrapped_block(
                MCP_DETAIL_PREFIX,
                MCP_DETAIL_CONTINUATION,
                &format!("参数 {arguments}"),
                visual.detail_prefix_style,
                visual.detail_text_style,
                width,
            ));
        }
        lines.extend(render_mcp_output_detail(visual, item, width));
    } else {
        if let Some(arguments) = typed_mcp_arguments_summary(item) {
            lines.extend(render_wrapped_block(
                MCP_DETAIL_PREFIX,
                MCP_DETAIL_CONTINUATION,
                &format!("参数 {arguments}"),
                visual.detail_prefix_style,
                visual.detail_text_style,
                width,
            ));
        }
        if let Some(detail) = typed_mcp_detail_text(item) {
            let (prefix_style, detail_style) = mcp_detail_styles(item.status, visual);
            lines.extend(render_wrapped_block(
                MCP_DETAIL_PREFIX,
                MCP_DETAIL_CONTINUATION,
                &normalize_and_truncate_cell_text(&detail, CELL_DETAIL_LIMIT),
                prefix_style,
                detail_style,
                width,
            ));
        }
    }
    lines
}

fn render_typed_mcp_header(
    visual: CellVisual,
    item: &AgentRenderMcpItem,
    width: u16,
    expanded: bool,
    show_toggle_hint: bool,
) -> Vec<Line<'static>> {
    let target = normalize_and_truncate_cell_text(&typed_mcp_target_text(item), CELL_DETAIL_LIMIT);
    let body_style = visual.detail_text_style;
    let failed = mcp_status_is_failure(item.status);
    let failure_style = strong_role_style(TranscriptColorRole::ToolFailed);
    let label_style = if failed { failure_style } else { body_style };
    let target_style = if failed {
        failure_style
    } else {
        role_style(TranscriptColorRole::McpIdentifier)
    };
    let mut spans = vec![Span::styled(item.display_label.clone(), label_style)];
    if !target.is_empty() {
        spans.push(Span::styled(" · ", body_style));
        spans.push(Span::styled(target, target_style));
    }
    if show_toggle_hint {
        let hint = if expanded {
            " · 点击收起"
        } else {
            " · 点击查看详情"
        };
        spans.push(Span::styled(hint, body_style));
    }

    let prefix = mcp_status_prefix(item.status);
    let body_width = usize::from(width)
        .saturating_sub(display_width(prefix))
        .max(1);
    wrap_line_with_prefix(
        HyperlinkLine::new(Line::from(spans)),
        prefix,
        MCP_PREFIX_CONTINUATION,
        mcp_status_prefix_style(item.status, visual),
        body_width,
    )
    .into_iter()
    .map(|line| line.line)
    .collect()
}

fn typed_mcp_arguments_summary(item: &AgentRenderMcpItem) -> Option<String> {
    let arguments = item
        .arguments_preview
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "{}")?;
    Some(normalize_and_truncate_cell_text(
        arguments,
        MCP_ARGUMENT_SUMMARY_LIMIT,
    ))
}

fn typed_mcp_detail_text(item: &AgentRenderMcpItem) -> Option<String> {
    item.output_preview
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            (item.status != AgentRenderLifecycleStatus::Completed)
                .then_some(item.summary.as_deref())
                .flatten()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .or_else(|| item.display_detail.clone())
}

fn render_mcp_output_detail(
    visual: CellVisual,
    item: &AgentRenderMcpItem,
    width: u16,
) -> Vec<Line<'static>> {
    let output = if mcp_status_is_failure(item.status) {
        typed_mcp_detail_text(item)
    } else {
        item.output_preview
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    let Some(output) = output else {
        return Vec::new();
    };
    let output_lines = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let mut lines = output_lines
        .iter()
        .take(MCP_OUTPUT_LINE_COUNT_LIMIT)
        .flat_map(|line| {
            let (prefix_style, detail_style) = mcp_detail_styles(item.status, visual);
            render_wrapped_block(
                MCP_DETAIL_PREFIX,
                MCP_DETAIL_CONTINUATION,
                &normalize_and_truncate_cell_text(line, MCP_OUTPUT_LINE_LIMIT),
                prefix_style,
                detail_style,
                width,
            )
        })
        .collect::<Vec<_>>();
    let omitted = output_lines
        .len()
        .saturating_sub(MCP_OUTPUT_LINE_COUNT_LIMIT);
    if omitted > 0 {
        lines.extend(render_wrapped_block(
            MCP_DETAIL_PREFIX,
            MCP_DETAIL_CONTINUATION,
            &format!("另 {omitted} 行结果"),
            visual.detail_prefix_style,
            visual.detail_text_style,
            width,
        ));
    }
    lines
}

fn mcp_detail_styles(status: AgentRenderLifecycleStatus, visual: CellVisual) -> (Style, Style) {
    if mcp_status_is_failure(status) {
        let style = role_style(TranscriptColorRole::ToolFailed);
        return (style, style);
    }
    (visual.detail_prefix_style, visual.detail_text_style)
}

fn mcp_status_is_failure(status: AgentRenderLifecycleStatus) -> bool {
    matches!(
        status,
        AgentRenderLifecycleStatus::Failed
            | AgentRenderLifecycleStatus::Blocked
            | AgentRenderLifecycleStatus::Stopped
    )
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

fn mcp_status_prefix(status: AgentRenderLifecycleStatus) -> &'static str {
    match status {
        AgentRenderLifecycleStatus::Completed => MCP_PREFIX_COMPLETED,
        AgentRenderLifecycleStatus::Failed
        | AgentRenderLifecycleStatus::Blocked
        | AgentRenderLifecycleStatus::Stopped => MCP_PREFIX_FAILED,
        AgentRenderLifecycleStatus::Running => MCP_PREFIX_RUNNING,
        AgentRenderLifecycleStatus::Pending | AgentRenderLifecycleStatus::Unknown => {
            MCP_PREFIX_PENDING
        }
    }
}

fn mcp_status_prefix_style(status: AgentRenderLifecycleStatus, visual: CellVisual) -> Style {
    match status {
        AgentRenderLifecycleStatus::Completed => {
            strong_role_style(TranscriptColorRole::ToolCompleted)
        }
        AgentRenderLifecycleStatus::Failed
        | AgentRenderLifecycleStatus::Blocked
        | AgentRenderLifecycleStatus::Stopped => strong_role_style(TranscriptColorRole::ToolFailed),
        AgentRenderLifecycleStatus::Running => strong_role_style(TranscriptColorRole::Loading),
        AgentRenderLifecycleStatus::Pending | AgentRenderLifecycleStatus::Unknown => visual
            .detail_prefix_style
            .fg(transcript_palette().color(TranscriptColorRole::Muted)),
    }
}

#[cfg(test)]
#[path = "transcript_mcp_tests.rs"]
mod tests;
