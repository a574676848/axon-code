//! Hook transcript cell 渲染。

use ratatui::{style::Style, text::Line};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::display_units::normalize_display_units_in_text,
    surface::{
        cell::RenderCellData,
        cell_event_data::{
            AgentRenderHookCell, AgentRenderHookItem, AgentRenderHookOutputEntry,
            AgentRenderHookOutputKind, AgentRenderLifecycleStatus,
        },
        visual_density::{
            DETAIL_CONTINUATION, NEUTRAL_PREFIX, STATUS_FAILED_PREFIX, STATUS_PENDING_PREFIX,
            STATUS_RUNNING_PREFIX,
        },
    },
    widgets::{
        transcript_cells::{
            CELL_DETAIL_LIMIT, normalize_and_truncate_cell_text, render_wrapped_block,
        },
        transcript_style::{CellVisual, role_style, strong_role_style},
    },
};

const HOOK_PREFIX: &str = STATUS_RUNNING_PREFIX;
const HOOK_CONTINUATION_PREFIX: &str = DETAIL_CONTINUATION;
const HOOK_EXPANDED_LIMIT: usize = usize::MAX;

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    render_hook_items(visual, data, width, HOOK_EXPANDED_LIMIT)
}

pub(super) fn render_hook_items(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    if let Some(cell) = hook_cell_data(data) {
        return render_typed_hook_items(visual, cell, width, limit);
    }
    Vec::new()
}

fn hook_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderHookCell> {
    match data {
        Some(RenderCellData::Hook(cell)) => Some(cell),
        _ => None,
    }
}

fn render_typed_hook_items(
    visual: CellVisual,
    cell: &AgentRenderHookCell,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    let visible_items = cell
        .items
        .iter()
        .filter(|item| !hook_item_is_quiet_success(item))
        .collect::<Vec<_>>();
    let original_count = visible_hook_group_count(&visible_items);
    let mut lines = Vec::new();
    let mut rendered = 0usize;
    let mut index = 0usize;
    while index < visible_items.len() && rendered < limit {
        let item = visible_items[index];
        if let Some((group, next_index)) = running_hook_group(&visible_items, index) {
            lines.extend(render_running_hook_group(visual, group, width));
            rendered += 1;
            index = next_index;
            continue;
        }
        lines.extend(render_typed_hook_item(visual, item, width));
        rendered += 1;
        index += 1;
    }
    append_omitted(
        &mut lines,
        visual,
        HOOK_PREFIX,
        original_count,
        limit,
        width,
    );
    lines
}

struct RunningHookGroup<'a> {
    item: &'a AgentRenderHookItem,
    count: usize,
}

fn visible_hook_group_count(items: &[&AgentRenderHookItem]) -> usize {
    let mut count = 0usize;
    let mut index = 0usize;
    while index < items.len() {
        index = running_hook_group(items, index).map_or(index + 1, |(_, next)| next);
        count += 1;
    }
    count
}

fn running_hook_group<'a>(
    items: &[&'a AgentRenderHookItem],
    start: usize,
) -> Option<(RunningHookGroup<'a>, usize)> {
    let first = *items.get(start)?;
    if first.status != AgentRenderLifecycleStatus::Running {
        return None;
    }
    let mut next = start + 1;
    while let Some(candidate) = items.get(next).copied() {
        if candidate.status != AgentRenderLifecycleStatus::Running
            || candidate.name != first.name
            || candidate.status_message.as_deref() != first.status_message.as_deref()
        {
            break;
        }
        next += 1;
    }
    Some((
        RunningHookGroup {
            item: first,
            count: next - start,
        },
        next,
    ))
}

fn render_running_hook_group(
    visual: CellVisual,
    group: RunningHookGroup<'_>,
    width: u16,
) -> Vec<Line<'static>> {
    render_wrapped_block(
        &running_hook_group_prefix(&group),
        HOOK_CONTINUATION_PREFIX,
        &normalize_and_truncate_cell_text(&running_hook_group_text(group), CELL_DETAIL_LIMIT),
        lifecycle_style_for_status(AgentRenderLifecycleStatus::Running, visual),
        visual.detail_text_style,
        width,
    )
}

fn running_hook_group_prefix(group: &RunningHookGroup<'_>) -> String {
    group
        .item
        .indicator
        .as_deref()
        .filter(|indicator| !indicator.trim().is_empty())
        .map_or_else(
            || HOOK_PREFIX.to_owned(),
            |indicator| format!("  {indicator} "),
        )
}

fn running_hook_group_text(group: RunningHookGroup<'_>) -> String {
    let mut text = format!("{} hook", group.item.name);
    if group.count > 1 {
        text.push_str(&format!(" x{}", group.count));
    }
    if let Some(detail) = group
        .item
        .status_message
        .as_deref()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
    {
        text.push_str(" · ");
        text.push_str(&normalize_display_units_in_text(detail));
    }
    text
}

fn render_typed_hook_item(
    visual: CellVisual,
    item: &AgentRenderHookItem,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text = hook_item_text(item);
    if !text.is_empty() {
        lines.extend(render_wrapped_block(
            lifecycle_prefix_for_status(item.status, HOOK_PREFIX),
            HOOK_CONTINUATION_PREFIX,
            &normalize_and_truncate_cell_text(&text, CELL_DETAIL_LIMIT),
            lifecycle_style_for_status(item.status, visual),
            visual.detail_text_style,
            width,
        ));
    }
    for entry in &item.entries {
        lines.extend(render_hook_output_entry(visual, entry, width));
    }
    lines
}

fn render_hook_output_entry(
    visual: CellVisual,
    entry: &AgentRenderHookOutputEntry,
    width: u16,
) -> Vec<Line<'static>> {
    let text = normalize_and_truncate_cell_text(
        &normalize_display_units_in_text(&entry.text),
        CELL_DETAIL_LIMIT,
    );
    if text.is_empty() {
        return Vec::new();
    }
    render_wrapped_block(
        hook_output_prefix(entry.kind),
        HOOK_CONTINUATION_PREFIX,
        &text,
        visual.detail_prefix_style,
        hook_output_style(entry.kind, visual),
        width,
    )
}

fn hook_item_text(item: &AgentRenderHookItem) -> String {
    let mut parts = vec![item.name.clone()];
    if should_show_hook_status_text(item.status) {
        parts.push(lifecycle_status_text(item.status).to_owned());
    }
    let detail = item
        .status_message
        .as_deref()
        .map(str::trim)
        .filter(|detail| !detail.is_empty());
    if let Some(detail) = detail.filter(|detail| !hook_detail_duplicates_context(&parts, detail)) {
        parts.push(normalize_display_units_in_text(detail));
    }
    parts.join(" · ")
}

fn hook_detail_duplicates_context(parts: &[String], detail: &str) -> bool {
    parts.iter().any(|part| part == detail)
        || matches!(detail, "完成" | "completed" | "运行中" | "running")
}

fn should_show_hook_status_text(status: AgentRenderLifecycleStatus) -> bool {
    !matches!(
        status,
        AgentRenderLifecycleStatus::Running | AgentRenderLifecycleStatus::Completed
    )
}

fn lifecycle_status_text(status: AgentRenderLifecycleStatus) -> &'static str {
    match status {
        AgentRenderLifecycleStatus::Completed => "完成",
        AgentRenderLifecycleStatus::Failed => "失败",
        AgentRenderLifecycleStatus::Blocked => "阻断",
        AgentRenderLifecycleStatus::Stopped => "停止",
        AgentRenderLifecycleStatus::Pending => "等待",
        AgentRenderLifecycleStatus::Running => "运行中",
        AgentRenderLifecycleStatus::Unknown => "未知",
    }
}

fn lifecycle_prefix_for_status(
    status: AgentRenderLifecycleStatus,
    running_prefix: &'static str,
) -> &'static str {
    match status {
        AgentRenderLifecycleStatus::Completed => NEUTRAL_PREFIX,
        AgentRenderLifecycleStatus::Failed
        | AgentRenderLifecycleStatus::Blocked
        | AgentRenderLifecycleStatus::Stopped => STATUS_FAILED_PREFIX,
        AgentRenderLifecycleStatus::Pending | AgentRenderLifecycleStatus::Unknown => {
            STATUS_PENDING_PREFIX
        }
        AgentRenderLifecycleStatus::Running => running_prefix,
    }
}

fn lifecycle_style_for_status(status: AgentRenderLifecycleStatus, visual: CellVisual) -> Style {
    match status {
        AgentRenderLifecycleStatus::Completed => {
            strong_role_style(TranscriptColorRole::ToolCompleted)
        }
        AgentRenderLifecycleStatus::Failed
        | AgentRenderLifecycleStatus::Blocked
        | AgentRenderLifecycleStatus::Stopped => strong_role_style(TranscriptColorRole::ToolFailed),
        AgentRenderLifecycleStatus::Pending | AgentRenderLifecycleStatus::Unknown => {
            role_style(TranscriptColorRole::Muted)
        }
        AgentRenderLifecycleStatus::Running => visual.detail_prefix_style,
    }
}

fn hook_output_prefix(kind: AgentRenderHookOutputKind) -> &'static str {
    match kind {
        AgentRenderHookOutputKind::Feedback => DETAIL_CONTINUATION,
        AgentRenderHookOutputKind::Warning => "  ! ",
        AgentRenderHookOutputKind::Stop => "  ■ ",
        AgentRenderHookOutputKind::Context => "  › ",
        AgentRenderHookOutputKind::Error => STATUS_FAILED_PREFIX,
    }
}

fn hook_output_style(kind: AgentRenderHookOutputKind, visual: CellVisual) -> Style {
    match kind {
        AgentRenderHookOutputKind::Warning => role_style(TranscriptColorRole::Warning),
        AgentRenderHookOutputKind::Stop | AgentRenderHookOutputKind::Error => {
            role_style(TranscriptColorRole::ToolFailed)
        }
        AgentRenderHookOutputKind::Context => role_style(TranscriptColorRole::Notice),
        AgentRenderHookOutputKind::Feedback => visual.detail_text_style,
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

fn hook_item_is_quiet_success(item: &AgentRenderHookItem) -> bool {
    item.status == AgentRenderLifecycleStatus::Completed
        && item.entries.is_empty()
        && item
            .status_message
            .as_deref()
            .is_none_or(|detail| hook_detail_is_quiet_success(detail.trim()))
}

fn hook_detail_is_quiet_success(detail: &str) -> bool {
    detail.is_empty() || matches!(detail, "完成" | "completed")
}
