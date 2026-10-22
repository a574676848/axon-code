//! Transcript 视口的高度索引、边界恢复与绘制。

use ratatui::layout::Rect;

use super::{custom_terminal::Frame, state::AgentTuiState};
use crate::r#loop::agent_tui::state::viewport_state::TranscriptViewBoundsCause;
use crate::widgets::{
    LiveTranscriptTail, build_transcript_viewport_height_index,
    render_transcript_viewport_with_height_index, transcript_viewport_metrics_from_height_index,
};
#[cfg(test)]
use crate::widgets::{TranscriptViewportRequest, transcript_view_metrics_with_height_cache};

pub(super) fn render_active_transcript(frame: &mut Frame, state: &mut AgentTuiState, area: Rect) {
    let selected_runtime_cell = state.selected_runtime_render_index_by_identity();
    let live_tail = state.live_transcript_tail_lines();
    let active_tail = live_tail.as_ref().map(|(lines, key)| LiveTranscriptTail {
        lines: lines.as_slice(),
        key: *key,
    });
    let previous_scroll_state = state.viewport.transcript_view_scroll_state;
    let previous_offset_rows = state.viewport.transcript_view_offset_rows;
    let previous_top_anchor = state.viewport.transcript_view_top_anchor;
    let height_index = build_transcript_viewport_height_index(
        &state.cells.transcript,
        selected_runtime_cell,
        None,
        active_tail,
        area.width,
        &state.viewport.transcript_render_cache,
        &state.viewport.transcript_height_cache,
    );
    let metrics = transcript_viewport_metrics_from_height_index(&height_index, area.height);
    let restored_anchor_row = previous_top_anchor.and_then(|anchor| metrics.anchor_row(anchor));
    let next_top_row = restored_transcript_view_top_row(
        previous_scroll_state.follows_tail(),
        previous_offset_rows,
        metrics.max_offset_rows,
        restored_anchor_row,
    );
    let next_top_anchor = metrics.capture_anchor(next_top_row);
    state.viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        metrics.max_offset_rows,
        area.height,
        restored_anchor_row,
        next_top_anchor,
    );
    let offset_rows = state.viewport.transcript_view_offset_rows;
    let (buffer, hyperlinks) = frame.surface_mut();
    render_transcript_viewport_with_height_index(
        height_index,
        offset_rows,
        false,
        &state.viewport.transcript_render_cache,
        area,
        buffer,
        Some(hyperlinks),
    );
}

#[cfg(test)]
pub(super) fn refresh_main_viewport_bounds(state: &mut AgentTuiState, area: Rect) {
    let selected_runtime_cell = state.selected_runtime_render_index_by_identity();
    let live_tail = state.live_transcript_tail_lines();
    let active_tail = live_tail.as_ref().map(|(lines, key)| LiveTranscriptTail {
        lines: lines.as_slice(),
        key: *key,
    });
    let metrics = transcript_view_metrics_with_height_cache(TranscriptViewportRequest {
        lines: &state.cells.transcript,
        height: area.height,
        width: area.width,
        selected_cell: selected_runtime_cell,
        selected_line: None,
        active_tail,
        cache: &state.viewport.transcript_render_cache,
        height_cache: &state.viewport.transcript_height_cache,
    });
    let restored_anchor_row = state
        .viewport
        .transcript_view_top_anchor
        .and_then(|anchor| metrics.anchor_row(anchor));
    let next_top_row = restored_transcript_view_top_row(
        state.viewport.transcript_view_scroll_state.follows_tail(),
        state.viewport.transcript_view_offset_rows,
        metrics.max_offset_rows,
        restored_anchor_row,
    );
    let next_top_anchor = metrics.capture_anchor(next_top_row);
    state.restore_transcript_view_offset_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        metrics.max_offset_rows,
        area.height,
        restored_anchor_row,
        next_top_anchor,
    );
}

fn restored_transcript_view_top_row(
    follows_tail: bool,
    offset_rows: usize,
    max_offset_rows: usize,
    restored_anchor_row: Option<usize>,
) -> usize {
    if follows_tail {
        return max_offset_rows;
    }
    restored_anchor_row
        .map(|row| row.min(max_offset_rows))
        .unwrap_or_else(|| max_offset_rows.saturating_sub(offset_rows.min(max_offset_rows)))
}
