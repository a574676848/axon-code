use super::{MOUSE_SCROLL_STEP_ROWS, state::AgentTuiState};
use crate::{
    app_shell::layout::layout_app_shell,
    r#loop::agent_tui::output_dock::selected_tool_cell_part_id,
    surface::{
        cell::RenderCellData, height_index::LiveTranscriptTail, render_policy::RenderPolicy,
    },
    widgets::{TranscriptViewportRequest, transcript_view_metrics_with_height_cache},
};
#[cfg(test)]
use crate::{
    r#loop::agent_tui::state::viewport_state::TranscriptViewBoundsCause,
    surface::viewport_anchor::ScrollAnchor,
};
use ratatui::layout::Rect;

impl AgentTuiState {
    pub(super) fn toggle_transcript_expandable_at(&mut self, screen_row: u16) -> bool {
        let frame = self.viewport.frame_size;
        let area = layout_app_shell(
            Rect::new(0, 0, frame.width, frame.height),
            self.composer_height(),
            super::render::header_bar_height(self, frame.width),
        )
        .viewport;
        if screen_row < area.y || screen_row >= area.bottom() {
            return false;
        }

        let selected_runtime_cell = self.selected_runtime_render_index_by_identity();
        let live_tail = self.live_transcript_tail_lines();
        let active_tail = live_tail.as_ref().map(|(lines, key)| LiveTranscriptTail {
            lines: lines.as_slice(),
            key: *key,
        });
        let metrics = transcript_view_metrics_with_height_cache(TranscriptViewportRequest {
            lines: &self.cells.transcript,
            height: area.height,
            width: area.width,
            selected_cell: selected_runtime_cell,
            selected_line: None,
            active_tail,
            cache: &self.viewport.transcript_render_cache,
            height_cache: &self.viewport.transcript_height_cache,
        });
        let top_row = metrics
            .max_offset_rows
            .saturating_sub(self.viewport.transcript_view_offset_rows);
        let content_row = top_row.saturating_add(usize::from(screen_row - area.y));
        let Some(cell) = metrics
            .cells
            .iter()
            .find(|cell| cell.top <= content_row && content_row < cell.bottom())
        else {
            return false;
        };
        let Some(index) = cell.transcript_index else {
            return false;
        };
        let Some(record) = self.cells.transcript.get(index) else {
            return false;
        };
        let Some((part_id, is_tool)) = expandable_transcript_part(record) else {
            return false;
        };
        if self.viewport.selected_runtime_part_id() == Some(part_id.as_str()) {
            self.viewport.clear_runtime_selection();
        } else {
            if is_tool {
                self.hydrate_tool_detail_at(index, &part_id);
            }
            self.viewport.select_runtime_part(index, part_id);
        }
        true
    }

    pub(super) fn scroll_transcript_view_up(&mut self) {
        self.refresh_max_transcript_view_offset_rows();
        let offset_rows = self
            .viewport
            .transcript_view_offset_rows
            .saturating_add(usize::from(MOUSE_SCROLL_STEP_ROWS))
            .min(self.viewport.max_transcript_view_offset_rows);
        self.viewport.detach_transcript_view_at_offset(offset_rows);
    }

    pub(super) fn scroll_transcript_view_down(&mut self) {
        let offset_rows = self
            .viewport
            .transcript_view_offset_rows
            .saturating_sub(usize::from(MOUSE_SCROLL_STEP_ROWS));
        self.viewport.detach_transcript_view_at_offset(offset_rows);
    }

    pub(super) fn page_transcript_view_up(&mut self) {
        self.refresh_max_transcript_view_offset_rows();
        let offset_rows = self
            .viewport
            .transcript_view_offset_rows
            .saturating_add(self.transcript_view_page_rows())
            .min(self.viewport.max_transcript_view_offset_rows);
        self.viewport.detach_transcript_view_at_offset(offset_rows);
    }

    pub(super) fn page_transcript_view_down(&mut self) {
        let offset_rows = self
            .viewport
            .transcript_view_offset_rows
            .saturating_sub(self.transcript_view_page_rows());
        self.viewport.detach_transcript_view_at_offset(offset_rows);
    }

    pub(super) fn jump_transcript_view_top(&mut self) {
        self.refresh_max_transcript_view_offset_rows();
        self.viewport
            .detach_transcript_view_at_offset(self.viewport.max_transcript_view_offset_rows);
    }

    pub(super) fn jump_transcript_view_bottom(&mut self) {
        self.viewport.attach_transcript_view_bottom();
    }

    pub(super) fn refresh_max_transcript_view_offset_rows(&mut self) {
        let frame = self.viewport.frame_size;
        let area = layout_app_shell(
            Rect::new(0, 0, frame.width, frame.height),
            self.composer_height(),
            super::render::header_bar_height(self, frame.width),
        )
        .viewport;
        let selected_runtime_cell = self.selected_runtime_render_index_by_identity();
        let live_tail = self.live_transcript_tail_lines();
        let active_tail = live_tail.as_ref().map(|(lines, key)| LiveTranscriptTail {
            lines: lines.as_slice(),
            key: *key,
        });
        let metrics = transcript_view_metrics_with_height_cache(TranscriptViewportRequest {
            lines: &self.cells.transcript,
            height: area.height,
            width: area.width,
            selected_cell: selected_runtime_cell,
            selected_line: None,
            active_tail,
            cache: &self.viewport.transcript_render_cache,
            height_cache: &self.viewport.transcript_height_cache,
        });
        self.viewport.max_transcript_view_offset_rows = metrics.max_offset_rows;
        self.viewport.transcript_view_page_rows = usize::from(area.height.max(1));
    }

    #[cfg(test)]
    pub(super) fn restore_transcript_view_offset_bounds(
        &mut self,
        cause: TranscriptViewBoundsCause,
        max_offset_rows: usize,
        page_rows: u16,
        restored_anchor_row: Option<usize>,
        next_top_anchor: Option<ScrollAnchor>,
    ) {
        self.viewport.restore_transcript_view_bounds(
            cause,
            max_offset_rows,
            page_rows,
            restored_anchor_row,
            next_top_anchor,
        );
    }

    fn transcript_view_page_rows(&self) -> usize {
        self.viewport.transcript_view_page_rows.max(1)
    }

    pub(in crate::r#loop::agent_tui) fn selected_runtime_render_index_by_identity(
        &self,
    ) -> Option<usize> {
        if let Some(part_id) = self.viewport.selected_runtime_part_id() {
            return self.runtime_cell_index_for_part_id(part_id);
        }
        self.viewport
            .selected_runtime_render_index()
            .filter(|index| self.selectable_transcript_cell(*index))
    }

    fn selectable_transcript_cell(&self, index: usize) -> bool {
        self.cells
            .transcript
            .get(index)
            .is_some_and(|line| RenderPolicy::decide(line).selectable)
    }

    fn runtime_cell_index_for_part_id(&self, part_id: &str) -> Option<usize> {
        self.cells
            .transcript
            .iter()
            .position(|line| line.semantic_part_id().as_deref() == Some(part_id))
    }
}

fn expandable_transcript_part(
    record: &crate::surface::cell::RenderCellRecord,
) -> Option<(String, bool)> {
    if let Some(part_id) = selected_tool_cell_part_id(record) {
        return Some((part_id, true));
    }
    match record.data.as_ref() {
        Some(RenderCellData::Reasoning(cell)) if cell.done => {
            record.semantic_part_id().map(|part_id| (part_id, false))
        }
        _ => None,
    }
}
