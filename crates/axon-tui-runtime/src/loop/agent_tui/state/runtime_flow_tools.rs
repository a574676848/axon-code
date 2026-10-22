//! TUI 工具执行生命周期的状态投影。

use super::*;

const TOOL_WRITE_STDIN: &str = "write_stdin";
const TERMINAL_INTERACTION_KEY: &str = "terminalInteraction";
const TERMINAL_INTERACTION_KIND_KEY: &str = "kind";
const TERMINAL_INTERACTION_WAIT: &str = "wait";
const TOOL_ARGUMENTS_PREVIEW_KEY: &str = "argumentsPreview";
const TOOL_SESSION_ID_KEY: &str = "session_id";
const TOOL_CHARS_KEY: &str = "chars";
const TOOL_CTRL_C_KEY: &str = "ctrl_c";

impl AgentTuiState {
    fn terminal_wait_cell_index(
        &self,
        receipt: &crate::r#loop::agent_tui::run_output::AgentTuiToolReceipt,
    ) -> Option<usize> {
        let metadata = receipt.metadata.as_ref()?;
        let interaction = metadata.get(TERMINAL_INTERACTION_KEY)?;
        if interaction
            .get(TERMINAL_INTERACTION_KIND_KEY)
            .and_then(serde_json::Value::as_str)
            != Some(TERMINAL_INTERACTION_WAIT)
        {
            return None;
        }
        let command = metadata
            .get(TOOL_ARGUMENTS_PREVIEW_KEY)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if receipt.tool_name.rsplit(['.', ':', '/']).next() != Some(TOOL_WRITE_STDIN) {
            return None;
        }
        self.latest_shell_cell_index(Some(command))
    }

    fn latest_shell_cell_index(&self, command: Option<&str>) -> Option<usize> {
        self.cells
            .transcript
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, line)| {
                let cell = exec_cell_data(line)?;
                let shell_cell = cell
                    .tool_name
                    .rsplit(['.', ':', '/'])
                    .next()
                    .is_some_and(|name| name == "shell_command");
                if !shell_cell {
                    return None;
                }
                command
                    .map(|command| {
                        cell.arguments_preview.as_deref() == Some(command)
                            || cell.display_target.as_deref() == Some(command)
                    })
                    .unwrap_or(true)
                    .then_some(index)
            })
    }

    fn wait_call_arguments(arguments: &str) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) else {
            return false;
        };
        let has_input = value
            .get(TOOL_CHARS_KEY)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|chars| !chars.is_empty());
        let interrupted = value
            .get(TOOL_CTRL_C_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        !interrupted
            && !has_input
            && value
                .get(TOOL_SESSION_ID_KEY)
                .and_then(serde_json::Value::as_str)
                .is_some_and(|session_id| !session_id.trim().is_empty())
    }

    fn upsert_tool_execution_cell(
        &mut self,
        part_id: String,
        line: RenderCellRecord,
        now: Instant,
    ) {
        if let Some(index) = self.cells.active_runtime_cells.tool_cell(&part_id)
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            existing.replace_with(line);
            return;
        }
        self.cells.transcript.push(line);
        self.cells.active_runtime_cells.set_tool_cell(
            part_id,
            self.cells.transcript.len().saturating_sub(1),
            now,
        );
    }

    pub(in crate::r#loop::agent_tui) fn apply_tool_execution_started(
        &mut self,
        view: AgentTuiToolExecutionView,
    ) {
        let now = Instant::now();
        if let Some(projection_part_id) = self.terminal_session_projection.begin_wait_call(
            &view.part_id,
            &view.tool_name,
            &view.arguments_preview,
        ) {
            if let Some(index) = self.transcript_index_by_part_id(&projection_part_id) {
                self.cells
                    .active_runtime_cells
                    .set_tool_cell(projection_part_id, index, now);
                return;
            }
            self.terminal_session_projection
                .cancel_wait_call(&view.part_id);
        }
        if view.tool_name.rsplit(['.', ':', '/']).next() == Some(TOOL_WRITE_STDIN)
            && Self::wait_call_arguments(&view.arguments_preview)
            && let Some(index) = self.latest_shell_cell_index(None)
            && let Some(projection_part_id) = self
                .cells
                .transcript
                .get(index)
                .and_then(RenderCellRecord::semantic_part_id)
        {
            self.terminal_session_projection
                .bind_wait_call_to_projection(&view.part_id, &projection_part_id);
            return;
        }
        self.upsert_tool_execution_cell(
            view.part_id.clone(),
            tool_execution_started_line(&view),
            now,
        );
    }

    pub(in crate::r#loop::agent_tui) fn apply_tool_diff_snapshot(
        &mut self,
        view: AgentTuiToolDiffSnapshotView,
    ) {
        self.record_turn_work_activity();
        let tool_part_id = view.tool_part_id.clone();
        self.replace_active_tool_cell_with_diff_snapshot(tool_part_id.as_deref(), view.into_diff());
    }

    pub(in crate::r#loop::agent_tui) fn apply_tool_execution_completed(
        &mut self,
        view: AgentTuiToolCompletionView,
    ) {
        let duration_ms = view.duration_ms;
        let receipt = view.into_receipt();
        if self.settle_active_diff_cell_without_tool_summary(&receipt) {
            let _ = self.cells.active_runtime_cells.take_tool_cell(&receipt.id);
        } else {
            self.settle_tool_execution_cell_with_duration(&receipt, duration_ms);
        }
    }

    pub(in crate::r#loop::agent_tui) fn merge_tool_output_chunk_cell(
        &mut self,
        chunk: &crate::r#loop::agent_tui::run_output::AgentTuiToolOutputChunk,
    ) {
        let projection_part_id = self
            .terminal_session_projection
            .projection_part_id(&chunk.part_id)
            .unwrap_or(&chunk.part_id)
            .to_owned();
        if let Some(index) = self
            .cells
            .active_runtime_cells
            .tool_cell(&projection_part_id)
            .or_else(|| self.active_tool_index_by_provider_call_id(&chunk.provider_call_id))
            .or_else(|| self.transcript_index_by_part_id(&projection_part_id))
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            merge_tool_output_chunk_into_cell(existing, chunk);
            return;
        }
        if chunk.semantic_kind == crate::r#loop::agent_tui::run_output::AgentTuiToolKind::Mcp {
            let mut projected_chunk = chunk.clone();
            projected_chunk.part_id = projection_part_id.clone();
            self.upsert_tool_execution_cell(
                projection_part_id,
                RenderCellRecord::mcp_with_data(AgentRenderMcpCell::from_tool_output_chunk(
                    &projected_chunk,
                    tool_output_chunk_preview(&projected_chunk),
                )),
                Instant::now(),
            );
            return;
        }
        let mut projected_chunk = chunk.clone();
        projected_chunk.part_id = projection_part_id.clone();
        self.upsert_tool_execution_cell(
            projection_part_id,
            tool_output_chunk_line(&projected_chunk),
            Instant::now(),
        );
    }

    pub(in crate::r#loop::agent_tui) fn merge_tool_progress_cell(
        &mut self,
        progress: &crate::r#loop::agent_tui::run_output::AgentTuiToolProgressView,
    ) {
        let preview = tool_progress_preview(progress);
        let projection_part_id = self
            .terminal_session_projection
            .projection_part_id(&progress.part_id)
            .unwrap_or(&progress.part_id)
            .to_owned();
        if let Some(index) = self
            .cells
            .active_runtime_cells
            .tool_cell(&projection_part_id)
            .or_else(|| self.active_tool_index_by_provider_call_id(&progress.provider_call_id))
            .or_else(|| self.transcript_index_by_part_id(&projection_part_id))
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            if let Some(cell) = exec_cell_data_mut(existing) {
                cell.merge_progress_preview(preview.clone());
                existing.bump_render_revision();
            }
            if let Some(cell) = mcp_cell_data_mut(existing) {
                cell.merge_output_preview(preview);
                existing.bump_render_revision();
            }
            return;
        }
        if progress.semantic_kind == crate::r#loop::agent_tui::run_output::AgentTuiToolKind::Mcp {
            let mut projected_progress = progress.clone();
            projected_progress.part_id = projection_part_id.clone();
            self.upsert_tool_execution_cell(
                projection_part_id,
                RenderCellRecord::mcp_with_data(AgentRenderMcpCell::from_tool_progress(
                    &projected_progress,
                    preview,
                )),
                Instant::now(),
            );
            return;
        }
        let mut projected_progress = progress.clone();
        projected_progress.part_id = projection_part_id.clone();
        self.upsert_tool_execution_cell(
            projection_part_id,
            tool_progress_line(&projected_progress),
            Instant::now(),
        );
    }

    pub(super) fn settle_tool_execution_cell(
        &mut self,
        receipt: &crate::r#loop::agent_tui::run_output::AgentTuiToolReceipt,
    ) {
        self.settle_tool_execution_cell_with_duration(receipt, None);
    }

    fn settle_tool_execution_cell_with_duration(
        &mut self,
        receipt: &crate::r#loop::agent_tui::run_output::AgentTuiToolReceipt,
        duration_ms: Option<u64>,
    ) {
        let mut line = tool_result_line(receipt);
        let active_tool = self.take_tool_cell_for_provider(&receipt.id, &receipt.provider_call_id);
        let duration_ms = duration_ms
            .or_else(|| active_tool.and_then(|(_, started_at)| duration_since(started_at)));
        stamp_exec_duration(&mut line, duration_ms);
        if let Some(active_tool) = active_tool
            && let Some(existing) = self.cells.transcript.get_mut(active_tool.0)
        {
            if let (Some(previous), Some(next)) =
                (exec_cell_data(existing), exec_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
                let text = merge_tool_result_text(&existing.text, receipt);
                existing.replace_with(line);
                existing.text = text;
                existing.bump_render_revision();
                return;
            }
            if let (Some(previous), Some(next)) =
                (mcp_cell_data(existing), mcp_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
                existing.replace_with(line);
                return;
            }
            existing.replace_with(line);
            return;
        }
        if let Some(existing_index) = self.transcript_index_by_part_id(&receipt.id)
            && let Some(existing) = self.cells.transcript.get_mut(existing_index)
        {
            if let (Some(previous), Some(next)) =
                (exec_cell_data(existing), exec_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
            }
            if let (Some(previous), Some(next)) =
                (mcp_cell_data(existing), mcp_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
            }
            existing.replace_with(line);
            self.cells
                .active_runtime_cells
                .clear_tool_cell_at_index(existing_index);
            return;
        }
        if let Some(existing_index) = self.terminal_wait_cell_index(receipt)
            && let Some(existing) = self.cells.transcript.get_mut(existing_index)
        {
            if let (Some(previous), Some(next)) =
                (exec_cell_data(existing), exec_cell_data_mut(&mut line))
            {
                let previous_id = previous.id.clone();
                next.fill_missing_previews_from(previous);
                next.id = previous_id;
            }
            existing.replace_with(line);
            self.cells
                .active_runtime_cells
                .clear_tool_cell_at_index(existing_index);
            return;
        }
        if receipt.filesystem_write_performed
            && (receipt.kind == crate::AgentTuiToolKind::Edit
                || receipt.tool_name == "builtin.edit")
            && self.cells.transcript.iter().any(|item| {
                matches!(
                    item.data.as_ref(),
                    Some(crate::surface::cell::RenderCellData::Diff(_))
                )
            })
        {
            return;
        }
        self.cells.transcript.push(line);
    }

    pub(super) fn settle_tool_lifecycle_cell(&mut self, lifecycle: &AgentTuiToolLifecycleLine) {
        let mut line = tool_lifecycle_line(lifecycle);
        let active_tool =
            self.take_tool_cell_for_provider(&lifecycle.tool_part_id, &lifecycle.provider_call_id);
        let duration_ms = active_tool.and_then(|(_, started_at)| duration_since(started_at));
        stamp_exec_duration(&mut line, duration_ms);
        if let Some(active_tool) = active_tool
            && let Some(existing) = self.cells.transcript.get_mut(active_tool.0)
        {
            if let (Some(previous), Some(next)) =
                (exec_cell_data(existing), exec_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
            }
            if let (Some(previous), Some(next)) =
                (mcp_cell_data(existing), mcp_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
            }
            existing.replace_with(line);
            return;
        }
        if let Some(existing_index) = self.transcript_index_by_part_id(&lifecycle.tool_part_id)
            && let Some(existing) = self.cells.transcript.get_mut(existing_index)
        {
            if let (Some(previous), Some(next)) =
                (exec_cell_data(existing), exec_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
            }
            if let (Some(previous), Some(next)) =
                (mcp_cell_data(existing), mcp_cell_data_mut(&mut line))
            {
                next.fill_missing_previews_from(previous);
            }
            existing.replace_with(line);
            return;
        }
        if lifecycle.kind == crate::AgentTuiToolKind::Edit
            && self.cells.transcript.iter().any(|item| {
                matches!(
                    item.data.as_ref(),
                    Some(crate::surface::cell::RenderCellData::Diff(_))
                )
            })
        {
            return;
        }
        self.cells.transcript.push(line);
    }

    pub(in crate::r#loop::agent_tui) fn transcript_index_by_part_id(
        &self,
        part_id: &str,
    ) -> Option<usize> {
        self.cells
            .transcript
            .iter()
            .position(|line| line.semantic_part_id().as_deref() == Some(part_id))
    }

    fn active_tool_index_by_provider_call_id(&self, provider_call_id: &str) -> Option<usize> {
        self.cells
            .active_runtime_cells
            .active_tool_cells()
            .into_iter()
            .find_map(|tool| {
                self.cells
                    .transcript
                    .get(tool.index)
                    .filter(|line| line.provider_call_id() == Some(provider_call_id))
                    .map(|_| tool.index)
            })
    }

    fn take_tool_cell_for_provider(
        &mut self,
        part_id: &str,
        provider_call_id: &str,
    ) -> Option<(usize, Option<std::time::Instant>)> {
        if let Some(active) = self.cells.active_runtime_cells.take_tool_cell(part_id) {
            return Some((active.index, active.started_at));
        }
        let fallback_part_id = self
            .cells
            .active_runtime_cells
            .active_runtime_parts()
            .into_iter()
            .find(|part| {
                self.cells
                    .transcript
                    .get(part.index)
                    .is_some_and(|line| line.provider_call_id() == Some(provider_call_id))
            })
            .map(|part| part.part_id);
        fallback_part_id.and_then(|part_id| {
            self.cells
                .active_runtime_cells
                .take_tool_cell(&part_id)
                .map(|active| (active.index, active.started_at))
        })
    }
}
