//! Wire runtime event 到 TUI 状态的投射。

use std::{collections::BTreeMap, path::PathBuf, time::Instant};

use axon_domain::ToolExecutionPermission;
use axon_protocol::wire::{
    WireContextRollover, WireEventEnvelope, WireEventKind, WireFileChangeEvent,
    WireGovernanceStatus, WireLifecycleStatus, WireMemoryStatus, WireMemorySurfaceEntry,
    WirePlanUpdate, WireProviderStatus, WireReviewStatus, WireRuntimeError, WireSkillStatus,
    WireTokenBudgetReminder, WireToolCall, WireToolResult, WireTurnLifecycle, WireUsageUpdate,
};

use crate::r#loop::agent_tui::{
    display_units::{format_decimal_count, format_duration_millis_compact},
    run_output::{
        AgentTuiHistoryItem, AgentTuiHistoryLine, AgentTuiHookStatusView,
        AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput, AgentTuiPlanLine,
        AgentTuiPlanStep, AgentTuiReviewLine, AgentTuiTokenUsage, AgentTuiToolCompletionView,
        AgentTuiToolDiffSnapshotView, AgentTuiToolExecutionView, AgentTuiToolKind,
        AgentTuiToolOutputChunk, AgentTuiToolProgressView,
    },
};

use super::{
    AgentTuiState,
    runtime_error_projection::{log_wire_runtime_error, wire_runtime_error_output},
    subagent_activity::{SUBAGENT_ACTIVITY_KIND, extract_agent_path, subagent_label},
};

const DEFAULT_WIRE_TOOL_REQUEST_ID: u64 = 0;
const FINAL_RECEIPT_PROJECTION_KIND: &str = "final_receipt";
const EXECUTION_PERMISSION_ALLOWED: &str = "allowed";
const EXECUTION_PERMISSION_FORBIDDEN: &str = "forbidden";
const WIRE_FORBIDDEN_REASON: &str = "wire 事件标记本次工具调用为禁止执行";
const WIRE_UNKNOWN_PERMISSION_REASON: &str = "wire 事件包含未知的工具执行权限";
const MEMORY_RECALL_SUBJECT: &str = "recall";
const MEMORY_USEFULNESS_SUBJECT: &str = "usefulness";
const WIRE_RUNTIME_ERROR_SCOPE: &str = "wire_event";
const SUBAGENT_RESULT_KIND: &str = "SubagentResult";
const SUBAGENT_RESULT_TITLE_SUFFIX: &str = "子代理完成";
const SUBAGENT_RESULT_DURATION_LABEL: &str = "耗时";
const SUBAGENT_RESULT_TOKEN_LABEL: &str = "Token 消耗";
const SUBAGENT_RESULT_CACHE_HIT_RATE_LABEL: &str = "缓存命中率";
const SUBAGENT_RESULT_SUMMARY_LABEL: &str = "摘要";
const SUBAGENT_RESULT_FIELD_SEPARATOR: &str = "，";
const STREAMING_COMPLETED_KIND: &str = axon_protocol::wire::WIRE_LIFECYCLE_KIND_STREAMING_COMPLETED;
const CONTEXT_BUDGET_HOOK_STATUS: &str = "completed";
const TOKEN_BUDGET_HOOK_NAME: &str = "Token 预算";
const TOKEN_BUDGET_REMAINING_PREFIX: &str = "剩余 ";
const TOKEN_BUDGET_REMAINING_SUFFIX: &str = " token";
const CONTEXT_ROLLOVER_HOOK_NAME: &str = "上下文轮转";
const CONTEXT_ROLLOVER_WINDOW_PREFIX: &str = "窗口 #";
const CONTEXT_ROLLOVER_ARCHIVED_PREFIX: &str = "归档 ";
const CONTEXT_ROLLOVER_ARCHIVED_SUFFIX: &str = " 条消息";
const CONTEXT_ROLLOVER_NOTES_INJECTED: &str = "，已注入战术笔记";
const CONTEXT_ROLLOVER_NOTES_SKIPPED: &str = "，未注入战术笔记";

pub(super) fn dispatch_wire_event(state: &mut AgentTuiState, event: WireEventEnvelope) {
    let event_id = event.event_id.0.clone();
    let execution_id = event.session.run_id.clone();
    let lane_id = event.session.session_id.clone();
    match event.kind {
        WireEventKind::TurnLifecycle(lifecycle) => {
            dispatch_turn_lifecycle(state, lifecycle, event_id, execution_id)
        }
        WireEventKind::ProviderStatus(status) => dispatch_provider_status(state, status),
        WireEventKind::AssistantMessage(message) if message.delta => {
            state.apply_assistant_delta_batch(message.part_id, vec![message.content]);
        }
        WireEventKind::AssistantMessage(message) => {
            state.apply_assistant_message_snapshot(message.part_id, message.content);
        }
        WireEventKind::ReasoningStart(reasoning) => {
            state.start_reasoning_part(reasoning.part_id);
        }
        WireEventKind::ReasoningDelta(reasoning) => {
            state.apply_reasoning_delta_batch(reasoning.part_id, vec![reasoning.delta]);
        }
        WireEventKind::ReasoningEnd(reasoning) => {
            state.finish_reasoning_part(reasoning.part_id);
        }
        WireEventKind::ToolCall(call) => dispatch_tool_call(state, call),
        WireEventKind::ToolResult(result) => dispatch_tool_result(state, result),
        WireEventKind::FileChange(change) => dispatch_file_change(state, change, event_id),
        WireEventKind::PlanUpdate(plan) => dispatch_plan_update(state, plan, event_id),
        WireEventKind::ReviewStatus(review) => dispatch_review_status(state, review, event_id),
        WireEventKind::MemoryStatus(status) => dispatch_memory_status(state, status),
        WireEventKind::SkillStatus(status) => dispatch_skill_status(state, status),
        WireEventKind::VerifyStatus(status) => {
            state.apply_hook_status(AgentTuiHookStatusView::new(
                status.verifier,
                wire_status_name(status.status),
                status.convergence.or(Some(status.outcome)),
            ));
        }
        WireEventKind::GovernanceStatus(status) => dispatch_governance_status(state, status),
        WireEventKind::UsageUpdate(usage) => state.record_live_turn_usage(wire_usage(usage)),
        WireEventKind::ResumeSnapshot(snapshot) => state.apply_wire_resume_snapshot(*snapshot),
        WireEventKind::RuntimeError(error) => {
            dispatch_runtime_error(state, error, &event_id);
        }
        WireEventKind::UserMessage(message) => state.commit_pending_steer(message.content),
        WireEventKind::TokenBudgetReminder(reminder) => {
            dispatch_token_budget_reminder(state, reminder);
        }
        WireEventKind::ContextRollover(rollover) => dispatch_context_rollover(state, rollover),
        WireEventKind::UserQuestion(question) => {
            state.apply_user_question(&lane_id, question);
        }
    }
}

fn dispatch_provider_status(state: &mut AgentTuiState, status: WireProviderStatus) {
    state.bridge.provider_activity.push(status, Instant::now());
}

/// Token 预算预警必须进入可见事实：剩余额度与模型提示内容一并落到 history 投影。
fn dispatch_token_budget_reminder(state: &mut AgentTuiState, reminder: WireTokenBudgetReminder) {
    state.apply_hook_status(AgentTuiHookStatusView::with_run_id(
        format!("token-budget:{}", reminder.turn_id),
        TOKEN_BUDGET_HOOK_NAME,
        CONTEXT_BUDGET_HOOK_STATUS,
        Some(format!(
            "{TOKEN_BUDGET_REMAINING_PREFIX}{}{TOKEN_BUDGET_REMAINING_SUFFIX}：{}",
            format_decimal_count(reminder.tokens_remaining),
            reminder.reminder_message
        )),
    ));
}

/// 上下文轮转必须进入可见事实：窗口序号、归档规模与便签注入结果。
fn dispatch_context_rollover(state: &mut AgentTuiState, rollover: WireContextRollover) {
    let notes = if rollover.notes_injected {
        CONTEXT_ROLLOVER_NOTES_INJECTED
    } else {
        CONTEXT_ROLLOVER_NOTES_SKIPPED
    };
    state.apply_hook_status(AgentTuiHookStatusView::with_run_id(
        format!("context-rollover:{}", rollover.window_id),
        CONTEXT_ROLLOVER_HOOK_NAME,
        CONTEXT_BUDGET_HOOK_STATUS,
        Some(format!(
            "{CONTEXT_ROLLOVER_WINDOW_PREFIX}{}：{CONTEXT_ROLLOVER_ARCHIVED_PREFIX}{}{CONTEXT_ROLLOVER_ARCHIVED_SUFFIX}{notes}",
            rollover.window_number,
            format_decimal_count(u64::try_from(rollover.archived_messages_count).unwrap_or(u64::MAX))
        )),
    ));
}

fn dispatch_turn_lifecycle(
    state: &mut AgentTuiState,
    lifecycle: WireTurnLifecycle,
    _event_id: String,
    execution_id: Option<String>,
) {
    if lifecycle.kind.as_deref() == Some(STREAMING_COMPLETED_KIND) {
        // Provider 流结束即结束“生成回复中”这一瞬时活动，避免收口后状态行滞留；
        // 同时把这条流的用量快照并入本轮累计，下一条流才不会覆盖掉它。
        state.fold_live_turn_usage();
        state.clear_generation_lifecycle_status();
        return;
    }
    if lifecycle.kind.as_deref() == Some(SUBAGENT_ACTIVITY_KIND) {
        let terminal = matches!(
            lifecycle.status,
            WireLifecycleStatus::Completed
                | WireLifecycleStatus::Failed
                | WireLifecycleStatus::Cancelled
        );
        let result_view = terminal.then(|| subagent_result_view(&lifecycle));
        state.apply_subagent_activity_lifecycle(lifecycle, execution_id, Instant::now());
        if let Some(result_view) = result_view {
            state.apply_lifecycle_status(result_view);
        }
        return;
    }
    state.apply_lifecycle_status(AgentTuiLifecycleStatusView::with_id(
        AgentTuiLifecycleStatusViewInput {
            id: lifecycle.part_id,
            title: lifecycle.title,
            kind: lifecycle.kind.unwrap_or_else(|| lifecycle.turn_id.clone()),
            summary: lifecycle.summary.unwrap_or_default(),
            phase: lifecycle.ui_phase.unwrap_or_else(|| "unknown".to_owned()),
            tool_name: lifecycle.tool_name,
            duration_ms: lifecycle.duration_ms,
            attempt_index: lifecycle
                .attempt_index
                .and_then(|value| usize::try_from(value).ok()),
        },
    ));
}

fn dispatch_tool_call(state: &mut AgentTuiState, call: WireToolCall) {
    state.record_visible_turn_activity();
    state.record_turn_work_activity();
    state.apply_tool_execution_started(AgentTuiToolExecutionView {
        request_id: call.request_id.unwrap_or(DEFAULT_WIRE_TOOL_REQUEST_ID),
        part_id: call.part_id,
        provider_call_id: call.tool_call_id,
        tool_name: call.tool_name,
        kind: call.tool_kind.clone(),
        semantic_kind: AgentTuiToolKind::from_code(&call.tool_kind),
        arguments_preview: call.arguments_summary.unwrap_or_default(),
        intent: call.intent,
        execution_permission: call
            .execution_permission
            .as_deref()
            .map(execution_permission_from_wire)
            .unwrap_or(ToolExecutionPermission::Allowed),
    });
}

fn dispatch_tool_result(state: &mut AgentTuiState, result: WireToolResult) {
    let source_part_id = result.part_id.clone();
    if state.projection_is_settled(&source_part_id, FINAL_RECEIPT_PROJECTION_KIND) {
        return;
    }
    let projection_part_id = state
        .terminal_session_projection
        .projection_part_id(&source_part_id)
        .unwrap_or(&source_part_id)
        .to_owned();
    let tool_name = result
        .tool_name
        .clone()
        .unwrap_or_else(|| result.tool_call_id.clone());
    let tool_kind = result
        .tool_kind
        .clone()
        .unwrap_or_else(|| tool_name.clone());
    let semantic_kind = AgentTuiToolKind::from_code(&tool_kind);
    if let Some(stream) = result.stream.clone() {
        state.record_visible_turn_activity();
        state.record_turn_work_activity();
        let chunk = AgentTuiToolOutputChunk {
            part_id: projection_part_id,
            provider_call_id: result.tool_call_id,
            tool_name,
            semantic_kind,
            stream,
            sequence: result.sequence.unwrap_or_default(),
            text: result.output_preview.unwrap_or(result.summary),
        };
        state.append_tool_output_chunk(&chunk);
        state.merge_tool_output_chunk_cell(&chunk);
        return;
    }
    if let Some(phase) = result.progress_phase.clone() {
        state.record_visible_turn_activity();
        state.record_turn_work_activity();
        state.merge_tool_progress_cell(&AgentTuiToolProgressView {
            part_id: projection_part_id,
            provider_call_id: result.tool_call_id,
            tool_name,
            semantic_kind,
            phase,
            items_completed: result.items_completed,
            items_total: result.items_total,
            current_item: result.current_item,
            elapsed_ms: result.duration_ms,
            message: result.output_preview.or(Some(result.summary)),
        });
        return;
    }
    if !state.settle_projection_once(&source_part_id, FINAL_RECEIPT_PROJECTION_KIND) {
        return;
    }
    let status = wire_status_name(result.status);
    state.terminal_session_projection.observe_result(
        &source_part_id,
        &projection_part_id,
        &tool_name,
        status,
        result.metadata.as_ref(),
    );
    let detail = tool_result_detail(&result);
    state.record_visible_turn_activity();
    state.record_turn_work_activity();
    state.append_tool_result_detail_if_absent(&projection_part_id, &tool_name, detail.as_deref());
    state.apply_tool_execution_completed(AgentTuiToolCompletionView {
        part_id: projection_part_id,
        provider_call_id: result.tool_call_id,
        tool_name,
        kind: tool_kind,
        semantic_kind,
        status: status.to_owned(),
        summary: result.summary,
        metadata: result.metadata,
        error_type: result.error_type,
        filesystem_write_performed: result.filesystem_write_performed,
        duration_ms: result.duration_ms,
    });
}

fn tool_result_detail(result: &WireToolResult) -> Option<String> {
    result
        .output_preview
        .as_deref()
        .or_else(|| {
            result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("outputPreview"))
                .and_then(serde_json::Value::as_str)
        })
        .or(Some(result.summary.as_str()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn dispatch_file_change(state: &mut AgentTuiState, event: WireFileChangeEvent, _event_id: String) {
    let mut changes = BTreeMap::new();
    let tool_part_id = event.tool_part_id.clone();
    let provider_call_id = event.tool_call_id.unwrap_or_default();
    let diff_id = event.part_id;
    let path = PathBuf::from(event.path);
    let renames_applied = usize::from(matches!(
        &event.change,
        axon_protocol::diff::FileChange::Update {
            move_path: Some(_),
            ..
        }
    ));
    changes.insert(path, event.change);
    state.record_visible_turn_activity();
    state.record_turn_work_activity();
    state.apply_tool_diff_snapshot(AgentTuiToolDiffSnapshotView {
        id: diff_id,
        tool_part_id,
        provider_call_id,
        title: "文件变更".to_owned(),
        changes,
        text_writes_applied: 0,
        renames_applied,
    });
}

fn dispatch_runtime_error(state: &mut AgentTuiState, error: WireRuntimeError, event_id: &str) {
    log_wire_runtime_error(WIRE_RUNTIME_ERROR_SCOPE, &error, event_id);
    state.apply_runtime_error_output(wire_runtime_error_output(&error, event_id));
}

fn dispatch_plan_update(state: &mut AgentTuiState, plan: WirePlanUpdate, _event_id: String) {
    state.replace_active_plan_cell(AgentTuiPlanLine::with_id(
        plan.part_id,
        plan.title,
        plan.explanation,
        plan.steps
            .into_iter()
            .map(|step| {
                AgentTuiPlanStep::new(
                    step.status,
                    step.step,
                    step.cycle,
                    step.diagnostic_count,
                    step.failing_step_count,
                )
            })
            .collect(),
    ));
}

fn dispatch_review_status(state: &mut AgentTuiState, review: WireReviewStatus, _event_id: String) {
    let details = if review.details.is_empty() {
        review.detail.into_iter().collect()
    } else {
        review.details
    };
    state.upsert_active_review_cell(AgentTuiReviewLine::with_id(
        review.part_id,
        wire_status_name(review.status),
        review.subject,
        details,
    ));
}

fn dispatch_memory_status(state: &mut AgentTuiState, status: WireMemoryStatus) {
    if status.subject == MEMORY_USEFULNESS_SUBJECT {
        return;
    }
    if status.subject == MEMORY_RECALL_SUBJECT {
        let visible_memories = status
            .surfaced_memories
            .iter()
            .filter(|memory| memory.is_user_visible())
            .collect::<Vec<_>>();
        if !visible_memories.is_empty() {
            state.upsert_history_cell(memory_recall_history_line(&status, &visible_memories));
        }
        return;
    }
    state.apply_hook_status(AgentTuiHookStatusView::new(
        "记忆",
        wire_status_name(status.status),
        status.summary.or(Some(status.outcome)),
    ));
}

fn memory_recall_history_line(
    status: &WireMemoryStatus,
    visible_memories: &[&WireMemorySurfaceEntry],
) -> AgentTuiHistoryLine {
    let items = visible_memories
        .iter()
        .map(|memory| {
            AgentTuiHistoryItem::new(
                memory.title.clone(),
                Some("记忆".to_owned()),
                Some(memory.summary.clone()),
            )
        })
        .collect::<Vec<_>>();
    AgentTuiHistoryLine::with_id(
        format!("history:memory:recall:{}", status.subject),
        format!("召回记忆 {} 条", items.len()),
        items,
    )
}

fn dispatch_skill_status(state: &mut AgentTuiState, status: WireSkillStatus) {
    state.apply_hook_status(AgentTuiHookStatusView::new(
        "Skill",
        wire_status_name(status.status),
        status.summary.or(Some(status.outcome)),
    ));
}

fn dispatch_governance_status(state: &mut AgentTuiState, status: WireGovernanceStatus) {
    state.apply_hook_status(AgentTuiHookStatusView::new(
        status.job_id,
        wire_status_name(status.status),
        status.reason.or(status.status_path),
    ));
}

fn wire_usage(usage: WireUsageUpdate) -> AgentTuiTokenUsage {
    AgentTuiTokenUsage::new(
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_write_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
    )
    .with_current_context_tokens(usage.current_context_tokens)
}

fn wire_status_name(status: WireLifecycleStatus) -> &'static str {
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

fn execution_permission_from_wire(value: &str) -> ToolExecutionPermission {
    match value.trim().to_ascii_lowercase().as_str() {
        EXECUTION_PERMISSION_ALLOWED => ToolExecutionPermission::Allowed,
        EXECUTION_PERMISSION_FORBIDDEN => ToolExecutionPermission::Forbidden {
            reason: WIRE_FORBIDDEN_REASON.to_owned(),
        },
        _ => ToolExecutionPermission::Forbidden {
            reason: WIRE_UNKNOWN_PERMISSION_REASON.to_owned(),
        },
    }
}

fn subagent_result_view(lifecycle: &WireTurnLifecycle) -> AgentTuiLifecycleStatusView {
    let source_summary = lifecycle.summary.as_deref().unwrap_or_default();
    let (label, _) = subagent_label(&lifecycle.title, Some(source_summary));
    AgentTuiLifecycleStatusView::with_id(AgentTuiLifecycleStatusViewInput {
        id: format!("subagent-result:{}", lifecycle.part_id),
        title: format!("{label}{SUBAGENT_RESULT_TITLE_SUFFIX}"),
        kind: SUBAGENT_RESULT_KIND.to_owned(),
        summary: subagent_result_details(
            source_summary,
            lifecycle.duration_ms,
            lifecycle.total_tokens,
            lifecycle.cache_hit_rate_basis_points,
        ),
        phase: "command".to_owned(),
        tool_name: lifecycle.tool_name.clone(),
        duration_ms: lifecycle.duration_ms,
        attempt_index: lifecycle
            .attempt_index
            .and_then(|value| usize::try_from(value).ok()),
    })
}

fn subagent_result_details(
    source_summary: &str,
    duration_ms: Option<u64>,
    total_tokens: Option<u64>,
    cache_hit_rate_basis_points: Option<u16>,
) -> String {
    let mut fields = Vec::new();
    if let Some(duration_ms) = duration_ms {
        fields.push(format!(
            "{SUBAGENT_RESULT_DURATION_LABEL}：{}",
            format_duration_millis_compact(duration_ms)
        ));
    }
    if let Some(total_tokens) = total_tokens.filter(|value| *value > 0) {
        fields.push(format!(
            "{SUBAGENT_RESULT_TOKEN_LABEL}：{}",
            format_decimal_count(total_tokens)
        ));
    }
    if let Some(value) = cache_hit_rate_basis_points {
        fields.push(format!(
            "{SUBAGENT_RESULT_CACHE_HIT_RATE_LABEL}：{}",
            axon_display::format_basis_points_percent(value)
        ));
    }
    let summary = subagent_result_summary(source_summary);
    if !summary.is_empty() {
        fields.push(format!("{SUBAGENT_RESULT_SUMMARY_LABEL}：{summary}"));
    }
    fields.join(SUBAGENT_RESULT_FIELD_SEPARATOR)
}

fn subagent_result_summary(source_summary: &str) -> &str {
    let source_summary = source_summary.trim();
    let summary = extract_agent_path(source_summary)
        .and_then(|path| source_summary.strip_prefix(path))
        .map_or(source_summary, str::trim_start);
    summary
        .split_once('：')
        .map_or(summary, |(_, detail)| detail.trim())
}
