use axon_agent_runtime::{
    AgentRunExecutionError, AgentRunReasoningMessage, AgentRunResponse,
    MemoryAutonomyFeedbackReport, TokenUsageTelemetry,
};
use axon_domain::RuntimeFailure;
use axon_protocol::wire::{
    WireAssistantMessage, WireCorrelationId, WireEventEnvelope, WireEventKind, WireFileChangeEvent,
    WireLifecyclePhase, WireLifecycleStatus, WireMemoryStatus, WireReasoningDelta,
    WireReasoningEnd, WireReasoningStart, WireSchemaVersion, WireSessionRef, WireSkillStatus,
    WireToolCall, WireToolResult, WireTurnLifecycle, WireUsageUpdate,
};
use serde::Serialize;

use super::{
    RunRequest, run_file_change_projection::project_tool_file_changes,
    run_memory_surface::surfaced_memory_entries, run_memory_usefulness::RunMemoryUsefulnessEvent,
    run_plan_projection::plan_update_from_tool_receipt,
    run_post_run_governance::PostRunGovernanceReceipt, run_skill_evolution::SkillEvolutionEvent,
    run_tool_loop::RunToolReceipt,
};

const RUN_LIFECYCLE_TURN_ID: &str = "run";
const ASSISTANT_CORRELATION_ID: &str = "assistant";
const ASSISTANT_FINAL_MESSAGE_ID: &str = "assistant-message-final";
const TOOL_KIND_METADATA_KEY: &str = "kind";
const TOOL_ARGUMENTS_PREVIEW_METADATA_KEY: &str = "argumentsPreview";
const TOOL_OUTPUT_PREVIEW_METADATA_KEY: &str = "outputPreview";

#[derive(Clone, Copy)]
pub(super) struct CompletedRunWireEventsInput<'a> {
    pub(super) request: &'a RunRequest,
    pub(super) response: &'a AgentRunResponse<RunToolReceipt>,
    pub(super) session_title: Option<&'a str>,
    pub(super) memory_usefulness: Option<&'a RunMemoryUsefulnessEvent>,
    pub(super) memory_autonomy: Option<&'a MemoryAutonomyFeedbackReport>,
    pub(super) skill_evolution: Option<&'a SkillEvolutionEvent>,
    pub(super) post_run_governance: Option<&'a PostRunGovernanceReceipt>,
    pub(super) recorded_at_ms: i64,
    pub(super) total_elapsed_ms: u64,
}

pub(super) fn completed_run_wire_events(
    input: &CompletedRunWireEventsInput<'_>,
) -> Vec<WireEventEnvelope> {
    let mut builder = RunWireEventBuilder::new(input.request, input.recorded_at_ms);
    builder.push_memory_recall(input.request);
    builder.push_memory_usefulness(input.memory_usefulness);
    builder.push_memory_autonomy(input.memory_autonomy);
    builder.push_skill_evolution(input.skill_evolution);
    builder.push_reasoning_messages(&input.response.reasoning_messages);
    builder.push_tool_receipts(&input.response.tool_receipts);
    builder.push_usage(input.response.token_usage);
    if let Some(receipt) = input.post_run_governance {
        builder.push_governance(receipt);
    }
    builder.push_assistant_message(&input.response.assistant_part_id, &input.response.content);
    builder.push_run_lifecycle(WireLifecycleStatus::Completed, "Run completed", None);
    builder.into_events()
}

pub(super) fn failed_run_wire_events(
    request: &RunRequest,
    error: &AgentRunExecutionError<RunToolReceipt>,
    failure: &RuntimeFailure,
    memory_usefulness: Option<&RunMemoryUsefulnessEvent>,
    memory_autonomy: Option<&MemoryAutonomyFeedbackReport>,
    recorded_at_ms: i64,
) -> Vec<WireEventEnvelope> {
    let mut builder = RunWireEventBuilder::new(request, recorded_at_ms);
    builder.push_memory_recall(request);
    builder.push_memory_usefulness(memory_usefulness);
    builder.push_memory_autonomy(memory_autonomy);
    builder.push_tool_receipts(&error.tool_receipts);
    builder.push_usage(error.token_usage);
    builder.push_runtime_error(failure);
    builder.push_run_lifecycle(
        WireLifecycleStatus::Failed,
        "Run failed",
        Some(error.message.clone()),
    );
    builder.into_events()
}

pub(super) fn wire_session_ref(request: &RunRequest) -> WireSessionRef {
    WireSessionRef {
        session_id: request.session_id.clone(),
        run_id: None,
        parent_session_id: request.parent_session_id.clone(),
    }
}

fn wire_usage_from_telemetry(usage: TokenUsageTelemetry) -> WireUsageUpdate {
    WireUsageUpdate {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        current_context_tokens: usage.current_context_tokens,
        system_prompt_tokens: usage.system_prompt_tokens,
        tool_definition_tokens: usage.tool_definition_tokens,
        conversation_tokens: usage.conversation_tokens,
    }
}

fn receipt_status_to_wire(status: &str) -> WireLifecycleStatus {
    match status.trim().to_ascii_lowercase().as_str() {
        "completed" => WireLifecycleStatus::Completed,
        "blocked" => WireLifecycleStatus::Blocked,
        "cancelled" | "canceled" => WireLifecycleStatus::Cancelled,
        "aborted" => WireLifecycleStatus::Aborted,
        _ => WireLifecycleStatus::Failed,
    }
}

fn metadata_text(metadata: Option<&serde_json::Value>, key: &str) -> Option<String> {
    metadata?
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn tool_kind(receipt: &RunToolReceipt) -> String {
    metadata_text(receipt.metadata.as_ref(), TOOL_KIND_METADATA_KEY)
        .unwrap_or_else(|| receipt.tool_name.clone())
}

fn metadata_value<T: Serialize>(value: &T) -> Option<serde_json::Value> {
    serde_json::to_value(value).ok()
}

fn metadata_string(metadata: Option<&serde_json::Value>, key: &str) -> Option<String> {
    metadata?
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn metadata_u64(metadata: Option<&serde_json::Value>, key: &str) -> u64 {
    metadata
        .and_then(|value| value.get(key))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default()
}

fn metadata_refs(metadata: Option<&serde_json::Value>, keys: &[&str]) -> Vec<String> {
    let mut refs = Vec::new();
    let Some(metadata) = metadata else {
        return refs;
    };
    for key in keys {
        if let Some(values) = metadata.get(*key).and_then(serde_json::Value::as_array) {
            refs.extend(
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned),
            );
        }
    }
    refs.sort();
    refs.dedup();
    refs
}

fn lifecycle_from_outcome(outcome: &str) -> WireLifecycleStatus {
    match outcome.trim().to_ascii_lowercase().as_str() {
        "failed" | "error" | "verify_failed" => WireLifecycleStatus::Failed,
        "blocked" | "stopped" | "verify_stopped" => WireLifecycleStatus::Blocked,
        "cancelled" | "canceled" | "skipped" => WireLifecycleStatus::Cancelled,
        _ => WireLifecycleStatus::Completed,
    }
}

struct RunWireEventBuilder {
    session: WireSessionRef,
    recorded_at_ms: i64,
    next_sequence: u64,
    events: Vec<WireEventEnvelope>,
}

impl RunWireEventBuilder {
    fn new(request: &RunRequest, recorded_at_ms: i64) -> Self {
        Self {
            session: wire_session_ref(request),
            recorded_at_ms,
            next_sequence: 1,
            events: Vec::new(),
        }
    }

    fn into_events(self) -> Vec<WireEventEnvelope> {
        self.events
    }

    fn push(&mut self, event_id: String, correlation_id: String, kind: WireEventKind) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.events.push(WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(event_id),
            correlation_id: WireCorrelationId(correlation_id),
            session: self.session.clone(),
            sequence,
            recorded_at_ms: self.recorded_at_ms,
            kind,
        });
    }

    fn push_run_lifecycle(
        &mut self,
        status: WireLifecycleStatus,
        title: &'static str,
        summary: Option<String>,
    ) {
        self.push(
            format!(
                "{}:{}:lifecycle",
                self.session.session_id, RUN_LIFECYCLE_TURN_ID
            ),
            self.session.session_id.clone(),
            WireEventKind::TurnLifecycle(WireTurnLifecycle {
                part_id: format!(
                    "lifecycle:{}:{}",
                    self.session.session_id, RUN_LIFECYCLE_TURN_ID
                ),
                turn_id: RUN_LIFECYCLE_TURN_ID.to_owned(),
                phase: WireLifecyclePhase::RegularTurn,
                status,
                title: title.to_owned(),
                kind: Some("RunLifecycle".to_owned()),
                ui_phase: None,
                tool_name: None,
                attempt_index: None,
                summary,
                duration_ms: None,
                started_at_ms: None,
                total_tokens: None,
                cache_hit_rate_basis_points: None,
            }),
        );
    }

    fn push_tool_receipts(&mut self, receipts: &[RunToolReceipt]) {
        for receipt in receipts {
            let tool_call_id = receipt.provider_call_id.clone();
            let part_id = receipt.part_id.clone();
            let arguments_preview = metadata_text(
                receipt.metadata.as_ref(),
                TOOL_ARGUMENTS_PREVIEW_METADATA_KEY,
            );
            let call_event_id = format!("{}:tool:{}:call", self.session.session_id, tool_call_id);
            let result_event_id =
                format!("{}:tool:{}:result", self.session.session_id, tool_call_id);
            self.push(
                call_event_id,
                tool_call_id.clone(),
                WireEventKind::ToolCall(WireToolCall {
                    part_id: part_id.clone(),
                    tool_call_id: tool_call_id.clone(),
                    request_id: None,
                    tool_name: receipt.tool_name.clone(),
                    tool_kind: tool_kind(receipt),
                    summary: receipt.summary.clone(),
                    arguments_summary: arguments_preview.clone(),
                    // 这条投影由收尾后的回执重建，没有实时调用事件里的 intent；
                    // 从同一条参数预览反解，解得出就照样上屏，预览被截断时才回落。
                    intent: axon_agent_runtime::tool_call_intent(
                        &receipt.tool_name,
                        arguments_preview.as_deref().unwrap_or_default(),
                    ),
                    execution_permission: None,
                }),
            );
            self.push(
                result_event_id,
                tool_call_id.clone(),
                WireEventKind::ToolResult(WireToolResult {
                    part_id,
                    tool_call_id,
                    tool_name: Some(receipt.tool_name.clone()),
                    tool_kind: Some(tool_kind(receipt)),
                    status: receipt_status_to_wire(&receipt.status),
                    summary: receipt.summary.clone(),
                    output_preview: metadata_text(
                        receipt.metadata.as_ref(),
                        TOOL_OUTPUT_PREVIEW_METADATA_KEY,
                    ),
                    stream: None,
                    sequence: None,
                    progress_phase: None,
                    items_completed: None,
                    items_total: None,
                    current_item: None,
                    duration_ms: receipt.duration_ms,
                    metadata: receipt.metadata.clone(),
                    error_type: receipt.error_type.clone(),
                    filesystem_write_performed: receipt.filesystem_write_performed,
                }),
            );
            if let Some(plan) = plan_update_from_tool_receipt(
                &self.session.session_id,
                &receipt.status,
                receipt.metadata.as_ref(),
            ) {
                self.push(
                    format!(
                        "{}:plan:{}",
                        self.session.session_id, receipt.provider_call_id
                    ),
                    receipt.provider_call_id.clone(),
                    WireEventKind::PlanUpdate(plan),
                );
            }
            for change in project_tool_file_changes(&receipt.part_id, receipt.metadata.as_ref()) {
                self.push(
                    format!(
                        "{}:tool:{}:file:{}",
                        self.session.session_id, receipt.provider_call_id, change.path
                    ),
                    receipt.provider_call_id.clone(),
                    WireEventKind::FileChange(WireFileChangeEvent {
                        part_id: change.part_id,
                        tool_part_id: Some(receipt.part_id.clone()),
                        tool_call_id: Some(receipt.provider_call_id.clone()),
                        path: change.path,
                        change: change.change,
                    }),
                );
            }
        }
    }

    fn push_usage(&mut self, usage: Option<TokenUsageTelemetry>) {
        if let Some(usage) = usage {
            self.push(
                format!("{}:usage", self.session.session_id),
                self.session.session_id.clone(),
                WireEventKind::UsageUpdate(wire_usage_from_telemetry(usage)),
            );
        }
    }

    fn push_reasoning_messages(&mut self, messages: &[AgentRunReasoningMessage]) {
        for message in messages {
            if message.content.trim().is_empty() {
                continue;
            }
            let part_id = message.part_id.clone();
            self.push(
                format!(
                    "{}:reasoning:{}:start",
                    self.session.session_id, message.reasoning_id
                ),
                part_id.clone(),
                WireEventKind::ReasoningStart(WireReasoningStart {
                    part_id: part_id.clone(),
                    reasoning_id: message.reasoning_id.clone(),
                }),
            );
            self.push(
                format!(
                    "{}:reasoning:{}:delta",
                    self.session.session_id, message.reasoning_id
                ),
                part_id.clone(),
                WireEventKind::ReasoningDelta(WireReasoningDelta {
                    part_id: part_id.clone(),
                    reasoning_id: message.reasoning_id.clone(),
                    delta: message.content.clone(),
                }),
            );
            self.push(
                format!(
                    "{}:reasoning:{}:end",
                    self.session.session_id, message.reasoning_id
                ),
                part_id.clone(),
                WireEventKind::ReasoningEnd(WireReasoningEnd {
                    part_id,
                    reasoning_id: message.reasoning_id.clone(),
                }),
            );
        }
    }

    /// 提交 canonical 最终 assistant 事件。
    ///
    /// 空正文同样必须提交：整轮的顺序契约是
    /// `canonical assistant -> foreground/run completed -> turn_completed`，
    /// “成功但无正文”是成功结果加告警，不是缺少 canonical assistant 的失败。
    fn push_assistant_message(&mut self, assistant_part_id: &str, content: &str) {
        let part_id = assistant_part_id.trim();
        if part_id.is_empty() {
            return;
        }
        self.push(
            format!(
                "{}:{}:{}",
                self.session.session_id, ASSISTANT_CORRELATION_ID, part_id
            ),
            ASSISTANT_CORRELATION_ID.to_owned(),
            WireEventKind::AssistantMessage(WireAssistantMessage {
                part_id: part_id.to_owned(),
                message_id: ASSISTANT_FINAL_MESSAGE_ID.to_owned(),
                content: content.to_owned(),
                delta: false,
            }),
        );
    }

    fn push_memory_recall(&mut self, request: &RunRequest) {
        let Some(recall) = request.memory_recall.as_ref() else {
            return;
        };
        let metadata = metadata_value(recall);
        let outcome =
            metadata_string(metadata.as_ref(), "status").unwrap_or_else(|| "unknown".to_owned());
        let surfaced_memories = surfaced_memory_entries(metadata.as_ref());
        self.push(
            format!("{}:memory:recall", self.session.session_id),
            self.session.session_id.clone(),
            WireEventKind::MemoryStatus(WireMemoryStatus {
                subject: "recall".to_owned(),
                status: lifecycle_from_outcome(&outcome),
                outcome,
                summary: Some(format!(
                    "Memory recall hit count {}",
                    metadata_u64(metadata.as_ref(), "hitCount")
                )),
                refs: metadata_refs(metadata.as_ref(), &["hitRefs"]),
                surfaced_memories,
                metadata,
            }),
        );
    }

    fn push_memory_usefulness(&mut self, event: Option<&RunMemoryUsefulnessEvent>) {
        let Some(event) = event else {
            return;
        };
        let metadata = metadata_value(event);
        self.push(
            format!("{}:memory:usefulness", self.session.session_id),
            self.session.session_id.clone(),
            WireEventKind::MemoryStatus(WireMemoryStatus {
                subject: "usefulness".to_owned(),
                status: WireLifecycleStatus::Completed,
                outcome: "evaluated".to_owned(),
                summary: Some(format!(
                    "Memory cited {} adopted {}",
                    metadata_u64(metadata.as_ref(), "citedHitCount"),
                    metadata_u64(metadata.as_ref(), "adoptedHitCount")
                )),
                refs: metadata_refs(metadata.as_ref(), &["citedHitRefs", "adoptedHitRefs"]),
                surfaced_memories: Vec::new(),
                metadata,
            }),
        );
    }

    fn push_memory_autonomy(&mut self, event: Option<&MemoryAutonomyFeedbackReport>) {
        let Some(event) = event else {
            return;
        };
        let metadata = metadata_value(event);
        let outcome = metadata_string(metadata.as_ref(), "status")
            .or_else(|| metadata_string(metadata.as_ref(), "decision"))
            .unwrap_or_else(|| "evaluated".to_owned());
        self.push(
            format!("{}:memory:autonomy", self.session.session_id),
            self.session.session_id.clone(),
            WireEventKind::MemoryStatus(WireMemoryStatus {
                subject: "autonomy".to_owned(),
                status: lifecycle_from_outcome(&outcome),
                outcome,
                summary: metadata_string(metadata.as_ref(), "reason"),
                refs: metadata_refs(metadata.as_ref(), &["refs", "memoryRefs"]),
                surfaced_memories: Vec::new(),
                metadata,
            }),
        );
    }

    fn push_skill_evolution(&mut self, event: Option<&SkillEvolutionEvent>) {
        let Some(event) = event else {
            return;
        };
        let metadata = metadata_value(event);
        let outcome =
            metadata_string(metadata.as_ref(), "status").unwrap_or_else(|| "unknown".to_owned());
        let skill_name =
            metadata_string(metadata.as_ref(), "skillName").unwrap_or_else(|| "unknown".to_owned());
        self.push(
            format!("{}:skill:{}", self.session.session_id, skill_name),
            self.session.session_id.clone(),
            WireEventKind::SkillStatus(WireSkillStatus {
                skill_name,
                status: lifecycle_from_outcome(&outcome),
                outcome,
                risk: metadata_string(metadata.as_ref(), "risk"),
                summary: metadata_string(metadata.as_ref(), "summary"),
                material_ref: metadata_string(metadata.as_ref(), "materialRef"),
                metadata,
            }),
        );
    }

    fn push_runtime_error(&mut self, failure: &RuntimeFailure) {
        self.push(
            format!("{}:runtime-error", self.session.session_id),
            self.session.session_id.clone(),
            WireEventKind::RuntimeError(failure.clone()),
        );
    }

    fn push_governance(&mut self, receipt: &PostRunGovernanceReceipt) {
        let event = receipt.to_wire_event(
            self.session.clone(),
            self.next_sequence,
            self.recorded_at_ms,
        );
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.events.push(event);
    }
}

#[cfg(test)]
#[path = "run_json_wire_plan_tests.rs"]
mod plan_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_builder_emits_reasoning_before_final_assistant_part() {
        let mut builder = RunWireEventBuilder {
            session: WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            recorded_at_ms: 1,
            next_sequence: 1,
            events: Vec::new(),
        };

        builder.push_reasoning_messages(&[AgentRunReasoningMessage {
            part_id: "reasoning-part:1".to_owned(),
            reasoning_id: "provider-reasoning:1".to_owned(),
            content: "先分析".to_owned(),
        }]);
        builder.push_assistant_message("assistant-part:1", "最终回答");

        let events = builder.into_events();
        assert!(matches!(
            &events[0].kind,
            WireEventKind::ReasoningStart(reasoning)
                if reasoning.part_id == "reasoning-part:1"
                    && reasoning.reasoning_id == "provider-reasoning:1"
        ));
        assert!(matches!(
            &events[1].kind,
            WireEventKind::ReasoningDelta(reasoning)
                if reasoning.part_id == "reasoning-part:1"
                    && reasoning.reasoning_id == "provider-reasoning:1"
                    && reasoning.delta == "先分析"
        ));
        assert!(matches!(
            &events[2].kind,
            WireEventKind::ReasoningEnd(reasoning)
                if reasoning.part_id == "reasoning-part:1"
                    && reasoning.reasoning_id == "provider-reasoning:1"
        ));
        assert!(matches!(
            &events[3].kind,
            WireEventKind::AssistantMessage(message)
                if message.part_id == "assistant-part:1"
                    && message.message_id == ASSISTANT_FINAL_MESSAGE_ID
                    && message.content == "最终回答"
                    && !message.delta
        ));
    }

    #[test]
    fn wire_builder_emits_content_before_terminal_lifecycle() {
        let mut builder = RunWireEventBuilder {
            session: WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            recorded_at_ms: 1,
            next_sequence: 1,
            events: Vec::new(),
        };

        builder.push_assistant_message("assistant-part:1", "最终回答");
        builder.push_run_lifecycle(WireLifecycleStatus::Completed, "Run completed", None);

        let events = builder.into_events();
        assert!(matches!(
            &events[0].kind,
            WireEventKind::AssistantMessage(message)
                if message.part_id == "assistant-part:1"
                    && message.message_id == ASSISTANT_FINAL_MESSAGE_ID
                    && message.content == "最终回答"
                    && !message.delta
        ));
        assert!(matches!(
            &events[1].kind,
            WireEventKind::TurnLifecycle(lifecycle)
                if lifecycle.status == WireLifecycleStatus::Completed
                    && lifecycle.kind.as_deref() == Some("RunLifecycle")
        ));
    }

    #[test]
    fn wire_builder_emits_empty_canonical_assistant_before_terminal_lifecycle() {
        let mut builder = RunWireEventBuilder {
            session: WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            recorded_at_ms: 1,
            next_sequence: 1,
            events: Vec::new(),
        };

        builder.push_assistant_message("assistant-part:1", "   ");
        builder.push_run_lifecycle(WireLifecycleStatus::Completed, "Run completed", None);

        let events = builder.into_events();
        assert_eq!(events.len(), 2, "空正文回合仍必须产出 canonical assistant");
        assert!(matches!(
            &events[0].kind,
            WireEventKind::AssistantMessage(message)
                if message.part_id == "assistant-part:1"
                    && message.content.trim().is_empty()
                    && !message.delta
        ));
        assert!(matches!(
            &events[1].kind,
            WireEventKind::TurnLifecycle(lifecycle)
                if lifecycle.status == WireLifecycleStatus::Completed
                    && lifecycle.kind.as_deref() == Some("RunLifecycle")
        ));
    }

    #[test]
    fn wire_builder_uses_receipt_part_id_for_tool_events() {
        let mut builder = RunWireEventBuilder {
            session: WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            recorded_at_ms: 1,
            next_sequence: 1,
            events: Vec::new(),
        };

        builder.push_tool_receipts(&[RunToolReceipt {
            tool_call_id: 7,
            part_id: "tool-part:receipt-stable".to_owned(),
            provider_call_id: "provider-call-7".to_owned(),
            tool_name: "builtin.search".to_owned(),
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: None,
        }]);

        let events = builder.into_events();
        assert!(matches!(
            &events[0].kind,
            WireEventKind::ToolCall(call)
                if call.part_id == "tool-part:receipt-stable"
                    && call.tool_call_id == "provider-call-7"
        ));
        assert!(matches!(
            &events[1].kind,
            WireEventKind::ToolResult(result)
                if result.part_id == "tool-part:receipt-stable"
                    && result.tool_call_id == "provider-call-7"
        ));
    }
}
