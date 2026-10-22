//! 文件变更 cell 渲染。

use ratatui::text::{Line, Span};

use crate::{
    surface::{
        cell::RenderCellData, display_width::DisplayWidthText, render_policy::RenderPolicyDecision,
    },
    widgets::{transcript_patch_files, transcript_style::CellVisual},
};

pub(super) fn render_patch(
    visual: CellVisual,
    decision: RenderPolicyDecision,
    selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(RenderCellData::Diff(cell)) = data else {
        return Vec::new();
    };
    let mut lines =
        transcript_patch_files::render_file_changes(visual, decision, selected, cell, width);
    append_toggle_hint(
        &mut lines,
        visual.detail_text_style,
        decision.render_expanded(selected),
        decision.openable,
        width,
    );
    lines
}

fn append_toggle_hint(
    lines: &mut [Line<'static>],
    style: ratatui::style::Style,
    expanded: bool,
    openable: bool,
    width: u16,
) {
    if !openable {
        return;
    }
    let Some(first) = lines.first_mut() else {
        return;
    };
    let hint = if expanded {
        " · 点击收起"
    } else {
        " · 点击查看详情"
    };
    let rendered_width = DisplayWidthText::width(&first.to_string());
    if rendered_width.saturating_add(DisplayWidthText::width(hint)) > usize::from(width) {
        return;
    }
    first.spans.push(Span::styled(hint, style));
}
