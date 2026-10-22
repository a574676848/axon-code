//! ComposerDock 的统一渲染网关。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use super::{
    BottomPaneView, Composer, ComposerDraft, ComposerDraftElement, ComposerDraftElementKind,
};
use crate::{
    app_shell::{
        composer_dock::{
            COMPOSER_DOCK_MIN_INPUT_HEIGHT, COMPOSER_DOCK_SEPARATOR_HEIGHT,
            COMPOSER_DOCK_WORKING_STATUS_HEIGHT, COMPOSER_DOCK_WORKING_STATUS_TOP_PADDING,
            ComposerDockDraftElementKind, ComposerDockPanel, ComposerDockViewModel, Dock,
            DockLifecycle, WorkingStatusTone, WorkingStatusViewModel, registered_dock_kinds,
        },
        style_tokens::{TranscriptColorRole, transcript_palette},
    },
    surface::{
        animation::spinner_frame,
        display_width::DisplayWidthText,
        subagent_activity::{
            SubagentActivityRow, SubagentActivitySnapshot, SubagentActivityStatus,
        },
    },
    widgets::{shimmer::shimmer_spans, transcript_style::role_style},
};

const PLACEHOLDER: &str = "描述要完成的任务，或输入 / 选择能力";
const SUBAGENT_PREFIX: &str = "子代理";
const SUBAGENT_ACTIVITY_SEPARATOR_LABEL: &str = " 子代理活动 ";
const SUBAGENT_ACTIVITY_HORIZONTAL_PADDING: u16 = 2;
const ACTIVITY_DIVIDER: char = '─';
const WAITING_MARKER: &str = "◌";
const COMPLETED_MARKER: &str = "√";
const INTERRUPTED_MARKER: &str = "·";
const FAILED_MARKER: &str = "×";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockRenderPartition {
    panel: Rect,
    working_status: Rect,
    input: Rect,
    subagent: Rect,
}

impl DockRenderPartition {
    pub(crate) const fn input(self) -> Rect {
        self.input
    }
}

pub(crate) struct DockRenderGateway;

impl DockRenderGateway {
    pub(crate) fn render(view_model: ComposerDockViewModel<'_>, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let partition = Self::partition(&view_model, area);
        render_panel(view_model.panel.as_ref(), partition.panel, buf);
        render_input(&view_model, partition.input, buf);
        render_working_status(
            view_model.working_status.as_ref(),
            partition.working_status,
            buf,
        );
        render_subagent_activity(&view_model.subagent_activity, partition.subagent, buf);
    }

    pub(crate) fn partition(
        view_model: &ComposerDockViewModel<'_>,
        area: Rect,
    ) -> DockRenderPartition {
        let panel_height = panel_height(view_model, area.height);
        let working_status_height = working_status_height(view_model, area.height);
        let subagent_height = subagent_activity_height(view_model, area.height);
        DockRenderPartition {
            panel: Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: panel_height,
            },
            working_status: Rect {
                x: area.x,
                y: area.y.saturating_add(panel_height),
                width: area.width,
                height: working_status_height,
            },
            input: Rect {
                x: area.x,
                y: area
                    .y
                    .saturating_add(panel_height)
                    .saturating_add(working_status_height),
                width: area.width,
                height: area
                    .height
                    .saturating_sub(panel_height)
                    .saturating_sub(working_status_height)
                    .saturating_sub(subagent_height),
            },
            subagent: Rect {
                x: area.x,
                y: area.bottom().saturating_sub(subagent_height),
                width: area.width,
                height: subagent_height,
            },
        }
    }
}

fn panel_height(view_model: &ComposerDockViewModel<'_>, dock_height: u16) -> u16 {
    let requested = view_model.panel.as_ref().map_or_else(
        || DockLifecycle::hidden().visible_height(),
        |panel| {
            debug_assert!(registered_dock_kinds().contains(&panel.kind()));
            panel.visible_height()
        },
    );
    let activity_height = subagent_activity_height(view_model, dock_height);
    requested.min(
        dock_height
            .saturating_sub(COMPOSER_DOCK_MIN_INPUT_HEIGHT)
            .saturating_sub(working_status_height(view_model, dock_height))
            .saturating_sub(activity_height),
    )
}

pub(super) fn subagent_activity_height(
    view_model: &ComposerDockViewModel<'_>,
    dock_height: u16,
) -> u16 {
    let activity_rows_height = view_model.subagent_activity.visible_height();
    let requested = if activity_rows_height == 0 {
        0
    } else {
        activity_rows_height.saturating_add(COMPOSER_DOCK_SEPARATOR_HEIGHT.saturating_mul(2))
    };
    requested.min(
        dock_height
            .saturating_sub(COMPOSER_DOCK_MIN_INPUT_HEIGHT)
            .saturating_sub(working_status_height(view_model, dock_height)),
    )
}

fn working_status_height(view_model: &ComposerDockViewModel<'_>, dock_height: u16) -> u16 {
    let required_height = COMPOSER_DOCK_MIN_INPUT_HEIGHT
        .saturating_add(view_model.subagent_activity.visible_height())
        .saturating_add(COMPOSER_DOCK_WORKING_STATUS_HEIGHT);
    if view_model.working_status.is_some() && dock_height >= required_height {
        COMPOSER_DOCK_WORKING_STATUS_HEIGHT
    } else {
        0
    }
}

fn render_panel(panel: Option<&ComposerDockPanel<'_>>, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    match panel {
        Some(ComposerDockPanel::BottomView { view, .. }) => {
            BottomPaneView { view }.render(area, buf);
        }
        None => {}
    }
}

fn render_input(view_model: &ComposerDockViewModel<'_>, area: Rect, buf: &mut Buffer) {
    let elements = view_model
        .draft
        .elements
        .iter()
        .map(|element| ComposerDraftElement {
            range: element.range.clone(),
            kind: match element.kind {
                ComposerDockDraftElementKind::Mention(sigil) => {
                    ComposerDraftElementKind::Mention(sigil)
                }
                ComposerDockDraftElementKind::File => ComposerDraftElementKind::File,
                ComposerDockDraftElementKind::Image => ComposerDraftElementKind::Image,
            },
        })
        .collect::<Vec<_>>();
    Composer {
        draft: ComposerDraft {
            text: view_model.draft.text,
            elements: &elements,
        },
        placeholder: PLACEHOLDER,
        command_suggestions: view_model.command_suggestions,
        popup_items: view_model.popup_items,
        suggestion_title: view_model.suggestion_title,
        selected_command_suggestion: view_model.selected_command_suggestion,
        queued_preview: view_model.queued_preview.as_deref(),
        draft_scroll_offset: view_model.draft_scroll_offset,
    }
    .render(area, buf);
}

fn render_working_status(status: Option<&WorkingStatusViewModel>, area: Rect, buf: &mut Buffer) {
    let Some(status) = status else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }
    let detail = status.detail_label();
    let role = match status.tone {
        WorkingStatusTone::Active | WorkingStatusTone::Success => TranscriptColorRole::Muted,
        WorkingStatusTone::Error => TranscriptColorRole::Error,
    };
    let mut spans = vec![Span::styled(status.prefix, role_style(role))];
    if status.animated {
        spans.extend(shimmer_spans(
            status.primary_label(),
            transcript_palette().color(role),
        ));
    } else {
        spans.push(Span::styled(
            status.primary_label().to_owned(),
            role_style(role),
        ));
    }
    if !detail.is_empty() {
        spans.push(Span::styled(
            format!("（{detail}）"),
            role_style(TranscriptColorRole::Muted),
        ));
    }
    let content_y = area
        .y
        .saturating_add(COMPOSER_DOCK_WORKING_STATUS_TOP_PADDING);
    buf.set_line(area.x, content_y, &Line::from(spans), area.width);
}

fn render_subagent_activity(snapshot: &SubagentActivitySnapshot, area: Rect, buf: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    render_activity_divider(area, buf);
    render_activity_bottom_divider(area, buf);
    let row_area = Rect {
        x: area.x.saturating_add(SUBAGENT_ACTIVITY_HORIZONTAL_PADDING),
        y: area.y.saturating_add(COMPOSER_DOCK_SEPARATOR_HEIGHT),
        width: area
            .width
            .saturating_sub(SUBAGENT_ACTIVITY_HORIZONTAL_PADDING.saturating_mul(2)),
        height: area
            .height
            .saturating_sub(COMPOSER_DOCK_SEPARATOR_HEIGHT.saturating_mul(2)),
    };
    for (row_index, row) in snapshot
        .rows
        .iter()
        .take(usize::from(row_area.height))
        .enumerate()
    {
        let y = row_area
            .y
            .saturating_add(u16::try_from(row_index).unwrap_or(u16::MAX));
        buf.set_string(
            row_area.x,
            y,
            " ".repeat(usize::from(row_area.width)),
            Style::default(),
        );
        buf.set_line(
            row_area.x,
            y,
            &subagent_activity_line(row, snapshot.animation_frame, row_area.width),
            row_area.width,
        );
    }
}

fn render_activity_divider(area: Rect, buf: &mut Buffer) {
    let divider = activity_divider_text(area.width);
    buf.set_string(
        area.x,
        area.y,
        divider,
        role_style(TranscriptColorRole::Muted),
    );
}

fn render_activity_bottom_divider(area: Rect, buf: &mut Buffer) {
    if area.height < COMPOSER_DOCK_SEPARATOR_HEIGHT.saturating_mul(2) {
        return;
    }
    let y = area.bottom().saturating_sub(1);
    let divider = ACTIVITY_DIVIDER.to_string().repeat(usize::from(area.width));
    buf.set_string(area.x, y, divider, role_style(TranscriptColorRole::Muted));
}

pub(super) fn activity_divider_text(width: u16) -> String {
    let width = usize::from(width);
    if width == 0 {
        return String::new();
    }
    if DisplayWidthText::width(SUBAGENT_ACTIVITY_SEPARATOR_LABEL) > width {
        return ACTIVITY_DIVIDER.to_string().repeat(width);
    }
    let remaining =
        width.saturating_sub(DisplayWidthText::width(SUBAGENT_ACTIVITY_SEPARATOR_LABEL));
    let left = remaining / 2;
    let right = remaining.saturating_sub(left);
    format!(
        "{}{}{}",
        ACTIVITY_DIVIDER.to_string().repeat(left),
        SUBAGENT_ACTIVITY_SEPARATOR_LABEL,
        ACTIVITY_DIVIDER.to_string().repeat(right)
    )
}

fn subagent_activity_line(
    row: &SubagentActivityRow,
    animation_frame: usize,
    width: u16,
) -> Line<'static> {
    let elapsed_ms = u64::try_from(row.elapsed.as_millis()).unwrap_or(u64::MAX);
    let elapsed = axon_display::format_duration_millis_compact(elapsed_ms);
    let mut suffix = row.label.clone();
    if let Some(model) = row
        .model
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        suffix.push_str(&format!(" · {model}"));
    }
    suffix.push_str(&format!(" · {}", row.activity));
    if let Some(tokens) = row.usage {
        suffix.push_str(&format!(
            " · token {}",
            axon_display::format_decimal_count(tokens)
        ));
    }
    if let Some(value) = row.cache_hit_rate_basis_points {
        suffix.push_str(&format!(
            " · 命中 {}",
            axon_display::format_basis_points_percent(value)
        ));
    }
    suffix.push_str(&format!(" · {elapsed}"));
    let marker = subagent_activity_marker(row.status, animation_frame);
    let prefix = format!("{marker} {SUBAGENT_PREFIX} ");
    let prefix_width = DisplayWidthText::width(&prefix);
    let suffix =
        DisplayWidthText::ellipsize(&suffix, usize::from(width).saturating_sub(prefix_width));
    Line::from(vec![
        Span::styled(prefix, subagent_activity_style(row.status)),
        Span::styled(suffix, Style::default().add_modifier(Modifier::DIM)),
    ])
}

fn subagent_activity_marker(
    status: SubagentActivityStatus,
    animation_frame: usize,
) -> &'static str {
    match status {
        SubagentActivityStatus::Starting | SubagentActivityStatus::Running => {
            spinner_frame(animation_frame)
        }
        SubagentActivityStatus::Waiting => WAITING_MARKER,
        SubagentActivityStatus::Completed => COMPLETED_MARKER,
        SubagentActivityStatus::Interrupted => INTERRUPTED_MARKER,
        SubagentActivityStatus::Failed => FAILED_MARKER,
    }
}

fn subagent_activity_style(status: SubagentActivityStatus) -> Style {
    let role = match status {
        SubagentActivityStatus::Starting | SubagentActivityStatus::Running => {
            TranscriptColorRole::SubagentActive
        }
        SubagentActivityStatus::Waiting => TranscriptColorRole::Accent,
        SubagentActivityStatus::Completed => TranscriptColorRole::ToolCompleted,
        SubagentActivityStatus::Interrupted => TranscriptColorRole::Muted,
        SubagentActivityStatus::Failed => TranscriptColorRole::Error,
    };
    role_style(role)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::{buffer::Buffer, layout::Rect};

    use super::{
        SubagentActivityRow, SubagentActivitySnapshot, SubagentActivityStatus,
        render_subagent_activity,
    };

    #[test]
    fn rerendering_activity_row_clears_previous_elapsed_text() {
        let area = Rect::new(0, 0, 120, 3);
        let mut buffer = Buffer::empty(area);
        let snapshot = |elapsed| SubagentActivitySnapshot {
            rows: vec![SubagentActivityRow {
                agent_id: "subagent:/root/example".to_owned(),
                label: "/root/example".to_owned(),
                status: SubagentActivityStatus::Running,
                activity: "生成中".to_owned(),
                elapsed,
                usage: Some(86_800),
                cache_hit_rate_basis_points: Some(6_049),
                model: Some("fixture-provider/fixture-model".to_owned()),
                summary: None,
            }],
            max_visible_rows: 1,
            animation_frame: 0,
        };

        render_subagent_activity(&snapshot(Duration::from_secs(214)), area, &mut buffer);
        render_subagent_activity(&snapshot(Duration::from_secs(208)), area, &mut buffer);

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("3m 28s"));
        assert!(!rendered.contains("3m 34s"));
        let rendered_compact = rendered.replace(' ', "");
        let label_index = rendered_compact
            .find("/root/example")
            .unwrap_or_else(|| panic!("应渲染任务路径：{rendered:?}"));
        let model_index = rendered_compact
            .find("fixture-provider/fixture-model")
            .unwrap_or_else(|| panic!("应渲染供应商与模型：{rendered:?}"));
        let activity_index = rendered_compact
            .find("生成中")
            .unwrap_or_else(|| panic!("应渲染活动状态：{rendered:?}"));
        let token_index = rendered_compact
            .find("token86.8K")
            .unwrap_or_else(|| panic!("应渲染 token：{rendered:?}"));
        let cache_rate_index = rendered_compact
            .find("命中60.49%")
            .unwrap_or_else(|| panic!("应渲染缓存命中率：{rendered:?}"));
        let elapsed_index = rendered_compact
            .find("3m28s")
            .unwrap_or_else(|| panic!("应渲染用时：{rendered:?}"));
        assert!(
            label_index < model_index
                && model_index < activity_index
                && activity_index < token_index
                && token_index < cache_rate_index
                && cache_rate_index < elapsed_index
        );
    }

    #[test]
    fn activity_rows_use_compact_horizontal_padding_for_cache_metrics() {
        let area = Rect::new(0, 0, 90, 3);
        let mut buffer = Buffer::empty(area);
        let snapshot = SubagentActivitySnapshot {
            rows: vec![SubagentActivityRow {
                agent_id: "subagent:/root/example".to_owned(),
                label: "/root/example".to_owned(),
                status: SubagentActivityStatus::Running,
                activity: "生成中".to_owned(),
                elapsed: Duration::from_secs(8),
                usage: Some(12_345),
                cache_hit_rate_basis_points: Some(6_000),
                model: Some("openai/gpt-test".to_owned()),
                summary: None,
            }],
            max_visible_rows: 1,
            animation_frame: 0,
        };

        render_subagent_activity(&snapshot, area, &mut buffer);

        assert_ne!(buffer[(2, 1)].symbol(), " ");
        assert_eq!(buffer[(1, 1)].symbol(), " ");
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.replace(' ', "").contains("命中60.00%"));
    }
}
