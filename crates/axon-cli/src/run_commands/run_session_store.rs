use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use axon_agent_runtime::{
    AGENT_RUN_ERROR_TYPE_INTERRUPTED, AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL,
    AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT, AgentModelLocalFileRef, AgentModelLocalImageRef,
    AgentModelMessage, AgentRunReasoningMessage, AgentTurnEvent, AgentTurnEventKind,
    TokenUsageTelemetry, agent_run_tool_receipt_completed_from_facts, redact_agent_context_text,
};
use axon_domain::{
    RUNTIME_FAILURE_CODE_INTERRUPTED, RUNTIME_FAILURE_CODE_RUNTIME_FAILED, RuntimeFailure, TurnId,
};
use axon_session::{ThreadHistoryFileRef, ThreadHistoryImageRef};

use super::{
    RunRequest,
    run_execution::{RunResponse, RunToolReceipt, now_ms},
    run_memory_recall::RunMemoryRecallEvent,
    run_memory_usefulness::RunMemoryUsefulnessEvent,
    run_rollout_store::{RolloutSqliteStore, resolved_rollout_db_path},
    run_startup_snapshot::RunStartupTimeline,
};
use crate::runtime_paths::runtime_axon_root_path_static;
use crate::runtime_workspace_instructions::WorkspaceInstruction;

const RUN_PERSISTENCE_ASSISTANT_PART_PREFIX: &str = "assistant-run";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunSessionRecord {
    pub(crate) session_id: String,
    pub(super) parent_session_id: Option<String>,
    pub(super) created_at_ms: u64,
    pub(crate) startup_timeline: RunStartupTimeline,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    pub(super) provider: String,
    pub(super) model: String,
    #[serde(default)]
    pub(super) selected_skill_name: Option<String>,
    #[serde(default)]
    pub(super) repo_context: Option<RunRepoContextRecord>,
    #[serde(default)]
    pub(super) session_context: Option<RunSessionContextRecord>,
    #[serde(default)]
    pub(super) durable_memory: Option<RunDurableMemoryRecord>,
    pub(super) prompt: String,
    pub(super) local_images: Vec<ThreadHistoryImageRef>,
    pub(super) local_files: Vec<ThreadHistoryFileRef>,
    pub(super) response: String,
    pub(super) assistant_part_id: String,
    pub(super) usage: Option<TokenUsageTelemetry>,
    #[serde(default)]
    pub(super) tool_receipts: Vec<RunToolReceipt>,
    pub(super) tool_calls: Vec<axon_agent_runtime::ProviderToolCall>,
    #[serde(default)]
    pub(super) reasoning_messages: Vec<RunReasoningMessageRecord>,
    /// 本轮注入 Provider 的非会话上下文；随 turn 持久化，供后续轮次原序回放。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) injected_context_messages: Vec<AgentModelMessage>,
    #[serde(default)]
    pub(super) turn_events: Vec<AgentTurnEvent>,
    pub(super) failure: Option<RuntimeFailure>,
    #[serde(default)]
    pub(super) upstream_model: Option<String>,
    #[serde(default)]
    pub(super) reasoning_effort: Option<String>,
    #[serde(default)]
    pub(super) request_path: Option<String>,
    #[serde(default)]
    pub(super) speed_mode: Option<String>,
    #[serde(default)]
    pub(super) ttft_ms: Option<u64>,
    #[serde(default)]
    pub(super) generation_speed_tps: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunRepoContextRecord {
    pub(super) cwd: String,
    #[serde(default)]
    pub(super) instruction_paths: Vec<String>,
    pub(super) instruction_count: usize,
    #[serde(default)]
    pub(super) primary_instruction_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunSessionContextRecord {
    pub(super) prompt: String,
    pub(super) response_preview: String,
    #[serde(default)]
    pub(super) latest_failure: Option<String>,
    pub(super) suggested_next_step: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunDurableMemoryRecord {
    #[serde(default)]
    pub(super) recall_status: Option<String>,
    pub(super) recall_hit_count: usize,
    #[serde(default)]
    pub(super) recall_embedding_status: Option<String>,
    #[serde(default)]
    pub(super) recall_reason: Option<String>,
    #[serde(default)]
    pub(super) write_status: Option<String>,
    #[serde(default)]
    pub(super) write_reason: Option<String>,
    #[serde(default)]
    pub(super) usefulness: Option<RunMemoryUsefulnessEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunReasoningMessageRecord {
    pub(super) part_id: String,
    pub(super) reasoning_id: String,
    pub(super) content: String,
}

impl RunReasoningMessageRecord {
    pub(super) fn from_runtime(message: &AgentRunReasoningMessage) -> Self {
        Self {
            part_id: message.part_id.clone(),
            reasoning_id: message.reasoning_id.clone(),
            content: message.content.clone(),
        }
    }
}

impl RunSessionRecord {
    pub(super) fn started(request: &RunRequest) -> Self {
        Self::from_partial(
            request,
            &RunSessionPartialInput {
                provider: request.provider.name.clone(),
                model: request.model.id.clone(),
                tool_calls: Vec::new(),
                tool_receipts: Vec::new(),
                reasoning_messages: Vec::new(),
                turn_events: Vec::new(),
            },
        )
    }

    pub(super) fn from_completed(
        request: &RunRequest,
        response: &RunResponse,
        memory_usefulness: Option<&RunMemoryUsefulnessEvent>,
        workspace_instructions: &[WorkspaceInstruction],
    ) -> Self {
        let created_at_ms = now_ms();
        let (local_images, local_files) = run_user_attachments(&request.model_payload.messages);
        let ttft_ms = compute_request_ttft_ms(request);
        let generation_speed_tps = compute_generation_speed_tps(
            ttft_ms,
            response.token_usage.as_ref(),
            &response.turn_events,
        );
        Self {
            session_id: request.session_id.clone(),
            parent_session_id: request.parent_session_id.clone(),
            created_at_ms,
            startup_timeline: request.startup.timeline.clone(),
            cwd: Some(request.cwd.to_string_lossy().to_string()),
            provider: response.provider.clone(),
            model: response.model.clone(),
            selected_skill_name: request.selected_skill_name.clone(),
            repo_context: Some(repo_context_record(&request.cwd, workspace_instructions)),
            session_context: Some(session_context_record(
                &request.prompt,
                &response.content,
                &response.tool_receipts,
            )),
            durable_memory: Some(durable_memory_record(
                request.memory_recall.as_ref(),
                &response.tool_receipts,
                memory_usefulness,
            )),
            prompt: request.prompt.clone(),
            local_images,
            local_files,
            response: response.content.clone(),
            assistant_part_id: run_persistence_assistant_part_id(request),
            usage: response.token_usage,
            tool_receipts: response.tool_receipts.clone(),
            tool_calls: response.tool_calls.clone(),
            reasoning_messages: response
                .reasoning_messages
                .iter()
                .map(RunReasoningMessageRecord::from_runtime)
                .collect(),
            injected_context_messages: request.model_payload.injected_context_messages.clone(),
            turn_events: response.turn_events.clone(),
            failure: None,
            upstream_model: Some(response.model.clone()),
            reasoning_effort: request.reasoning_effort.clone(),
            request_path: Some(request.provider.name.clone()),
            speed_mode: None,
            ttft_ms,
            generation_speed_tps,
        }
    }

    pub(super) fn from_partial(request: &RunRequest, input: &RunSessionPartialInput) -> Self {
        let created_at_ms = now_ms();
        let (local_images, local_files) = run_user_attachments(&request.model_payload.messages);
        let ttft_ms = compute_request_ttft_ms(request);
        Self {
            session_id: request.session_id.clone(),
            parent_session_id: request.parent_session_id.clone(),
            created_at_ms,
            startup_timeline: request.startup.timeline.clone(),
            cwd: Some(request.cwd.to_string_lossy().to_string()),
            provider: input.provider.clone(),
            model: input.model.clone(),
            selected_skill_name: request.selected_skill_name.clone(),
            repo_context: None,
            session_context: None,
            durable_memory: None,
            prompt: request.prompt.clone(),
            local_images,
            local_files,
            response: String::new(),
            assistant_part_id: run_persistence_assistant_part_id(request),
            usage: None,
            tool_receipts: input.tool_receipts.clone(),
            tool_calls: input.tool_calls.clone(),
            reasoning_messages: input
                .reasoning_messages
                .iter()
                .map(RunReasoningMessageRecord::from_runtime)
                .collect(),
            injected_context_messages: request.model_payload.injected_context_messages.clone(),
            turn_events: input.turn_events.clone(),
            failure: None,
            upstream_model: Some(input.model.clone()),
            reasoning_effort: request.reasoning_effort.clone(),
            request_path: Some(request.provider.name.clone()),
            speed_mode: None,
            ttft_ms,
            generation_speed_tps: None,
        }
    }

    pub(super) fn mark_failed(
        &mut self,
        failure: RuntimeFailure,
        partial_response: String,
        error: &super::run_execution::RunExecutionError,
    ) {
        self.response = partial_response;
        self.usage = error.token_usage;
        self.tool_receipts = error.tool_receipts.to_vec();
        self.failure = Some(failure);
        let terminal_kind = if error.error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED {
            AgentTurnEventKind::Interrupted
        } else {
            AgentTurnEventKind::Failed
        };
        if !self
            .turn_events
            .iter()
            .any(|event| event.kind == terminal_kind)
        {
            let turn_id = self
                .turn_events
                .iter()
                .map(|event| event.turn_id)
                .max_by_key(|turn_id| turn_id.0)
                .unwrap_or(TurnId(1));
            let summary = if terminal_kind == AgentTurnEventKind::Interrupted {
                "run interrupted"
            } else {
                "run failed"
            };
            self.turn_events.push(
                AgentTurnEvent::new(turn_id, terminal_kind, summary).with_phase("completion"),
            );
        }
    }
}

fn run_user_attachments(
    messages: &[AgentModelMessage],
) -> (Vec<ThreadHistoryImageRef>, Vec<ThreadHistoryFileRef>) {
    let Some(message) = messages.iter().rev().find(|message| message.role == "user") else {
        return (Vec::new(), Vec::new());
    };
    let images = message.local_images.iter().map(history_image_ref).collect();
    let files = message.local_files.iter().map(history_file_ref).collect();
    (images, files)
}

fn history_image_ref(image: &AgentModelLocalImageRef) -> ThreadHistoryImageRef {
    ThreadHistoryImageRef {
        path: image.path.clone(),
        mime_type: image.mime_type.clone(),
        byte_len: image.byte_len,
        sha256: image.sha256.clone(),
        archive_ref: image.archive_ref.clone(),
        source_ref: image.source_ref.clone(),
    }
}

fn history_file_ref(file: &AgentModelLocalFileRef) -> ThreadHistoryFileRef {
    ThreadHistoryFileRef {
        name: file.name.clone(),
        path: file.path.clone(),
        mime_type: file.mime_type.clone(),
        preview: file.preview.clone(),
        byte_len: file.byte_len,
        sha256: file.sha256.clone(),
        archive_ref: file.archive_ref.clone(),
        source_ref: file.source_ref.clone(),
    }
}

pub(super) fn run_persistence_assistant_part_id(request: &RunRequest) -> String {
    format!(
        "{}-{}",
        RUN_PERSISTENCE_ASSISTANT_PART_PREFIX, request.invocation_id
    )
}

fn compute_request_ttft_ms(request: &RunRequest) -> Option<u64> {
    let recorder = request.performance.as_ref()?;
    let snapshot = recorder.snapshot();
    let start = snapshot
        .stages
        .iter()
        .find(|s| s.stage == axon_session_runtime::TurnPerformanceStage::ProviderRequestStarted)?;
    let first = snapshot.stages.iter().find(|s| {
        s.stage == axon_session_runtime::TurnPerformanceStage::ProviderFirstDeltaParsed
    })?;
    if first.elapsed_us >= start.elapsed_us {
        Some((first.elapsed_us - start.elapsed_us) / 1000)
    } else {
        None
    }
}

/// 本轮生成速率：与头部同口径，含思考输出，窗口只算 provider 实际流式耗时。
///
/// 首字只从**第一条** provider 流的时间里扣：思考型模型的首字可能晚于该流收口，
/// 整笔扣会连带扣掉后续流的时间，把速率抬到几千。
fn compute_generation_speed_tps(
    ttft_ms: Option<u64>,
    usage: Option<&TokenUsageTelemetry>,
    turn_events: &[AgentTurnEvent],
) -> Option<f64> {
    let ttft = ttft_ms?;
    let (output_tokens, reasoning_output_tokens) = usage
        .map(|u| (u.output_tokens, u.reasoning_output_tokens))
        .unwrap_or((0, 0));
    let generated_tokens = output_tokens.saturating_add(reasoning_output_tokens);
    if generated_tokens == 0 {
        return None;
    }
    let provider_ms: Vec<u64> = turn_events
        .iter()
        .filter(|event| event.kind == axon_agent_runtime::AgentTurnEventKind::ProviderCompleted)
        .filter_map(|event| event.duration_ms)
        .collect();
    let total_provider_ms = provider_ms
        .iter()
        .fold(0_u64, |total, value| total.saturating_add(*value));
    let latency_ms = provider_ms.first().map_or(ttft, |first| ttft.min(*first));
    let decode_ms = total_provider_ms.saturating_sub(latency_ms);
    axon_domain::GenerationMetrics::calculate_generation_speed_tps(generated_tokens, decode_ms)
}

pub(super) struct RunSessionPartialInput {
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) tool_calls: Vec<axon_agent_runtime::ProviderToolCall>,
    pub(super) tool_receipts: Vec<RunToolReceipt>,
    pub(super) reasoning_messages: Vec<AgentRunReasoningMessage>,
    pub(super) turn_events: Vec<AgentTurnEvent>,
}

fn repo_context_record(
    cwd: &Path,
    workspace_instructions: &[WorkspaceInstruction],
) -> RunRepoContextRecord {
    RunRepoContextRecord {
        cwd: cwd.to_string_lossy().to_string(),
        instruction_paths: workspace_instructions
            .iter()
            .map(|instruction| instruction.path.clone())
            .collect(),
        instruction_count: workspace_instructions.len(),
        primary_instruction_summary: workspace_instructions
            .last()
            .map(|instruction| summarize_instruction(&instruction.content)),
    }
}

fn session_context_record(
    prompt: &str,
    response: &str,
    tool_receipts: &[RunToolReceipt],
) -> RunSessionContextRecord {
    RunSessionContextRecord {
        prompt: prompt.trim().to_owned(),
        response_preview: bounded_line(response, 160),
        latest_failure: last_tool_failure(tool_receipts),
        suggested_next_step: suggested_next_step(tool_receipts),
    }
}

fn durable_memory_record(
    recall: Option<&RunMemoryRecallEvent>,
    tool_receipts: &[RunToolReceipt],
    memory_usefulness: Option<&RunMemoryUsefulnessEvent>,
) -> RunDurableMemoryRecord {
    let memory_write = tool_receipts
        .iter()
        .rev()
        .find(|receipt| receipt.tool_name == "builtin.memory_write");
    RunDurableMemoryRecord {
        recall_status: recall.map(|event| event.status.to_owned()),
        recall_hit_count: recall.map_or(0, |event| event.hit_count),
        recall_embedding_status: recall.map(|event| event.embedding_status.to_owned()),
        recall_reason: recall.and_then(|event| event.reason.clone()),
        write_status: memory_write.map(|receipt| receipt.status.clone()),
        write_reason: memory_write.map(|receipt| receipt.summary.clone()),
        usefulness: memory_usefulness.cloned(),
    }
}

fn summarize_instruction(content: &str) -> String {
    bounded_line(
        content
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(""),
        80,
    )
}

fn last_tool_failure(tool_receipts: &[RunToolReceipt]) -> Option<String> {
    tool_receipts
        .iter()
        .rev()
        .find(|receipt| !agent_run_tool_receipt_completed_from_facts(*receipt))
        .map(|receipt| {
            let summary = if receipt.summary.trim().is_empty() {
                "工具执行失败"
            } else {
                receipt.summary.trim()
            };
            format!("{}: {}", receipt.tool_name, bounded_line(summary, 120))
        })
}

fn suggested_next_step(tool_receipts: &[RunToolReceipt]) -> String {
    if let Some(failure) = last_tool_failure(tool_receipts) {
        return format!("优先修复上一轮失败点：{failure}");
    }
    "基于已恢复上下文继续执行，并在落地修改后再次运行验证".to_owned()
}

fn bounded_line(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut output = trimmed.chars().take(max_chars).collect::<String>();
    output.push_str("...");
    output
}

pub(super) fn runtime_failure_from_execution_error(
    session_id: &str,
    error: &super::run_execution::RunExecutionError,
    partial_response: &str,
    timestamp: u64,
) -> RuntimeFailure {
    let code = error.error_code.clone().unwrap_or_else(|| {
        if error.error_type == AGENT_RUN_ERROR_TYPE_INTERRUPTED {
            RUNTIME_FAILURE_CODE_INTERRUPTED.to_owned()
        } else {
            RUNTIME_FAILURE_CODE_RUNTIME_FAILED.to_owned()
        }
    });
    let visible_output_emitted =
        error.visible_output_emitted || !partial_response.trim().is_empty();
    let replay_safe = !visible_output_emitted && error.tool_receipts.is_empty();
    let attempt_count = u32::try_from(error.channel_attempts.len().max(1)).unwrap_or(u32::MAX);
    RuntimeFailure {
        category: error.error_type.to_owned(),
        code,
        provider_protocol: error.provider_protocol.clone(),
        redacted_message: redact_agent_context_text(&error.message),
        http_status: error.http_status,
        caused_by_event: Some(format!("run:{session_id}:runtime-error")),
        visible_output_emitted,
        retryable: matches!(
            error.error_type,
            AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL | AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT
        ),
        replay_safe,
        attempt_count,
        partial_output_present: !partial_response.trim().is_empty(),
        timestamp,
        failure_kind: error.failure_kind.clone(),
        missing_field: (error.error_type == AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL)
            .then(|| error.error_param.clone())
            .flatten(),
        call_index: error.call_index,
        provider_event_type: error.provider_event_type.clone(),
        upstream_request_id: error.upstream_request_id.clone(),
    }
}

pub(crate) fn resolved_axon_root(config_path: &Path) -> Result<PathBuf, &'static str> {
    runtime_axon_root_path_static(config_path)
}

pub(super) enum RunSessionIndexLookup<'a> {
    Latest,
    SessionId(&'a str),
}

pub(super) fn load_indexed_run_session(
    config_path: &Path,
    lookup: &RunSessionIndexLookup<'_>,
) -> Result<Option<RunSessionRecord>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Ok(None);
    }
    let store = RolloutSqliteStore::open(&path)?;
    match lookup {
        RunSessionIndexLookup::Latest => store.load_latest(),
        RunSessionIndexLookup::SessionId(session_id) => store.load_by_id(session_id),
    }
}

#[cfg(test)]
mod runtime_failure_tests {
    use super::*;

    fn provider_protocol_error() -> super::super::run_execution::RunExecutionError {
        super::super::run_execution::RunExecutionError {
            message: "provider 返回不完整工具调用".to_owned(),
            http_status: Some(200),
            error_type: AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL,
            error_code: Some(
                axon_domain::RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE.to_owned(),
            ),
            provider_protocol: Some("openai".to_owned()),
            error_param: Some("arguments".to_owned()),
            failure_kind: Some(axon_domain::RUNTIME_FAILURE_KIND_INCOMPLETE.to_owned()),
            call_index: Some(0),
            provider_error: None,
            token_usage: None,
            tool_receipts: Box::new([]),
            visible_output_emitted: false,
            channel_attempts: Vec::new(),
            provider_event_type: None,
            upstream_request_id: None,
            committed_side_effects: false,
        }
    }

    #[test]
    fn failure_without_visible_output_or_tool_receipts_is_replay_safe() {
        let failure = runtime_failure_from_execution_error(
            "session-safe",
            &provider_protocol_error(),
            "",
            10,
        );

        assert!(!failure.visible_output_emitted);
        assert!(!failure.partial_output_present);
        assert_eq!(failure.provider_protocol.as_deref(), Some("openai"));
        assert!(failure.replay_safe);
    }

    #[test]
    fn failure_after_partial_assistant_output_is_not_replay_safe() {
        let failure = runtime_failure_from_execution_error(
            "session-visible",
            &provider_protocol_error(),
            "已输出的部分回答",
            11,
        );

        assert!(failure.visible_output_emitted);
        assert!(failure.partial_output_present);
        assert!(!failure.replay_safe);
    }
}

#[cfg(test)]
mod attachment_projection_tests {
    use super::*;

    #[test]
    fn run_user_attachments_selects_latest_user_and_preserves_strong_refs() {
        let image = AgentModelLocalImageRef {
            path: String::new(),
            mime_type: "image/png".to_owned(),
            byte_len: Some(8),
            sha256: Some("image-sha".to_owned()),
            archive_ref: Some("blobs/image-sha.blob".to_owned()),
            source_ref: Some("local-upload://image.png".to_owned()),
        };
        let file = AgentModelLocalFileRef {
            name: "notes.txt".to_owned(),
            path: Some("C:\\work\\notes.txt".to_owned()),
            mime_type: "text/plain".to_owned(),
            preview: "preview".to_owned(),
            byte_len: Some(7),
            sha256: Some("file-sha".to_owned()),
            archive_ref: Some("blobs/file-sha.blob".to_owned()),
            source_ref: Some("local-upload://notes.txt".to_owned()),
        };
        let messages = vec![
            AgentModelMessage::text("user", "旧消息")
                .with_local_images(vec![AgentModelLocalImageRef::local("old.png", "image/png")]),
            AgentModelMessage::text("assistant", "处理中"),
            AgentModelMessage::text("user", "当前消息")
                .with_local_images(vec![image.clone()])
                .with_local_files(vec![file.clone()]),
        ];

        let (images, files) = run_user_attachments(&messages);

        assert_eq!(images, vec![history_image_ref(&image)]);
        assert_eq!(files, vec![history_file_ref(&file)]);
    }
}
