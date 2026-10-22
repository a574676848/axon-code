use std::sync::atomic::{AtomicU64, Ordering};

use axon_protocol::{
    diff::FileChange,
    wire::{
        WireCorrelationId, WireEventEnvelope, WireEventKind, WireFileChangeEvent,
        WireLifecyclePhase, WireLifecycleStatus, WireReasoningDelta, WireReasoningEnd,
        WireReasoningStart, WireSchemaVersion, WireSessionRef, WireSkillStatus, WireToolCall,
        WireToolResult, WireTurnLifecycle, WireUsageUpdate,
    },
};

use super::run_output::{
    AgentTuiHookStatus, AgentTuiHookStatusView, AgentTuiLifecycleStatusView,
    AgentTuiLifecycleStatusViewInput, AgentTuiRuntimeEvent, AgentTuiRuntimePayload,
    AgentTuiTokenUsage, AgentTuiToolCompletionView, AgentTuiToolDiffSnapshotView,
    AgentTuiToolExecutionView, AgentTuiToolOutputChunk, AgentTuiToolProgressView,
    runtime_driver_event,
};

static WIRE_TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn wire_event(kind: WireEventKind) -> AgentTuiRuntimeEvent {
    let sequence = WIRE_TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    runtime_driver_event(AgentTuiRuntimePayload::WireEvent {
        event: Box::new(WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("test-event-{sequence}")),
            correlation_id: WireCorrelationId(format!("test-correlation-{sequence}")),
            session: WireSessionRef {
                session_id: "test-session".to_owned(),
                run_id: Some("test-run".to_owned()),
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap_or(i64::MAX),
            kind,
        }),
    })
}

pub(crate) fn assistant_delta_event(delta: impl Into<String>) -> AgentTuiRuntimeEvent {
    assistant_delta_event_with_id("assistant:default", "provider-message:default", delta)
}

pub(crate) fn assistant_delta_event_with_id(
    part_id: impl Into<String>,
    message_id: impl Into<String>,
    delta: impl Into<String>,
) -> AgentTuiRuntimeEvent {
    assistant_message_event_with_id(part_id, message_id, delta, true)
}

pub(crate) fn assistant_message_event_with_id(
    part_id: impl Into<String>,
    message_id: impl Into<String>,
    content: impl Into<String>,
    delta: bool,
) -> AgentTuiRuntimeEvent {
    let part_id = part_id.into();
    wire_event(WireEventKind::AssistantMessage(
        axon_protocol::wire::WireAssistantMessage {
            part_id: part_id.clone(),
            message_id: message_id.into(),
            content: content.into(),
            delta,
        },
    ))
}

pub(crate) fn reasoning_delta_event(
    part_id: impl Into<String>,
    reasoning_id: impl Into<String>,
    delta: impl Into<String>,
) -> AgentTuiRuntimeEvent {
    let part_id = part_id.into();
    let reasoning_id = reasoning_id.into();
    wire_event(WireEventKind::ReasoningDelta(WireReasoningDelta {
        part_id,
        reasoning_id,
        delta: delta.into(),
    }))
}

pub(crate) fn reasoning_start_event(
    part_id: impl Into<String>,
    reasoning_id: impl Into<String>,
) -> AgentTuiRuntimeEvent {
    let part_id = part_id.into();
    let reasoning_id = reasoning_id.into();
    wire_event(WireEventKind::ReasoningStart(WireReasoningStart {
        part_id,
        reasoning_id,
    }))
}

pub(crate) fn reasoning_end_event(
    part_id: impl Into<String>,
    reasoning_id: impl Into<String>,
) -> AgentTuiRuntimeEvent {
    let part_id = part_id.into();
    let reasoning_id = reasoning_id.into();
    wire_event(WireEventKind::ReasoningEnd(WireReasoningEnd {
        part_id,
        reasoning_id,
    }))
}

pub(crate) fn lifecycle_status_event(view: AgentTuiLifecycleStatusView) -> AgentTuiRuntimeEvent {
    wire_event(WireEventKind::TurnLifecycle(WireTurnLifecycle {
        part_id: view.id.clone(),
        turn_id: view.kind.clone(),
        phase: WireLifecyclePhase::RegularTurn,
        status: WireLifecycleStatus::Running,
        title: view.title,
        kind: Some(view.kind),
        ui_phase: Some(lifecycle_phase_name(view.phase).to_owned()),
        tool_name: view.tool_name,
        attempt_index: view
            .attempt_index
            .and_then(|value| u64::try_from(value).ok()),
        summary: Some(view.summary),
        duration_ms: view.duration_ms,
        started_at_ms: None,
        total_tokens: None,
        cache_hit_rate_basis_points: None,
    }))
}

pub(crate) fn streaming_started_event(
    provider: impl AsRef<str>,
    model: impl AsRef<str>,
    attempt_index: usize,
) -> AgentTuiRuntimeEvent {
    let provider = provider.as_ref();
    let model = model.as_ref();
    let summary = if provider.is_empty() {
        format!("{model} 生成回复中")
    } else {
        format!("{provider}/{model} 生成回复中")
    };
    lifecycle_status_event(AgentTuiLifecycleStatusView::with_id(
        AgentTuiLifecycleStatusViewInput {
            id: "test:lifecycle:streaming-started".to_owned(),
            title: "生成".to_owned(),
            kind: "StreamingStarted".to_owned(),
            summary,
            phase: "streaming".to_owned(),
            tool_name: Some(model.to_owned()),
            duration_ms: None,
            attempt_index: Some(attempt_index),
        },
    ))
}

pub(crate) fn streaming_completed_event(
    provider: impl AsRef<str>,
    model: impl AsRef<str>,
    attempt_index: usize,
) -> AgentTuiRuntimeEvent {
    let provider = provider.as_ref();
    let model = model.as_ref();
    let summary = if provider.is_empty() {
        format!("{model} 生成回复中")
    } else {
        format!("{provider}/{model} 生成回复中")
    };
    lifecycle_status_event(AgentTuiLifecycleStatusView::with_id(
        AgentTuiLifecycleStatusViewInput {
            id: "test:lifecycle:streaming-completed".to_owned(),
            title: "生成".to_owned(),
            kind: "StreamingCompleted".to_owned(),
            summary,
            phase: "streaming".to_owned(),
            tool_name: Some(model.to_owned()),
            duration_ms: None,
            attempt_index: Some(attempt_index),
        },
    ))
}

pub(crate) fn usage_update_event(usage: AgentTuiTokenUsage) -> AgentTuiRuntimeEvent {
    wire_event(WireEventKind::UsageUpdate(WireUsageUpdate {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        current_context_tokens: usage.current_context_tokens,
        system_prompt_tokens: 0,
        tool_definition_tokens: 0,
        conversation_tokens: 0,
    }))
}

pub(crate) fn tool_started_event(view: AgentTuiToolExecutionView) -> AgentTuiRuntimeEvent {
    wire_event(WireEventKind::ToolCall(WireToolCall {
        part_id: view.part_id,
        tool_call_id: view.provider_call_id,
        request_id: Some(view.request_id),
        tool_name: view.tool_name,
        tool_kind: view.kind,
        summary: view.arguments_preview.clone(),
        arguments_summary: Some(view.arguments_preview),
        intent: None,
        execution_permission: Some(format!("{:?}", view.execution_permission)),
    }))
}

pub(crate) fn tool_output_chunk_event(chunk: AgentTuiToolOutputChunk) -> AgentTuiRuntimeEvent {
    wire_event(WireEventKind::ToolResult(WireToolResult {
        part_id: chunk.part_id,
        tool_call_id: chunk.provider_call_id,
        tool_name: Some(chunk.tool_name),
        tool_kind: Some(tool_kind_code(chunk.semantic_kind).to_owned()),
        status: WireLifecycleStatus::Running,
        summary: chunk.text.clone(),
        output_preview: Some(chunk.text),
        stream: Some(chunk.stream),
        sequence: Some(chunk.sequence),
        progress_phase: None,
        items_completed: None,
        items_total: None,
        current_item: None,
        duration_ms: None,
        metadata: None,
        error_type: None,
        filesystem_write_performed: false,
    }))
}

pub(crate) fn tool_progress_event(progress: AgentTuiToolProgressView) -> AgentTuiRuntimeEvent {
    wire_event(WireEventKind::ToolResult(WireToolResult {
        part_id: progress.part_id,
        tool_call_id: progress.provider_call_id,
        tool_name: Some(progress.tool_name),
        tool_kind: Some(tool_kind_code(progress.semantic_kind).to_owned()),
        status: WireLifecycleStatus::Running,
        summary: progress.message.clone().unwrap_or_default(),
        output_preview: progress.message,
        stream: None,
        sequence: None,
        progress_phase: Some(progress.phase),
        items_completed: progress.items_completed,
        items_total: progress.items_total,
        current_item: progress.current_item,
        duration_ms: progress.elapsed_ms,
        metadata: None,
        error_type: None,
        filesystem_write_performed: false,
    }))
}

pub(crate) fn tool_completed_event(view: AgentTuiToolCompletionView) -> AgentTuiRuntimeEvent {
    wire_event(WireEventKind::ToolResult(WireToolResult {
        part_id: view.part_id,
        tool_call_id: view.provider_call_id,
        tool_name: Some(view.tool_name),
        tool_kind: Some(view.kind),
        status: lifecycle_status_from_str(&view.status),
        summary: view.summary,
        output_preview: None,
        stream: None,
        sequence: None,
        progress_phase: None,
        items_completed: None,
        items_total: None,
        current_item: None,
        duration_ms: view.duration_ms,
        metadata: view.metadata,
        error_type: view.error_type,
        filesystem_write_performed: view.filesystem_write_performed,
    }))
}

pub(crate) fn tool_diff_snapshot_event(view: AgentTuiToolDiffSnapshotView) -> AgentTuiRuntimeEvent {
    let (path, change) = view.changes.into_iter().next().unwrap_or_else(|| {
        (
            std::path::PathBuf::from("unknown"),
            FileChange::Add {
                content: String::new(),
            },
        )
    });
    wire_event(WireEventKind::FileChange(WireFileChangeEvent {
        part_id: view.id,
        tool_part_id: view.tool_part_id,
        tool_call_id: Some(view.provider_call_id),
        path: path.to_string_lossy().into_owned(),
        change,
    }))
}

pub(crate) fn hook_status_event(view: AgentTuiHookStatusView) -> AgentTuiRuntimeEvent {
    if view.name == "Skill" {
        return wire_event(WireEventKind::SkillStatus(WireSkillStatus {
            skill_name: view.run_id,
            status: hook_status_to_lifecycle(view.status),
            outcome: hook_outcome(view.status).to_owned(),
            risk: None,
            summary: view.status_message,
            material_ref: None,
            metadata: None,
        }));
    }
    wire_event(WireEventKind::GovernanceStatus(
        axon_protocol::wire::WireGovernanceStatus {
            job_id: view.run_id,
            status: hook_status_to_lifecycle(view.status),
            status_path: None,
            reason: view.status_message,
        },
    ))
}

fn hook_outcome(status: AgentTuiHookStatus) -> &'static str {
    match status {
        AgentTuiHookStatus::Completed => "completed",
        AgentTuiHookStatus::Failed => "failed",
        AgentTuiHookStatus::Running => "running",
        AgentTuiHookStatus::Pending => "pending",
        AgentTuiHookStatus::Blocked => "blocked",
        AgentTuiHookStatus::Stopped => "stopped",
        AgentTuiHookStatus::Unknown => "unknown",
    }
}

fn lifecycle_status_from_str(value: &str) -> WireLifecycleStatus {
    match value {
        "completed" | "complete" => WireLifecycleStatus::Completed,
        "failed" | "error" => WireLifecycleStatus::Failed,
        "cancelled" | "canceled" => WireLifecycleStatus::Cancelled,
        "aborted" => WireLifecycleStatus::Aborted,
        "blocked" => WireLifecycleStatus::Blocked,
        "waiting" => WireLifecycleStatus::Waiting,
        _ => WireLifecycleStatus::Running,
    }
}

fn hook_status_to_lifecycle(status: AgentTuiHookStatus) -> WireLifecycleStatus {
    match status {
        AgentTuiHookStatus::Completed => WireLifecycleStatus::Completed,
        AgentTuiHookStatus::Failed => WireLifecycleStatus::Failed,
        AgentTuiHookStatus::Running => WireLifecycleStatus::Running,
        AgentTuiHookStatus::Pending => WireLifecycleStatus::Waiting,
        AgentTuiHookStatus::Blocked => WireLifecycleStatus::Blocked,
        AgentTuiHookStatus::Stopped => WireLifecycleStatus::Aborted,
        AgentTuiHookStatus::Unknown => WireLifecycleStatus::Running,
    }
}

fn lifecycle_phase_name(phase: super::run_output::AgentTuiLifecyclePhase) -> &'static str {
    match phase {
        super::run_output::AgentTuiLifecyclePhase::Streaming => "streaming",
        super::run_output::AgentTuiLifecyclePhase::Generation => "generation",
        super::run_output::AgentTuiLifecyclePhase::Notice => "notice",
        super::run_output::AgentTuiLifecyclePhase::Command => "command",
        super::run_output::AgentTuiLifecyclePhase::Unknown => "unknown",
    }
}

fn tool_kind_code(kind: super::run_output::AgentTuiToolKind) -> &'static str {
    match kind {
        super::run_output::AgentTuiToolKind::Search => "search",
        super::run_output::AgentTuiToolKind::ReadFile => "read_file",
        super::run_output::AgentTuiToolKind::Glob => "glob",
        super::run_output::AgentTuiToolKind::MemorySearch => "memory_search",
        super::run_output::AgentTuiToolKind::MemoryWrite => "memory_write",
        super::run_output::AgentTuiToolKind::History => "history",
        super::run_output::AgentTuiToolKind::Notes => "notes",
        super::run_output::AgentTuiToolKind::Mcp => "mcp",
        super::run_output::AgentTuiToolKind::InvokeSkill => "invoke_skill",
        super::run_output::AgentTuiToolKind::Shell => "shell",
        super::run_output::AgentTuiToolKind::Edit => "edit",
        super::run_output::AgentTuiToolKind::Unsupported => "unsupported",
    }
}
