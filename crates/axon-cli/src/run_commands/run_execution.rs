use std::time::{SystemTime, UNIX_EPOCH};

use axon_agent_runtime::{
    AgentRunChannelAttempt, AgentRunExecutionError, AgentRunProviderErrorProjection,
    AgentRunResponse, AgentTurnEvent, MemoryAutonomyFeedbackReport,
};
use axon_domain::RuntimeFailure;
use axon_protocol::wire::WireEventEnvelope;
use serde::Serialize;

use super::run_progress::RunProgressSink;
pub(super) use super::run_tool_loop::RunToolReceipt;
use super::{
    RunRequest,
    run_json_wire_events::{
        CompletedRunWireEventsInput, completed_run_wire_events, failed_run_wire_events,
    },
};

pub(super) type ChannelAttempt = AgentRunChannelAttempt;
pub(super) type RunResponse = AgentRunResponse<RunToolReceipt>;
pub(super) type RunExecutionError = AgentRunExecutionError<RunToolReceipt>;
pub(super) type ProviderErrorProjection = AgentRunProviderErrorProjection;

pub(super) struct ProviderStreamingRunner;

impl ProviderStreamingRunner {
    pub(super) fn run(
        request: &RunRequest,
        progress: Option<&RunProgressSink<'_>>,
    ) -> Result<RunResponse, Box<RunExecutionError>> {
        super::run_failover::run_with_failover(request, progress)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunJsonEvent {
    pub(crate) event: &'static str,
    pub(crate) status: &'static str,
    pub(crate) session_id: String,
    resumed_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_title: Option<String>,
    pub(crate) provider: String,
    pub(crate) model: String,
    /// 本轮实际请求的思考强度。
    ///
    /// 客户端据此显示这一轮真正使用的强度，而不是再去猜配置值；为空表示这一轮
    /// 没有可用的强度（配置与模型都不支持）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) selected_skill_name: Option<String>,
    pub(crate) selected_skill_source_ref: Option<String>,
    pub(crate) http_status: Option<u16>,
    pub(crate) content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure: Option<RuntimeFailure>,
    pub(crate) provider_error: Option<Box<ProviderErrorProjection>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) channel_attempts: Vec<ChannelAttempt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) reasoning_messages: Vec<axon_agent_runtime::AgentRunReasoningMessage>,
    pub(crate) turn_events: Vec<AgentTurnEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) wire_events: Vec<WireEventEnvelope>,
    /// 上一轮的记忆反思提示；无提示时整字段省略，客户端据此决定是否显示。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) memory_reflection_notice: Option<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) failed_tool_attempts:
        Vec<super::run_outcome_diagnostics::FailedToolAttemptDiagnostic>,
    pub(crate) provider_elapsed_ms: u64,
    pub(crate) orchestration_elapsed_ms: u64,
    pub(crate) total_elapsed_ms: u64,
}

impl RunJsonEvent {
    pub(super) fn completed(input: CompletedRunWireEventsInput<'_>) -> Self {
        let request = input.request;
        let response = input.response;
        let diagnostics = super::run_outcome_diagnostics::collect(
            response,
            input.total_elapsed_ms,
            &request.persona_warnings,
        );
        Self {
            event: "run_completed",
            status: diagnostics.status,
            session_id: request.session_id.clone(),
            resumed_from: request.resumed_from.clone(),
            session_title: input.session_title.map(str::to_owned),
            provider: response.provider.clone(),
            model: response.model.clone(),
            reasoning_effort: non_empty_reasoning_effort(request.reasoning_effort.as_deref()),
            selected_skill_name: request.selected_skill_name.clone(),
            selected_skill_source_ref: request.selected_skill_source_ref.clone(),
            http_status: Some(response.http_status),
            content: completed_content(&response.content),
            failure: None,
            provider_error: None,
            channel_attempts: response.channel_attempts.clone(),
            reasoning_messages: response.reasoning_messages.clone(),
            turn_events: response.turn_events.clone(),
            wire_events: completed_run_wire_events(&input),
            memory_reflection_notice: None,
            warnings: diagnostics.warnings,
            failed_tool_attempts: diagnostics.failed_tool_attempts,
            provider_elapsed_ms: diagnostics.provider_elapsed_ms,
            orchestration_elapsed_ms: diagnostics.orchestration_elapsed_ms,
            total_elapsed_ms: diagnostics.total_elapsed_ms,
        }
    }

    pub(super) fn failed(
        request: &RunRequest,
        error: &RunExecutionError,
        failure: &RuntimeFailure,
        memory_usefulness: Option<&super::run_memory_usefulness::RunMemoryUsefulnessEvent>,
        memory_autonomy: Option<&MemoryAutonomyFeedbackReport>,
    ) -> Self {
        Self {
            event: "run_failed",
            status: "failed",
            session_id: request.session_id.clone(),
            resumed_from: request.resumed_from.clone(),
            session_title: None,
            provider: request.provider.name.clone(),
            model: request.model.id.clone(),
            reasoning_effort: non_empty_reasoning_effort(request.reasoning_effort.as_deref()),
            selected_skill_name: request.selected_skill_name.clone(),
            selected_skill_source_ref: request.selected_skill_source_ref.clone(),
            http_status: error.http_status,
            content: None,
            failure: Some(failure.clone()),
            provider_error: error.provider_error.clone(),
            channel_attempts: error.channel_attempts.clone(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
            wire_events: failed_run_wire_events(
                request,
                error,
                failure,
                memory_usefulness,
                memory_autonomy,
                wire_recorded_at_ms(),
            ),
            // 失败轮不展示记忆提示：先把注意力留给错误本身。
            memory_reflection_notice: None,
            warnings: request.persona_warnings.clone(),
            failed_tool_attempts: Vec::new(),
            provider_elapsed_ms: 0,
            orchestration_elapsed_ms: 0,
            total_elapsed_ms: 0,
        }
    }
}

pub(super) fn wire_recorded_at_ms() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

pub(super) fn new_run_session_id() -> String {
    format!("run-{}-{}", std::process::id(), now_ms())
}

pub(super) fn now_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_millis();
    u64::try_from(millis).expect("当前时间毫秒值应可表示为 u64")
}

fn completed_content(content: &str) -> Option<String> {
    (!content.trim().is_empty()).then(|| content.to_owned())
}

/// 去掉首尾空白的思考强度；缺失或空值按「没有强度」处理，而不是发一个空串。
fn non_empty_reasoning_effort(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{completed_content, non_empty_reasoning_effort};

    #[test]
    fn reasoning_effort_blank_values_are_omitted() {
        assert_eq!(non_empty_reasoning_effort(None), None);
        assert_eq!(non_empty_reasoning_effort(Some("  ")), None);
        assert_eq!(
            non_empty_reasoning_effort(Some(" high ")),
            Some("high".to_owned())
        );
    }

    #[test]
    fn completed_content_omits_empty_assistant_message() {
        assert_eq!(completed_content(" \n"), None);
    }

    #[test]
    fn completed_content_preserves_visible_assistant_message() {
        assert_eq!(completed_content("done"), Some("done".to_owned()));
    }
}
