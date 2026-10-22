//! Agent TUI 运行时提交与增量事件流。

use std::time::{Duration, Instant};

use crate::r#loop::agent_tui::{
    AgentTuiSubmitFailure, RuntimeError, TUI_HINT_DONE,
    run_output::{
        AgentTuiLifecycleLine, AgentTuiMessagePartRole, AgentTuiRunLine, AgentTuiRunOutput,
        AgentTuiRuntimeEvent, AgentTuiRuntimePayload, AgentTuiToolCompletionView,
        AgentTuiToolDiffSnapshotView, AgentTuiToolExecutionView, AgentTuiToolLifecycleLine,
        AgentTuiTranscriptPart, render_transcript_parts,
    },
    state::{AgentRuntimeStatus, AgentTuiState},
    tool_output_projection::{
        merge_tool_result_text, tool_execution_started_line, tool_lifecycle_line,
        tool_output_chunk_line, tool_output_chunk_preview, tool_progress_line,
        tool_progress_preview, tool_result_line,
    },
};
use crate::surface::event_envelope::RuntimeEventId;
use crate::surface::render_policy::RenderPolicy;
use crate::surface::{
    cell::{
        ROLE_AGENT, ROLE_USER, RenderCellData, RenderCellKind, RenderCellRecord,
        RenderGenerationStatusCell,
    },
    cell_event_data::AgentRenderMcpCell,
};

use super::runtime_error_projection::{runtime_error_line, runtime_error_output};
use super::runtime_flow_cell_access::{
    exec_cell_data, exec_cell_data_mut, mcp_cell_data, mcp_cell_data_mut,
};
use super::runtime_flow_dispatch::dispatch_runtime_payload;
use super::runtime_flow_helpers::{
    duration_since, lifecycle_status_is_retry, merge_tool_output_chunk_into_cell,
    run_line_is_transient_final_output, stamp_exec_duration,
};
use super::runtime_wire_policy::wire_event_transaction_name;

#[path = "runtime_flow_tools.rs"]
mod runtime_flow_tools;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_runtime_error_output(
        &mut self,
        output: AgentTuiRunOutput,
    ) {
        self.apply_run_output(output, true, false, None);
    }

    pub(in crate::r#loop::agent_tui) fn restore_historical_turn(
        &mut self,
        output: AgentTuiRunOutput,
    ) {
        if output.is_empty() {
            return;
        }
        let usage = output.token_usage_total();
        self.apply_run_output(output, false, false, None);
        let usage = self.settle_turn_usage(usage);
        self.header
            .session_summary
            .record_turn(false, usage, Duration::ZERO);
        self.push_historical_turn_summary(usage);
        self.header.hint = TUI_HINT_DONE;
        self.bridge.runtime_status = AgentRuntimeStatus::Ready;
        self.bridge.generation_performance.turn_terminal = true;
    }

    #[cfg(test)]
    pub(in crate::r#loop::agent_tui) fn finish_submit(
        &mut self,
        response: Result<AgentTuiRunOutput, RuntimeError>,
    ) {
        self.finish_submit_with_cause(
            response.map_err(AgentTuiSubmitFailure::without_output),
            None,
        );
    }

    pub(in crate::r#loop::agent_tui) fn finish_submit_with_cause(
        &mut self,
        response: Result<AgentTuiRunOutput, AgentTuiSubmitFailure>,
        caused_by: Option<RuntimeEventId>,
    ) {
        let before = self.invariant_snapshot();
        let failed = response.is_err();
        let output = match response {
            Ok(output) => output,
            Err(failure) => {
                log_runtime_submit_failure(failure.error(), caused_by);
                failure_terminal_output(failure, caused_by)
            }
        };
        let finished_at = Instant::now();
        let elapsed = self.bridge.live_status.finish(finished_at);
        let token_usage = self.settle_turn_usage(output.token_usage_total());
        self.header
            .session_summary
            .record_turn(failed, token_usage, elapsed);
        let output_has_plan = output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::Plan(_)));
        let final_review = output.lines().iter().find_map(|line| match line {
            AgentTuiRunLine::Review(review) => Some(review.clone()),
            _ => None,
        });
        let active_plan = self.take_live_plan_line();
        if !output_has_plan && let Some(active_plan) = active_plan {
            self.settle_active_plan_cell(active_plan);
        }
        let active_review_settled = self.settle_active_review_with_final(final_review.as_ref());
        if failed {
            self.persist_failure_live_lifecycle_line();
        }
        // 终态只做两件事：封口实时流、按 durable 顺序幂等重放终态投影。
        // 不再做 assistant 补偿、文本去重或终态淘汰，避免与 post-foreground 过滤互相打架。
        self.flush_live_assistant_output();
        let held_assistant = self.take_current_turn_final_assistant_cell();
        self.commit_active_reasoning_cell();
        self.bridge.live_lifecycle_line = None;
        self.bridge.live_hook_line = None;
        self.bridge.provider_activity.clear();
        self.bridge.subscription_phase = None;
        self.apply_run_output(output, failed, active_review_settled, held_assistant);
        self.push_turn_summary_if_needed(elapsed, token_usage);
        if failed {
            self.fail_resume_replay();
        } else {
            self.complete_resume_replay();
        }
        self.request_catalog_reload(false);
        self.bridge.live_lifecycle_line = None;
        self.bridge.live_hook_line = None;
        self.bridge.provider_activity.clear();
        self.bridge.subscription_phase = None;
        self.clear_active_lifecycle_cells();
        self.header.hint = TUI_HINT_DONE;
        self.bridge.runtime_status = AgentRuntimeStatus::Ready;
        self.bridge.generation_performance.turn_terminal = true;
        self.advance_terminal_title_lifecycle();
        self.composer.input_queue.user_turn_pending_start = false;
        self.clear_pending_steers();
        self.clear_cancel_edit_candidate();
        self.bridge.interrupt_requested = false;
        self.debug_assert_transaction_invariants("finish_submit", before);
    }

    /// 按 durable 顺序应用终态投影。
    ///
    /// 每条投影行携带 payload 自有的稳定 part id；已由实时流提交的 cell 会被原位替换，
    /// 未提交的行才追加，因此重复重放是幂等的。
    /// `held_assistant` 由轮次收口方摘出：canonical assistant 是本轮 durable 流最后的事实，
    /// 重放期间补齐的过程事实必须落在它之前，重放结束后最终正文仍由 canonical 行落位；
    /// resume 与历史恢复路径传入 `None`，不改变既有顺序。
    pub(super) fn apply_run_output(
        &mut self,
        output: AgentTuiRunOutput,
        failed: bool,
        skip_final_review: bool,
        held_assistant: Option<RenderCellRecord>,
    ) {
        let mut applied_assistant = false;
        for line in output.into_lines() {
            if skip_final_review && matches!(line, AgentTuiRunLine::Review(_)) {
                continue;
            }
            if failed && self.persist_failure_lifecycle_line(&line) {
                continue;
            }
            if run_line_is_transient_final_output(&line) {
                continue;
            }
            applied_assistant |= run_line_is_assistant_message(&line);
            if self.replace_existing_final_cells(&line) {
                continue;
            }
            self.apply_run_line(line);
        }
        if !applied_assistant && let Some(assistant) = held_assistant {
            self.cells.transcript.push(assistant);
        }
        self.prune_tool_output_buffers();
    }

    /// 摘出本轮已提交的最终 assistant cell，重放结束后再落位。
    fn take_current_turn_final_assistant_cell(&mut self) -> Option<RenderCellRecord> {
        let current_turn_start = self.current_turn_start_index();
        let index = self
            .cells
            .transcript
            .iter()
            .enumerate()
            .skip(current_turn_start)
            .rfind(|(_, line)| {
                line.role == ROLE_AGENT && matches!(line.data, Some(RenderCellData::Message(_)))
            })
            .map(|(index, _)| index)?;
        let assistant = self.cells.transcript.remove(index);
        self.cells
            .active_runtime_cells
            .remove_transcript_index(index);
        self.cells
            .transcript_surfaces
            .remove_transcript_index(index);
        self.viewport.rebase_runtime_selection_after_remove(index);
        Some(assistant)
    }

    fn replace_existing_final_cells(&mut self, line: &AgentTuiRunLine) -> bool {
        if matches!(
            line,
            AgentTuiRunLine::ToolReceipt(_)
                | AgentTuiRunLine::ToolLifecycle(_)
                | AgentTuiRunLine::Reasoning(_)
        ) {
            return false;
        }
        let cells = line.clone().render_cells();
        if cells.is_empty() {
            return false;
        }
        let replacements = cells
            .into_iter()
            .map(|cell| {
                let part_id = cell.semantic_part_id()?;
                let index = self.current_turn_transcript_index_by_part_id(&part_id)?;
                Some((index, cell))
            })
            .collect::<Option<Vec<_>>>();
        let Some(replacements) = replacements else {
            return false;
        };
        for (index, cell) in replacements {
            let _ = self.cells.transcript.replace_cell(index, cell);
        }
        true
    }

    fn current_turn_start_index(&self) -> usize {
        self.cells
            .transcript
            .iter()
            .rposition(|line| line.role == ROLE_USER)
            .map_or(0, |index| index.saturating_add(1))
    }

    fn current_turn_transcript_index_by_part_id(&self, part_id: &str) -> Option<usize> {
        self.cells
            .transcript
            .iter()
            .enumerate()
            .skip(self.current_turn_start_index())
            .find_map(|(index, line)| {
                (line.semantic_part_id().as_deref() == Some(part_id)).then_some(index)
            })
    }

    fn persist_failure_lifecycle_line(&mut self, line: &AgentTuiRunLine) -> bool {
        let AgentTuiRunLine::GenerationStatus(lifecycle) = line else {
            return false;
        };
        let retry_items = lifecycle
            .items
            .iter()
            .filter(|item| lifecycle_status_is_retry(item.kind.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if retry_items.is_empty() {
            return false;
        }
        let retry_line = AgentTuiLifecycleLine::with_id(
            lifecycle.id.clone(),
            lifecycle.title.clone(),
            retry_items,
        );
        self.push_persisted_part(AgentTuiTranscriptPart::GenerationStatus(retry_line));
        true
    }

    fn persist_failure_live_lifecycle_line(&mut self) -> bool {
        let Some(line) = &self.bridge.live_lifecycle_line else {
            return false;
        };
        let Some(RenderCellData::GenerationStatus(cell)) = &line.data else {
            return false;
        };
        let retry_items = cell
            .items
            .iter()
            .filter(|item| lifecycle_status_is_retry(item.kind.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if retry_items.is_empty() {
            return false;
        }
        self.push_policy_persisted_line(RenderCellRecord::generation_status_with_data(
            RenderGenerationStatusCell {
                id: cell.id.clone(),
                title: cell.title.clone(),
                items: retry_items,
            },
        ));
        true
    }

    pub(super) fn remove_startup_transcript_lines(&mut self) {
        self.cells
            .transcript
            .retain(|line| !matches!(line.mode, crate::surface::cell::RenderCellMode::Startup));
    }

    fn apply_run_line(&mut self, line: AgentTuiRunLine) {
        match line {
            AgentTuiRunLine::Session(info) => {
                self.header.overrides.session = Some(info.session_id.clone());
                self.header.overrides.continue_last = false;
                self.clear_transient_runtime_state();
                self.header.session_summary.record_session(
                    info.session_id,
                    info.resumed_from,
                    info.title,
                );
                self.complete_resume_replay();
            }
            AgentTuiRunLine::Usage(_) => {}
            AgentTuiRunLine::ToolReceipt(receipt) => {
                self.record_turn_work_activity();
                let detail = receipt
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("outputPreview"))
                    .and_then(serde_json::Value::as_str)
                    .or(Some(receipt.summary.as_str()));
                self.append_tool_result_detail_if_absent(&receipt.id, &receipt.tool_name, detail);
                if self.settle_active_diff_cell_without_tool_summary(&receipt) {
                    let _ = self.cells.active_runtime_cells.take_tool_cell(&receipt.id);
                    return;
                }
                self.settle_tool_execution_cell(&receipt);
            }
            AgentTuiRunLine::ToolLifecycle(tool) => {
                self.record_turn_work_activity();
                self.settle_tool_lifecycle_cell(&tool);
            }
            AgentTuiRunLine::Diff(diff) => {
                self.record_turn_work_activity();
                if !self.replace_active_tool_cell_with_diff_snapshot(None, diff.clone()) {
                    self.push_persisted_part(AgentTuiTranscriptPart::Diff(diff));
                }
            }
            AgentTuiRunLine::Plan(plan) => {
                self.replace_or_append_active_plan_cell(plan);
            }
            AgentTuiRunLine::Review(review) => {
                self.replace_or_append_active_review_cell(review);
            }
            AgentTuiRunLine::Status(status) => {
                self.replace_or_append_active_status_cell(status);
            }
            AgentTuiRunLine::History(history) => {
                self.push_persisted_part(AgentTuiTranscriptPart::History(history));
            }
            AgentTuiRunLine::Hook(hook) => {
                self.record_turn_work_activity();
                self.apply_hook_status(hook);
            }
            AgentTuiRunLine::GenerationStatus(status) => {
                if let Some(line) =
                    render_transcript_parts([AgentTuiTranscriptPart::GenerationStatus(status)])
                        .into_iter()
                        .next()
                {
                    self.replace_or_push_lifecycle_cell(RenderCellKind::GENERATION_STATUS, line);
                }
            }
            other => {
                self.push_persisted_parts(other.into_transcript_parts());
            }
        }
    }

    fn push_persisted_parts(&mut self, parts: Vec<AgentTuiTranscriptPart>) {
        for part in parts {
            self.push_persisted_part(part);
        }
    }

    fn push_persisted_part(&mut self, part: AgentTuiTranscriptPart) -> bool {
        if self.merge_completed_reasoning_part(&part) {
            return true;
        }
        let mut persisted = false;
        for line in render_transcript_parts([part]) {
            persisted |= self.push_policy_persisted_line(line);
        }
        persisted
    }

    fn merge_completed_reasoning_part(&mut self, part: &AgentTuiTranscriptPart) -> bool {
        let AgentTuiTranscriptPart::Reasoning(reasoning) = part else {
            return false;
        };
        let existing_index = self
            .cells
            .transcript
            .iter()
            .enumerate()
            .find_map(|(index, line)| {
                matches!(
                    &line.data,
                    Some(RenderCellData::Reasoning(cell)) if cell.id == reasoning.id
                )
                .then_some(index)
            });
        let Some(index) = existing_index else {
            return false;
        };
        let Some(existing) = self.cells.transcript.get_mut(index) else {
            return false;
        };
        let Some(RenderCellData::Reasoning(cell)) = existing.data.as_mut() else {
            return false;
        };
        cell.text.clone_from(&reasoning.text);
        cell.done = reasoning.done;
        existing.text.clone_from(&reasoning.text);
        existing.bump_render_revision();
        true
    }

    fn push_policy_persisted_line(&mut self, line: RenderCellRecord) -> bool {
        if !RenderPolicy::decide(&line).history_persisted {
            return false;
        }
        self.cells.transcript.push(line);
        true
    }

    pub(in crate::r#loop::agent_tui) fn apply_runtime_event(
        &mut self,
        event: AgentTuiRuntimeEvent,
    ) {
        let before = self.invariant_snapshot();
        let transaction = runtime_event_transaction_name(&event.payload);
        self.header.session_summary.record_runtime_event(&event);
        let accepted = self.bridge.accept_event(&event);
        if !accepted {
            self.debug_assert_transaction_invariants(transaction, before);
            return;
        }
        if self.bridge.telemetry.first_token_enabled() {
            self.bridge
                .telemetry
                .observe_first_token_mailbox(event.transport_timing.as_ref(), None);
        }
        let snapshot = self.bridge.apply_accepted(event);
        let _event_id = snapshot.event_id;
        let is_visible = match &snapshot.payload {
            AgentTuiRuntimePayload::BatchedAssistantDelta { deltas, .. } => {
                deltas.iter().any(|d| !d.is_empty())
            }
            AgentTuiRuntimePayload::BatchedReasoningDelta { deltas, .. } => {
                deltas.iter().any(|d| !d.is_empty())
            }
            _ => false,
        };
        dispatch_runtime_payload(self, snapshot.payload);
        if is_visible {
            self.bridge.telemetry.observe_first_visible_applied();
        }
        self.debug_assert_transaction_invariants(transaction, before);
    }

    pub(in crate::r#loop::agent_tui) fn finish_restored_runtime(&mut self) {
        let before = self.invariant_snapshot();
        self.bridge.runtime_status = AgentRuntimeStatus::Ready;
        self.bridge.generation_performance.turn_terminal = true;
        self.advance_terminal_title_lifecycle();
        self.composer.input_queue.user_turn_pending_start = false;
        self.bridge.live_status.finish(Instant::now());
        self.bridge.live_lifecycle_line = None;
        self.bridge.live_hook_line = None;
        self.bridge.provider_activity.clear();
        self.bridge.subscription_phase = None;
        self.commit_active_reasoning_cell();
        self.cells.active_runtime_cells.clear_hook_runs();
        self.terminal_session_projection.clear();
        self.debug_assert_transaction_invariants("finish_restored_runtime", before);
    }
}

fn runtime_event_transaction_name(payload: &AgentTuiRuntimePayload) -> &'static str {
    match payload {
        AgentTuiRuntimePayload::BatchedAssistantDelta { .. } => {
            "runtime_event:BatchedAssistantDelta"
        }
        AgentTuiRuntimePayload::ReasoningPartStarted { .. } => "runtime_event:ReasoningPartStarted",
        AgentTuiRuntimePayload::BatchedReasoningDelta { .. } => {
            "runtime_event:BatchedReasoningDelta"
        }
        AgentTuiRuntimePayload::ReasoningPartEnded { .. } => "runtime_event:ReasoningPartEnded",
        AgentTuiRuntimePayload::WireEvent { event, .. } => wire_event_transaction_name(&event.kind),
        AgentTuiRuntimePayload::WireResumeSnapshot(_) => "runtime_event:WireResumeSnapshot",
        AgentTuiRuntimePayload::WireRuntimeError(_) => "runtime_event:WireRuntimeError",
        AgentTuiRuntimePayload::Watcher(_) => "runtime_event:Watcher",
        AgentTuiRuntimePayload::CancelRequested(_) => "runtime_event:CancelRequested",
        AgentTuiRuntimePayload::InterruptRequested(_) => "runtime_event:InterruptRequested",
        AgentTuiRuntimePayload::SubscriptionPhase(_) => "runtime_event:SubscriptionPhase",
        AgentTuiRuntimePayload::Observation(_) => "runtime_event:Observation",
    }
}

fn run_line_is_assistant_message(line: &AgentTuiRunLine) -> bool {
    matches!(
        line,
        AgentTuiRunLine::Message(message)
            if message.role == AgentTuiMessagePartRole::Assistant
    )
}

/// 失败终态投影：宿主已交出 durable 全量重放时直接收口，缺失时才退化为仅错误行。
///
/// 实时邮箱只淘汰可重建的展示增量（见 `submission_worker_mailbox`），被淘汰的增量
/// 只能由终态按 durable 顺序完整重放补齐；宿主给出了重放就必须使用它。
fn failure_terminal_output(
    failure: AgentTuiSubmitFailure,
    caused_by: Option<RuntimeEventId>,
) -> AgentTuiRunOutput {
    let error = failure.error().clone();
    let mut output = failure.into_output();
    if output.is_empty() {
        return runtime_error_output(error, caused_by);
    }
    let missing_error_line = !output
        .lines()
        .iter()
        .any(|line| matches!(line, AgentTuiRunLine::Error(_)));
    if missing_error_line {
        output.push(runtime_error_line(&error, caused_by));
    }
    output
}

fn log_runtime_submit_failure(error: &RuntimeError, caused_by: Option<RuntimeEventId>) {
    axon_logger::error(&format!(
        "agent_tui_submit_failed: message={}, error_type={:?}, error_code={}, caused_by={}, next_step={}",
        error.message(),
        error.error_type(),
        error.error_code().unwrap_or("none"),
        caused_by
            .map(|event_id| format!("runtime-event-{}", event_id.get()))
            .unwrap_or_else(|| "runtime-finish".to_owned()),
        error.next_step().unwrap_or("none")
    ));
}

#[cfg(test)]
#[path = "runtime_flow_internal_tests.rs"]
mod tests;
