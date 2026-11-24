//! 工具执行中的 diff 快照 active cell 管理。

use crate::r#loop::agent_tui::{
    run_output::{AgentTuiRunLine, AgentTuiToolReceipt},
    run_output_diff::AgentTuiDiffLine,
    state::AgentTuiState,
};

impl AgentTuiState {
    pub(super) fn replace_active_tool_cell_with_diff_snapshot(
        &mut self,
        tool_part_id: Option<&str>,
        diff: AgentTuiDiffLine,
    ) -> bool {
        let diff_id = diff.id.clone();
        let Some(diff_line) = AgentTuiRunLine::Diff(diff).render_cell() else {
            return false;
        };
        let tool_part_id = tool_part_id.filter(|value| !value.trim().is_empty());

        // A single edit/write can emit one snapshot per changed path. Keep those
        // snapshots in the same transcript cell instead of projecting each one.
        if let Some(existing_index) = self.diff_snapshot_index(tool_part_id, &diff_id)
            && let Some(existing) = self.cells.transcript.get_mut(existing_index)
            && let (
                Some(crate::surface::cell::RenderCellData::Diff(existing_data)),
                Some(crate::surface::cell::RenderCellData::Diff(incoming_data)),
            ) = (existing.data.as_mut(), diff_line.data.as_ref())
        {
            existing_data.changes.extend(incoming_data.changes.clone());
            for (path, diagnostics) in &incoming_data.diagnostics {
                existing_data
                    .diagnostics
                    .entry(path.clone())
                    .or_default()
                    .extend(diagnostics.iter().cloned());
            }
            existing_data.text_writes_applied = existing_data
                .text_writes_applied
                .max(incoming_data.text_writes_applied);
            existing_data.renames_applied = existing_data
                .renames_applied
                .max(incoming_data.renames_applied);
            existing_data.truncated |= incoming_data.truncated;
            if existing_data.truncation_boundary.is_none() {
                existing_data.truncation_boundary = incoming_data.truncation_boundary.clone();
            }
            existing.bump_render_revision();
            return true;
        }

        let active_part_id = tool_part_id
            .map(ToOwned::to_owned)
            .or_else(|| self.active_tool_part_id_for_diff(&diff_id));
        let active_part_id = active_part_id.as_deref();
        if let Some(active_part_id) = active_part_id
            && let Some(active_tool) = self
                .cells
                .active_runtime_cells
                .take_tool_cell(active_part_id)
            && let Some(existing) = self.cells.transcript.get_mut(active_tool.index)
        {
            existing.replace_with(diff_line);
            self.cells
                .active_runtime_cells
                .mark_diff_snapshot_tool_part(active_part_id.to_owned());
            self.prune_tool_output_buffers();
            return true;
        }
        if let Some(existing_index) = self.transcript_index_by_part_id(&diff_id)
            && let Some(existing) = self.cells.transcript.get_mut(existing_index)
        {
            existing.replace_with(diff_line);
            self.cells
                .active_runtime_cells
                .clear_tool_cell_at_index(existing_index);
            self.prune_tool_output_buffers();
            return true;
        }
        if let Some(active_part_id) = active_part_id
            && let Some(existing_index) = self.exec_index_by_part_id(active_part_id)
            && let Some(existing) = self.cells.transcript.get_mut(existing_index)
        {
            existing.replace_with(diff_line);
            self.cells
                .active_runtime_cells
                .clear_tool_cell_at_index(existing_index);
            self.prune_tool_output_buffers();
            return true;
        }
        self.cells.transcript.push(diff_line);
        if let Some(tool_part_id) = active_part_id {
            let _ = self.cells.active_runtime_cells.take_tool_cell(tool_part_id);
            self.cells
                .active_runtime_cells
                .mark_diff_snapshot_tool_part(tool_part_id.to_owned());
        }
        self.prune_tool_output_buffers();
        true
    }

    pub(super) fn settle_active_diff_cell_without_tool_summary(
        &mut self,
        receipt: &AgentTuiToolReceipt,
    ) -> bool {
        if self
            .cells
            .active_runtime_cells
            .take_diff_snapshot_tool_part(&receipt.id)
        {
            return true;
        }
        if !receipt.provider_call_id.trim().is_empty()
            && self
                .cells
                .active_runtime_cells
                .take_diff_snapshot_tool_part(&receipt.provider_call_id)
        {
            return true;
        }
        if receipt.filesystem_write_performed
            && (receipt.kind == crate::AgentTuiToolKind::Edit
                || receipt.tool_name == "builtin.edit")
            && self.cells.transcript.iter().any(|line| {
                matches!(
                    line.data.as_ref(),
                    Some(crate::surface::cell::RenderCellData::Diff(_))
                )
            })
        {
            return true;
        }
        false
    }

    fn diff_snapshot_index(&self, tool_part_id: Option<&str>, diff_id: &str) -> Option<usize> {
        self.cells
            .transcript
            .iter()
            .enumerate()
            .find_map(|(index, line)| {
                let crate::surface::cell::RenderCellData::Diff(cell) = line.data.as_ref()? else {
                    return None;
                };
                let matches_tool = tool_part_id.is_some_and(|tool_part_id| {
                    cell.id
                        .strip_prefix("diff:")
                        .is_some_and(|id| id.starts_with(tool_part_id))
                }) || tool_part_id.is_none()
                    && cell.id.rsplit_once(':').is_some_and(|(prefix, _)| {
                        diff_id.starts_with(prefix)
                            && diff_id.as_bytes().get(prefix.len()) == Some(&b':')
                    });
                (cell.id == diff_id || matches_tool).then_some(index)
            })
    }

    fn active_tool_part_id_for_diff(&self, diff_id: &str) -> Option<String> {
        self.cells
            .active_runtime_cells
            .tool_part_ids()
            .find(|part_id| {
                diff_id
                    .strip_prefix("diff:")
                    .is_some_and(|id| id.starts_with(*part_id))
            })
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.cells
                    .transcript
                    .iter()
                    .find_map(|line| match line.data.as_ref()? {
                        crate::surface::cell::RenderCellData::Exec(cell)
                            if diff_id
                                .strip_prefix("diff:")
                                .is_some_and(|id| id.starts_with(cell.id.as_str())) =>
                        {
                            Some(cell.id.clone())
                        }
                        _ => None,
                    })
            })
    }

    fn exec_index_by_part_id(&self, part_id: &str) -> Option<usize> {
        self.cells
            .transcript
            .iter()
            .enumerate()
            .find_map(|(index, line)| match line.data.as_ref()? {
                crate::surface::cell::RenderCellData::Exec(cell) if cell.id == part_id => {
                    Some(index)
                }
                _ => None,
            })
    }
}
