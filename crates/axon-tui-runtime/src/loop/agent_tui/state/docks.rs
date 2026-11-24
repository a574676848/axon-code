//! Agent TUI Dock 状态入口。

use crate::app_shell::composer_dock::{DockKind, registered_dock_kinds};
use crate::r#loop::agent_tui::{
    bottom_view::{BottomView, ShortcutHelpDock},
    output_dock::{ToolOutputDock, selected_tool_cell_part_id},
    shortcut_help::shortcut_entries,
};
use std::collections::HashSet;

use crate::surface::{
    cell::{ROLE_TOOL, RenderCellData},
    tool_output_buffer::{TOOL_RESULT_DETAIL_STREAM, ToolOutputBuffer},
};

use super::AgentTuiState;

use crate::r#loop::agent_tui::MOUSE_SCROLL_STEP_ROWS;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn active_dock_kind(&self) -> Option<DockKind> {
        self.composer.bottom_views.top_kind()
    }

    pub(in crate::r#loop::agent_tui) fn dock_open(&self, kind: DockKind) -> bool {
        self.composer.bottom_views.contains_kind(kind)
    }

    pub(in crate::r#loop::agent_tui) fn dock_registry_kinds() -> &'static [DockKind] {
        registered_dock_kinds()
    }

    pub(in crate::r#loop::agent_tui) fn is_shortcut_help_open(&self) -> bool {
        self.dock_open(DockKind::ShortcutHelp)
    }

    pub(in crate::r#loop::agent_tui) fn toggle_shortcut_help_dock(&mut self) {
        if self.is_shortcut_help_open() {
            self.close_dock(DockKind::ShortcutHelp);
            return;
        }
        self.open_exclusive_dock(BottomView::ShortcutHelp(ShortcutHelpDock::new(
            shortcut_entries(),
        )));
    }

    pub(in crate::r#loop::agent_tui) fn close_shortcut_help_dock(&mut self) {
        self.close_dock(DockKind::ShortcutHelp);
    }

    pub(in crate::r#loop::agent_tui) fn close_tool_output_viewer_dock(&mut self) {
        self.close_dock(DockKind::ToolOutputViewer);
        self.viewport.tool_output_dock = ToolOutputDock::default();
    }

    pub(in crate::r#loop::agent_tui) fn open_selected_tool_output_dock(&mut self) -> bool {
        let Some(index) = self.selected_runtime_render_index_by_identity() else {
            return self.open_latest_tool_output_dock();
        };
        if self.open_tool_output_dock_at(index) {
            return true;
        }
        self.open_latest_tool_output_dock()
    }

    pub(in crate::r#loop::agent_tui) fn scroll_tool_output_dock_up(&mut self) {
        if let Some(dock) = self.current_tool_output_dock_mut() {
            dock.scroll_up();
            self.viewport.tool_output_dock = dock.clone();
        }
    }

    pub(in crate::r#loop::agent_tui) fn scroll_tool_output_dock_down(&mut self) {
        if let Some(dock) = self.current_tool_output_dock_mut() {
            dock.scroll_down();
            self.viewport.tool_output_dock = dock.clone();
        }
    }

    pub(in crate::r#loop::agent_tui) fn scroll_tool_output_dock_mouse_up(&mut self) {
        if let Some(dock) = self.current_tool_output_dock_mut() {
            dock.scroll_up_by(MOUSE_SCROLL_STEP_ROWS);
            self.viewport.tool_output_dock = dock.clone();
        }
    }

    pub(in crate::r#loop::agent_tui) fn scroll_tool_output_dock_mouse_down(&mut self) {
        if let Some(dock) = self.current_tool_output_dock_mut() {
            dock.scroll_down_by(MOUSE_SCROLL_STEP_ROWS);
            self.viewport.tool_output_dock = dock.clone();
        }
    }

    pub(in crate::r#loop::agent_tui) fn jump_tool_output_dock_top(&mut self) {
        if let Some(dock) = self.current_tool_output_dock_mut() {
            dock.jump_top();
            self.viewport.tool_output_dock = dock.clone();
        }
    }

    pub(in crate::r#loop::agent_tui) fn jump_tool_output_dock_bottom(&mut self) {
        if let Some(dock) = self.current_tool_output_dock_mut() {
            dock.jump_bottom();
            self.viewport.tool_output_dock = dock.clone();
        }
    }

    pub(in crate::r#loop::agent_tui) fn jump_tool_output_dock_error(&mut self) -> bool {
        let Some(dock) = self.current_tool_output_dock_mut() else {
            return false;
        };
        let jumped = dock.jump_error();
        self.viewport.tool_output_dock = dock.clone();
        jumped
    }

    pub(in crate::r#loop::agent_tui) fn close_all_docks(&mut self) {
        self.composer.bottom_views.clear();
        self.viewport.clear_runtime_selection();
    }

    pub(in crate::r#loop::agent_tui) fn close_top_dock(&mut self) {
        let _ = self.composer.bottom_views.request_close_top();
    }

    fn close_dock(&mut self, kind: DockKind) {
        self.composer.bottom_views.close_kind(kind);
    }

    pub(in crate::r#loop::agent_tui) fn open_exclusive_dock(&mut self, view: BottomView) {
        debug_assert!(Self::dock_registry_kinds().contains(&view.kind()));
        self.viewport.clear_runtime_selection();
        self.close_composer_popup();
        self.composer.command_suggestions.clear();
        self.composer.bottom_views.open_exclusive(view);
    }

    fn open_latest_tool_output_dock(&mut self) -> bool {
        let Some(index) = self
            .cells
            .transcript
            .iter()
            .rposition(|line| selected_tool_cell_part_id(line).is_some())
        else {
            return false;
        };
        self.open_tool_output_dock_at(index)
    }

    pub(in crate::r#loop::agent_tui) fn open_tool_output_dock_at(&mut self, index: usize) -> bool {
        let Some(part_id) = self
            .cells
            .transcript
            .get(index)
            .and_then(selected_tool_cell_part_id)
        else {
            return false;
        };
        let Some(output) = self.cells.tool_outputs_by_part_id.get(&part_id) else {
            return false;
        };
        let title = output.title();
        let view = output.view();
        if !self.viewport.tool_output_dock.open(title, view) {
            return false;
        }
        self.open_exclusive_dock(BottomView::ToolOutputViewer(
            self.viewport.tool_output_dock.clone(),
        ));
        true
    }

    fn current_tool_output_dock_mut(&mut self) -> Option<&mut ToolOutputDock> {
        self.composer
            .bottom_views
            .top_mut()
            .and_then(|entry| entry.view_mut().tool_output_mut())
    }

    pub(in crate::r#loop::agent_tui) fn append_tool_output_chunk(
        &mut self,
        chunk: &crate::AgentTuiToolOutputChunk,
    ) {
        let projection_part_id = self
            .terminal_session_projection
            .projection_part_id(&chunk.part_id)
            .unwrap_or(&chunk.part_id)
            .to_owned();
        self.cells
            .tool_outputs_by_part_id
            .entry(projection_part_id)
            .or_insert_with(|| ToolOutputBuffer::new(&chunk.tool_name))
            .push_chunk(&chunk.stream, chunk.sequence, &chunk.text);
    }

    pub(in crate::r#loop::agent_tui) fn append_tool_result_detail_if_absent(
        &mut self,
        part_id: &str,
        tool_name: &str,
        detail: Option<&str>,
    ) -> bool {
        let Some(detail) = detail.map(str::trim).filter(|value| !value.is_empty()) else {
            return false;
        };
        if self
            .cells
            .active_runtime_cells
            .is_diff_snapshot_tool_part(part_id)
            || self.cells.tool_outputs_by_part_id.contains_key(part_id)
        {
            return false;
        }
        let mut output = ToolOutputBuffer::new(tool_name);
        output.push_chunk(TOOL_RESULT_DETAIL_STREAM, 0, detail);
        self.cells
            .tool_outputs_by_part_id
            .insert(part_id.to_owned(), output);
        true
    }

    pub(in crate::r#loop::agent_tui) fn prune_tool_output_buffers(&mut self) -> usize {
        let mut retained_part_ids = self
            .cells
            .transcript
            .iter()
            .filter(|line| line.role == ROLE_TOOL)
            .filter_map(crate::surface::cell::RenderCellRecord::semantic_part_id)
            .collect::<HashSet<_>>();
        retained_part_ids.extend(
            self.cells
                .active_runtime_cells
                .tool_part_ids()
                .map(ToOwned::to_owned),
        );
        let before = self.cells.tool_outputs_by_part_id.len();
        self.cells
            .tool_outputs_by_part_id
            .retain(|part_id, _| retained_part_ids.contains(part_id));
        before.saturating_sub(self.cells.tool_outputs_by_part_id.len())
    }

    pub(in crate::r#loop::agent_tui) fn hydrate_tool_detail_at(
        &mut self,
        index: usize,
        part_id: &str,
    ) -> bool {
        let Some(detail) = self
            .cells
            .tool_outputs_by_part_id
            .get(part_id)
            .and_then(ToolOutputBuffer::inline_text)
        else {
            return false;
        };
        let Some(record) = self.cells.transcript.get_mut(index) else {
            return false;
        };
        let changed = match record.data.as_mut() {
            Some(RenderCellData::Exec(cell)) => {
                if cell.output_preview.as_deref() == Some(detail.as_str()) {
                    false
                } else {
                    cell.output_preview = Some(detail);
                    true
                }
            }
            Some(RenderCellData::Mcp(cell)) => {
                let Some(item) = cell.items.first_mut() else {
                    return false;
                };
                if item.output_preview.as_deref() == Some(detail.as_str()) {
                    false
                } else {
                    item.output_preview = Some(detail);
                    true
                }
            }
            _ => false,
        };
        if changed {
            record.bump_render_revision();
        }
        changed
    }
}
