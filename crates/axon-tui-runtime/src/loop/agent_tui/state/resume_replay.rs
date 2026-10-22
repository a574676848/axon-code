//! Resume 会话回放 UX 与 snapshot 状态编排。

use std::{collections::BTreeMap, path::PathBuf};

use axon_domain::{RUNTIME_FAILURE_TIMESTAMP_UNKNOWN, RuntimeFailure};
use axon_protocol::wire::{
    WireFileChangeEvent, WireGovernanceStatus, WireLifecycleStatus, WirePlanUpdate,
    WireRenderTimelineItem, WireRenderTimelineItemKind, WireResumeSnapshot, WireReviewStatus,
    WireRuntimeError, WireToolCall, WireToolResult, WireTurnSummary, WireUsageUpdate,
};

use crate::{
    r#loop::agent_tui::{
        TUI_HINT_DONE,
        error_projection::run_error_cell,
        run_output::{
            AgentTuiMessagePart, AgentTuiToolKind, AgentTuiToolReceipt, AgentTuiToolReceiptDetails,
            AgentTuiTranscriptPart, render_transcript_parts,
        },
        state::{
            runtime_error_projection::{
                agent_tui_run_error_from_wire, log_wire_runtime_error, wire_runtime_error_output,
            },
            runtime_flow_cell_access::exec_cell_data_mut,
            runtime_flow_helpers::stamp_exec_duration,
        },
        tool_output_projection::tool_result_line,
    },
    surface::{
        cell::{RenderCellAction, RenderCellRecord, RenderStatusCell, RenderStatusItem},
        cell_diff_data::AgentRenderDiffCell,
        cell_event_data::{AgentRenderExecCell, AgentRenderUsageCell},
        cell_plan_data::{
            AgentRenderPlanCell, AgentRenderPlanSourceKind, AgentRenderPlanStatus,
            AgentRenderPlanStep,
        },
        cell_review_data::{
            AgentRenderReviewCell, AgentRenderReviewDetail, AgentRenderReviewDetailKind,
            AgentRenderReviewStatus,
        },
        cell_turn_summary_data::AgentRenderTurnSummaryCell,
        replay_snapshot::{ReplaySnapshotHeaderState, replay_failed_cell, replay_loading_cell},
    },
};

#[cfg(test)]
use crate::surface::cell_store::CellStore;

const RESUME_RUNTIME_ERROR_EVENT_ID: &str = "resume-runtime-error";
const RESUME_RUNTIME_ERROR_SCOPE: &str = "resume";

use super::AgentTuiState;

impl AgentTuiState {
    #[cfg(test)]
    pub(in crate::r#loop::agent_tui) fn begin_resume_replay(&mut self) {
        self.cells = CellStore::new(Vec::new());
        self.header.replay_snapshot = Some(ReplaySnapshotHeaderState::loading(0, true));
        self.upsert_resume_replay_cell(replay_loading_cell(0));
    }

    pub(in crate::r#loop::agent_tui) fn complete_resume_replay(&mut self) {
        let Some(replay) = self.header.replay_snapshot else {
            return;
        };
        if !replay.is_loading() {
            return;
        }
        self.header.replay_snapshot = Some(ReplaySnapshotHeaderState::completed(
            replay.loaded_cells,
            true,
        ));
        self.take_resume_replay_cell();
    }

    pub(in crate::r#loop::agent_tui) fn fail_resume_replay(&mut self) {
        let Some(replay) = self.header.replay_snapshot else {
            return;
        };
        if !replay.is_loading() {
            return;
        }
        let loaded_cells = replay.loaded_cells;
        self.header.replay_snapshot = Some(ReplaySnapshotHeaderState::error(loaded_cells, true));
        self.upsert_resume_replay_cell(replay_failed_cell(loaded_cells));
    }

    pub(in crate::r#loop::agent_tui) fn apply_wire_resume_snapshot(
        &mut self,
        snapshot: WireResumeSnapshot,
    ) {
        if let Err(message) = validate_timeline_part_ids(&snapshot.timeline) {
            self.apply_resume_runtime_error(RuntimeFailure::new(
                "snapshot",
                "corrupt_resume_snapshot",
                message,
                RUNTIME_FAILURE_TIMESTAMP_UNKNOWN,
            ));
            return;
        }
        let viewport_handoff = self.viewport.handoff_state();
        let preserve_viewport_handoff = !self.viewport.transcript_view_scroll_state.follows_tail();
        let session_id = snapshot.session.session_id.clone();
        let run_stream = super::super::runtime_bridge::wire_stream_key(
            &session_id,
            snapshot.session.run_id.as_deref(),
        );
        if let Some(sequence) = snapshot.timeline.iter().map(|item| item.sequence).max() {
            self.bridge.turn_ledger.seed_sequence(run_stream, sequence);
        }
        for item in &snapshot.timeline {
            if matches!(&item.kind, WireRenderTimelineItemKind::ToolResult(_)) {
                self.settle_projection_once(&item.part_id, "final_receipt");
            }
        }
        self.header.cwd = snapshot
            .status_bar
            .cwd
            .clone()
            .unwrap_or_else(|| self.header.cwd.clone());
        self.header.overrides.session = Some(session_id.clone());
        self.header.overrides.continue_last = false;
        let usage = snapshot
            .status_bar
            .usage
            .map(usage_from_wire)
            .unwrap_or_default();
        self.header.session_usage = usage;
        self.header.last_turn_usage = usage;
        self.header.live_turn_usage = crate::AgentTuiTokenUsage::default();
        self.header.session_summary.record_session(
            session_id.clone(),
            snapshot.session.parent_session_id.clone(),
            None,
        );
        self.header.session_summary.token_usage = usage;
        self.clear_transient_runtime_state();
        self.finish_restored_runtime();
        self.header.replay_snapshot = Some(ReplaySnapshotHeaderState::loading(0, true));
        self.upsert_resume_replay_cell(replay_loading_cell(0));

        // 回放的轮次摘要同时补进会话级生成台账，恢复后 header 的均值与加权速率才有历史样本。
        for item in &snapshot.timeline {
            if let WireRenderTimelineItemKind::TurnSummary(summary) = &item.kind {
                let first_token_ms = summary.first_token_avg_ms;
                let output_tokens = summary
                    .usage
                    .map(|usage| {
                        usage
                            .output_tokens
                            .saturating_add(usage.reasoning_output_tokens)
                    })
                    .unwrap_or_default();
                let decode_ms = summary
                    .duration_ms
                    .zip(first_token_ms)
                    .map_or(0, |(duration, ttft)| duration.saturating_sub(ttft));
                self.bridge.session_generation.seed_turn(
                    &summary.turn_id,
                    first_token_ms,
                    output_tokens,
                    decode_ms,
                );
            }
        }

        // UQ-204: 扫描未结算提问并在重启/恢复后重新打开 dock
        let pending_question = snapshot.timeline.iter().rev().find_map(|item| {
            if let WireRenderTimelineItemKind::UserQuestion(q) = &item.kind
                && (q.status == "pending" || q.status == "suspended")
            {
                Some(q.clone())
            } else {
                None
            }
        });

        let render_cells = render_timeline(snapshot.timeline);
        let loaded_cells = render_cells.len();
        self.take_resume_replay_cell();
        self.cells.transcript.extend(render_cells);
        self.header.replay_snapshot =
            Some(ReplaySnapshotHeaderState::completed(loaded_cells, true));
        if preserve_viewport_handoff {
            self.viewport.restore_handoff_state(viewport_handoff);
        }
        self.header.hint = TUI_HINT_DONE;

        if let Some(q) = pending_question {
            self.open_bottom_view(crate::r#loop::agent_tui::bottom_view::BottomView::Question(
                crate::r#loop::agent_tui::bottom_view::QuestionView::new(
                    session_id,
                    q.call_id,
                    q.questions,
                    q.deadline_at_ms,
                ),
            ));
        }
    }

    pub(in crate::r#loop::agent_tui) fn apply_resume_runtime_error(
        &mut self,
        error: WireRuntimeError,
    ) {
        log_wire_runtime_error(
            RESUME_RUNTIME_ERROR_SCOPE,
            &error,
            RESUME_RUNTIME_ERROR_EVENT_ID,
        );
        let loaded_cells = self
            .header
            .replay_snapshot
            .map_or(0, |replay| replay.loaded_cells);
        self.header.replay_snapshot = Some(ReplaySnapshotHeaderState::error(loaded_cells, true));
        self.upsert_resume_replay_cell(replay_failed_cell(loaded_cells));
        self.apply_runtime_error_output(wire_runtime_error_output(
            &error,
            RESUME_RUNTIME_ERROR_EVENT_ID,
        ));
    }

    fn take_resume_replay_cell(&mut self) {
        let Some(index) = self.cells.transcript_surfaces.resume_replay_cell() else {
            return;
        };
        let _ = self.cells.transcript.remove(index);
        self.cells
            .transcript_surfaces
            .remove_transcript_index(index);
    }

    fn upsert_resume_replay_cell(&mut self, cell: RenderCellRecord) {
        if let Some(index) = self.cells.transcript_surfaces.resume_replay_cell()
            && self
                .cells
                .transcript
                .replace_cell(index, cell.clone())
                .is_ok()
        {
            return;
        }
        self.cells.transcript.push(cell);
        self.cells
            .transcript_surfaces
            .set_resume_replay_cell(self.cells.transcript.len().saturating_sub(1));
    }
}

fn render_timeline(timeline: Vec<WireRenderTimelineItem>) -> Vec<RenderCellRecord> {
    timeline
        .into_iter()
        .flat_map(render_timeline_item)
        .collect()
}

fn validate_timeline_part_ids(timeline: &[WireRenderTimelineItem]) -> Result<(), String> {
    for item in timeline {
        let Some(payload_part_id) = payload_part_id(&item.kind) else {
            continue;
        };
        if payload_part_id != item.part_id {
            return Err(format!(
                "wire resume snapshot part_id 不一致：timeline={} payload={payload_part_id}",
                item.part_id
            ));
        }
    }
    Ok(())
}

fn payload_part_id(kind: &WireRenderTimelineItemKind) -> Option<&str> {
    match kind {
        WireRenderTimelineItemKind::AssistantMessage(message) => Some(&message.part_id),
        WireRenderTimelineItemKind::UserMessage(message) => Some(&message.part_id),
        WireRenderTimelineItemKind::ReasoningMessage(message) => Some(&message.part_id),
        WireRenderTimelineItemKind::ToolCall(call) => Some(&call.part_id),
        WireRenderTimelineItemKind::ToolResult(result) => Some(&result.part_id),
        WireRenderTimelineItemKind::FileChange(change) => Some(&change.part_id),
        WireRenderTimelineItemKind::PlanUpdate(plan) => Some(&plan.part_id),
        WireRenderTimelineItemKind::ReviewStatus(review) => Some(&review.part_id),
        WireRenderTimelineItemKind::TurnSummary(summary) => Some(&summary.part_id),
        WireRenderTimelineItemKind::UserQuestion(question) => Some(&question.part_id),
        WireRenderTimelineItemKind::GovernanceStatus(_)
        | WireRenderTimelineItemKind::UsageUpdate(_)
        | WireRenderTimelineItemKind::Error(_) => None,
    }
}

fn render_timeline_item(item: WireRenderTimelineItem) -> Vec<RenderCellRecord> {
    match item.kind {
        WireRenderTimelineItemKind::AssistantMessage(message) => {
            render_single_part(AgentTuiTranscriptPart::Message(
                AgentTuiMessagePart::assistant(message.part_id, message.content),
            ))
        }
        WireRenderTimelineItemKind::UserMessage(message) => {
            // 可见正文口径由 `AgentTuiMessagePart::user` 统一收口：回执块只属于
            // 编码器，不能原样摊进转录。
            render_single_part(AgentTuiTranscriptPart::Message(AgentTuiMessagePart::user(
                message.part_id,
                message.content,
            )))
        }
        WireRenderTimelineItemKind::ReasoningMessage(_) => Vec::new(),
        WireRenderTimelineItemKind::ToolCall(call) => vec![render_tool_call(call)],
        WireRenderTimelineItemKind::ToolResult(result) => vec![render_tool_result(result)],
        WireRenderTimelineItemKind::FileChange(change) => {
            render_file_change(change).into_iter().collect()
        }
        WireRenderTimelineItemKind::PlanUpdate(plan) => {
            vec![render_plan_update(plan)]
        }
        WireRenderTimelineItemKind::ReviewStatus(review) => {
            vec![render_review_status(review)]
        }
        WireRenderTimelineItemKind::GovernanceStatus(governance) => {
            vec![render_governance_status(item.part_id, governance)]
        }
        WireRenderTimelineItemKind::UsageUpdate(usage) => {
            vec![RenderCellRecord::usage_with_data(usage_cell_from_wire(
                item.part_id,
                usage,
            ))]
        }
        WireRenderTimelineItemKind::TurnSummary(summary) => {
            vec![render_turn_summary(summary)]
        }
        WireRenderTimelineItemKind::Error(failure) => {
            vec![render_error(item.part_id, &failure)]
        }
        WireRenderTimelineItemKind::UserQuestion(question) => {
            vec![RenderCellRecord::question_with_data(
                crate::surface::cell_question_data::AgentRenderQuestionCell::new(
                    item.part_id,
                    question.call_id,
                    question.status,
                    question.questions,
                    question.answers,
                    question.handoff,
                    question.deadline_at_ms,
                ),
            )]
        }
    }
}

fn render_single_part(part: AgentTuiTranscriptPart) -> Vec<RenderCellRecord> {
    render_transcript_parts([part])
}

fn render_tool_call(call: WireToolCall) -> RenderCellRecord {
    let mut exec_cell = AgentRenderExecCell::running_with_id(
        call.part_id,
        call.tool_name,
        call.tool_call_id,
        call.arguments_summary,
    );
    exec_cell.summary = Some(call.summary);
    RenderCellRecord::exec_with_data(exec_cell, RenderCellAction::Running)
}

fn render_tool_result(result: WireToolResult) -> RenderCellRecord {
    let tool_name = result
        .tool_name
        .clone()
        .unwrap_or_else(|| result.tool_call_id.clone());
    let semantic_kind = result
        .tool_kind
        .as_deref()
        .map(AgentTuiToolKind::from_code)
        .unwrap_or_else(|| AgentTuiToolKind::from_tool_name(&tool_name));
    let output_preview = result.output_preview;
    let mut line = tool_result_line(&AgentTuiToolReceipt::with_id(
        result.part_id,
        result.tool_call_id,
        tool_name,
        AgentTuiToolReceiptDetails {
            kind: semantic_kind,
            status: wire_tool_result_status_code(result.status).to_owned(),
            summary: result.summary,
            metadata: result.metadata,
            error_type: result.error_type,
            filesystem_write_performed: result.filesystem_write_performed,
        },
    ));
    if let Some(output_preview) = output_preview
        && let Some(cell) = exec_cell_data_mut(&mut line)
        && cell.output_preview.is_none()
    {
        cell.output_preview = Some(output_preview);
    }
    stamp_exec_duration(&mut line, result.duration_ms);
    line
}

fn render_file_change(change: WireFileChangeEvent) -> Option<RenderCellRecord> {
    let mut changes = BTreeMap::new();
    changes.insert(PathBuf::from(change.path), change.change);
    let diff = AgentRenderDiffCell {
        id: change.part_id,
        title: "文件变更".to_owned(),
        changes,
        diagnostics: BTreeMap::new(),
        text_writes_applied: 1,
        renames_applied: 0,
        truncated: false,
        truncation_boundary: None,
    };
    diff.has_diff_content()
        .then(|| RenderCellRecord::file_change_with_data(diff))
}

fn render_plan_update(plan: WirePlanUpdate) -> RenderCellRecord {
    let steps = plan
        .steps
        .into_iter()
        .map(|step| AgentRenderPlanStep {
            id: None,
            status: plan_status_from_wire(&step.status),
            summary: step.step,
        })
        .collect::<Vec<_>>();
    let source = plan_source(&plan.title, &steps);
    RenderCellRecord::plan_with_data(AgentRenderPlanCell {
        id: plan.part_id,
        title: plan.title,
        explanation: None,
        steps,
        source,
        source_kind: AgentRenderPlanSourceKind::Markdown,
    })
}

fn render_review_status(review: WireReviewStatus) -> RenderCellRecord {
    let details = if review.details.is_empty() {
        review.detail.into_iter().collect::<Vec<_>>()
    } else {
        review.details
    };
    RenderCellRecord::review_with_data(AgentRenderReviewCell {
        id: review.part_id,
        subject: review.subject,
        status: Some(review_status_from_wire(review.status)),
        details: details
            .into_iter()
            .map(|text| AgentRenderReviewDetail {
                kind: AgentRenderReviewDetailKind::Message,
                text,
            })
            .collect(),
    })
}

fn render_governance_status(part_id: String, governance: WireGovernanceStatus) -> RenderCellRecord {
    RenderCellRecord::status_with_data(RenderStatusCell {
        id: prefixed_part_id("status", &part_id),
        title: "后台治理".to_owned(),
        items: vec![RenderStatusItem {
            kind: "governance.status".to_owned(),
            summary: lifecycle_status_label(governance.status).to_owned(),
            phase: Some(crate::AgentTuiLifecyclePhase::Notice),
            indicator: governance.reason,
            tool_name: Some(governance.job_id),
            duration_ms: None,
            turn_id: None,
            attempt_index: None,
        }],
    })
}

fn render_error(part_id: String, failure: &WireRuntimeError) -> RenderCellRecord {
    let event_id = failure.caused_by_event.as_deref().unwrap_or(&part_id);
    let run_error = agent_tui_run_error_from_wire(failure, event_id);
    RenderCellRecord::error_with_data(run_error_cell(&run_error))
}

fn prefixed_part_id(kind: &str, part_id: &str) -> String {
    if part_id.starts_with(kind) {
        part_id.to_owned()
    } else {
        format!("{kind}:{part_id}")
    }
}

fn usage_from_wire(usage: WireUsageUpdate) -> crate::AgentTuiTokenUsage {
    crate::AgentTuiTokenUsage::new(
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_write_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
    )
    .with_current_context_tokens(usage.current_context_tokens)
}

fn usage_cell_from_wire(id: String, usage: WireUsageUpdate) -> AgentRenderUsageCell {
    AgentRenderUsageCell::from_usage(id, usage_from_wire(usage))
}

fn render_turn_summary(summary: WireTurnSummary) -> RenderCellRecord {
    let cell = AgentRenderTurnSummaryCell::new(
        summary.part_id,
        summary.duration_ms,
        summary.usage.map(usage_from_wire),
    )
    .with_total_tokens(summary.total_tokens)
    .with_performance(summary.first_token_avg_ms, summary.generation_tps_x10);
    RenderCellRecord::turn_summary_with_data(cell)
}

fn wire_tool_result_status_code(status: WireLifecycleStatus) -> &'static str {
    match status {
        WireLifecycleStatus::Completed => "completed",
        WireLifecycleStatus::Failed => "failed",
        WireLifecycleStatus::Running | WireLifecycleStatus::Started => "running",
        WireLifecycleStatus::Waiting => "waiting",
        WireLifecycleStatus::Blocked => "blocked",
        WireLifecycleStatus::Cancelled | WireLifecycleStatus::Aborted => "stopped",
    }
}

fn plan_status_from_wire(status: &str) -> AgentRenderPlanStatus {
    match status {
        "completed" | "complete" => AgentRenderPlanStatus::Completed,
        "running" | "in_progress" => AgentRenderPlanStatus::Running,
        "failed" | "error" => AgentRenderPlanStatus::Failed,
        "skipped" => AgentRenderPlanStatus::Skipped,
        _ => AgentRenderPlanStatus::Pending,
    }
}

fn review_status_from_wire(status: WireLifecycleStatus) -> AgentRenderReviewStatus {
    match status {
        WireLifecycleStatus::Completed => AgentRenderReviewStatus::Completed,
        WireLifecycleStatus::Failed => AgentRenderReviewStatus::Failed,
        WireLifecycleStatus::Blocked => AgentRenderReviewStatus::Blocked,
        WireLifecycleStatus::Cancelled | WireLifecycleStatus::Aborted => {
            AgentRenderReviewStatus::Stopped
        }
        WireLifecycleStatus::Waiting => AgentRenderReviewStatus::Pending,
        WireLifecycleStatus::Running | WireLifecycleStatus::Started => {
            AgentRenderReviewStatus::Running
        }
    }
}

fn lifecycle_status_label(status: WireLifecycleStatus) -> &'static str {
    match status {
        WireLifecycleStatus::Started => "started",
        WireLifecycleStatus::Running => "running",
        WireLifecycleStatus::Waiting => "waiting",
        WireLifecycleStatus::Completed => "completed",
        WireLifecycleStatus::Failed => "failed",
        WireLifecycleStatus::Cancelled => "cancelled",
        WireLifecycleStatus::Aborted => "aborted",
        WireLifecycleStatus::Blocked => "blocked",
    }
}

fn plan_source(title: &str, steps: &[AgentRenderPlanStep]) -> String {
    let mut lines = vec![format!("# {}", title.trim())];
    if !steps.is_empty() {
        lines.push(String::new());
        lines.extend(steps.iter().map(|step| {
            format!(
                "- {} {}",
                plan_status_marker(step.status),
                step.summary.trim()
            )
        }));
    }
    lines.join("\n")
}

fn plan_status_marker(status: AgentRenderPlanStatus) -> &'static str {
    match status {
        AgentRenderPlanStatus::Pending => "[ ]",
        AgentRenderPlanStatus::Running => "[~]",
        AgentRenderPlanStatus::Completed => "[x]",
        AgentRenderPlanStatus::Failed => "[!]",
        AgentRenderPlanStatus::Skipped => "[-]",
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        r#loop::agent_tui::state::runtime_flow_test_support::test_state,
        surface::{
            cell::{RenderCellData, RenderCellKind},
            replay_snapshot::{RESUME_LOADING_TITLE, ReplaySnapshotPhase},
        },
    };
    use axon_protocol::wire::{WireTurnSummary, WireUsageUpdate};

    #[test]
    fn resume_replay_loading_cell_is_replaced_on_completion() {
        let mut state = test_state();

        state.begin_resume_replay();
        state.complete_resume_replay();

        assert_eq!(
            state.header.replay_snapshot.map(|state| state.phase),
            Some(ReplaySnapshotPhase::Completed)
        );
        assert_eq!(
            state
                .cells
                .transcript
                .iter()
                .filter(|cell| matches!(cell.mode, crate::surface::cell::RenderCellMode::Cell(kind) if kind == RenderCellKind::STATUS))
                .count(),
            0
        );
        assert!(
            !state
                .cells
                .transcript
                .iter()
                .any(|cell| cell.text == RESUME_LOADING_TITLE)
        );
    }

    #[test]
    fn resume_usage_cell_uses_token_unit_renderer() {
        let cell = super::usage_cell_from_wire(
            "usage:resume:test".to_owned(),
            WireUsageUpdate {
                input_tokens: 202_188,
                output_tokens: 1_353,
                cached_input_tokens: 113_502,
                cache_write_input_tokens: 0,
                reasoning_output_tokens: 3_381,
                current_context_tokens: 0,
                system_prompt_tokens: 0,
                tool_definition_tokens: 0,
                conversation_tokens: 0,
            },
        );

        assert_eq!(cell.total, "320K");
        assert_eq!(cell.input, "202K");
        assert_eq!(cell.cache_read, "114K");
        assert_eq!(cell.output, "1.35K");
        assert_eq!(cell.reasoning, "3.38K");
    }

    #[test]
    fn resume_turn_summary_reuses_complete_usage_projection() {
        let record = super::render_turn_summary(WireTurnSummary {
            part_id: "turn-summary:resume:test".to_owned(),
            turn_id: "turn:test".to_owned(),
            duration_ms: None,
            total_tokens: Some(3_750),
            usage: Some(WireUsageUpdate {
                input_tokens: 1_000,
                output_tokens: 400,
                cached_input_tokens: 2_000,
                cache_write_input_tokens: 300,
                reasoning_output_tokens: 50,
                current_context_tokens: 0,
                system_prompt_tokens: 0,
                tool_definition_tokens: 0,
                conversation_tokens: 0,
            }),
            first_token_avg_ms: Some(1_250),
            generation_tps_x10: Some(125),
        });

        let Some(RenderCellData::TurnSummary(cell)) = record.data else {
            panic!("resume 应恢复为 turn summary cell");
        };
        let usage = cell.usage.expect("turn summary 应保留完整 usage");
        assert_eq!(usage.total_tokens(), 3_750);
        assert_eq!(usage.cache_hit_rate_basis_points(), 6_060);
        assert_eq!(cell.first_token_avg_ms, Some(1_250));
        assert_eq!(cell.generation_tps_x10, Some(125));
    }
}
