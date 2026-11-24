//! Provider streaming transport 编排合同。
//!
//! 该模块只串联请求计划、外层 transport 回调和 provider runner；
//! HTTP 执行、API key 读取、超时策略和 socket 生命周期仍由 CLI / Agent Pane 外层适配器负责。

use axon_client_identity::ClientChannel;
use axon_domain::TurnId;
use axon_domain::{AxonError, AxonResult};

use axon_config::ProviderCapabilities;

use crate::{
    AgentModelHistorySnapshot, ContextTokenComposition, ModelToolDefinition,
    PromptCacheProviderWireFormat, ProviderPromptCachePlan, ProviderPromptCachePlanInput,
    ProviderPromptCachePlanRuntime, ProviderStreamProtocol, ProviderStreamingAttemptPlan,
    ProviderStreamingHttpRequestInput, ProviderStreamingHttpRequestPlan,
    ProviderStreamingHttpRequestRuntime, ProviderStreamingRunnerContext,
    ProviderStreamingRunnerDecision, ProviderStreamingRunnerReport, ProviderStreamingRunnerRuntime,
    ProviderStreamingTransportOutcome, ProviderVisibleCacheShape, default_attachment_archive_root,
    provider_streaming_retry_policy,
};

#[path = "provider_reasoning_effort.rs"]
mod provider_reasoning_effort;

use crate::provider_structured_output::{
    ProviderStructuredOutputContract, ProviderStructuredOutputNegotiation,
    STRUCTURED_OUTPUT_MAX_DOWNGRADE_ATTEMPTS,
};
use provider_reasoning_effort::{
    ProviderReasoningEffortNegotiation, REASONING_EFFORT_MAX_DOWNGRADE_ATTEMPTS,
};

/// provider transport loop 已规划请求原因。
pub const PROVIDER_STREAMING_TRANSPORT_REASON_REQUEST_PLANNED: &str =
    "provider_streaming_transport:request_planned";
/// provider transport loop 已完成原因。
pub const PROVIDER_STREAMING_TRANSPORT_REASON_COMPLETED: &str =
    "provider_streaming_transport:completed";
/// provider transport 远控 request body 关闭原因。
pub const PROVIDER_STREAMING_TRANSPORT_REASON_REMOTE_REQUEST_BODY_CLOSED: &str =
    "provider_streaming_transport:remote_request_body_closed";
/// provider transport 远控原始 payload 关闭原因。
pub const PROVIDER_STREAMING_TRANSPORT_REASON_REMOTE_PROVIDER_PAYLOAD_CLOSED: &str =
    "provider_streaming_transport:remote_provider_payload_closed";
/// provider transport 已自动降低 reasoning effort。
pub const PROVIDER_STREAMING_TRANSPORT_REASON_REASONING_EFFORT_DOWNGRADED: &str =
    "provider_streaming_transport:reasoning_effort_downgraded";
/// provider transport 已将结构化输出降级为 prompt-only。
pub const PROVIDER_STREAMING_TRANSPORT_REASON_STRUCTURED_OUTPUT_DOWNGRADED: &str =
    "provider_streaming_transport:structured_output_downgraded";

const PROVIDER_STREAMING_TRANSPORT_INITIAL_ATTEMPT_COUNT: usize = 1;
const PROVIDER_STREAMING_TRANSPORT_MAX_ATTEMPTS: usize =
    PROVIDER_STREAMING_TRANSPORT_INITIAL_ATTEMPT_COUNT
        + provider_streaming_retry_policy().stream_max_retries as usize
        + REASONING_EFFORT_MAX_DOWNGRADE_ATTEMPTS
        + STRUCTURED_OUTPUT_MAX_DOWNGRADE_ATTEMPTS;

/// Provider streaming 底层传输链路。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProviderStreamingWireTransport {
    /// HTTP/SSE streaming。
    #[default]
    Http,
    /// WebSocket streaming，失败时由外层 adapter 回退 HTTP。
    WebSocket,
}

/// Provider streaming transport loop 输入。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderStreamingTransportInput {
    /// 请求入口渠道。
    pub client_channel: ClientChannel,
    /// 本轮 transport loop 首个 agent turn 的序号；后续 turn 依次递增。
    ///
    /// 同一个 run 内每次渠道尝试都必须分配不重叠的起点（见 `AgentTurnRuntime::run`），
    /// `TurnId(0)` 表示未指定，按 1 起编号。
    pub turn_id: TurnId,
    /// provider 名称。
    pub provider_name: String,
    /// Provider 请求历史、缓存和服务端状态能力。
    pub capabilities: ProviderCapabilities,
    /// provider 协议。
    pub protocol: ProviderStreamProtocol,
    /// Provider base URL。
    pub base_url: String,
    /// API key 环境变量名。
    pub api_key_env: Option<String>,
    /// 直接配置的 API key，优先级高于 api_key_env。
    pub api_key: Option<String>,
    /// 模型 ID。
    pub model_id: String,
    /// OpenAI-compatible 请求的 service_tier；为空时不发送。
    pub service_tier: Option<String>,
    /// reasoning effort；Anthropic 请求层会映射为 thinking level。
    pub reasoning_effort: Option<String>,
    /// reasoning summary；provider 请求层会映射为对应展示字段。
    pub reasoning_summary: Option<String>,
    /// 本轮普通 Coding turn 的可选结构化输出契约。
    pub structured_output: Option<ProviderStructuredOutputContract>,
    /// 上下文窗口。
    pub context_window: u64,
    /// 是否支持工具。
    pub supports_tools: bool,
    /// 是否支持视觉。
    pub supports_vision: bool,
    /// 模型消息。
    pub messages: AgentModelHistorySnapshot,
    /// 模型可见工具定义。
    pub model_tools: Vec<ModelToolDefinition>,
    /// 本次请求的上下文构成估算；由 Runtime 在发出请求前填入，供用量帧一起投影。
    pub context_composition: Option<ContextTokenComposition>,
    /// 上下文 section 数量。
    pub context_section_count: usize,
    /// provider prompt cache 计划。
    pub prompt_cache_plan: ProviderPromptCachePlan,
    /// Anthropic Messages max_tokens。
    pub anthropic_max_tokens: u64,
    /// provider streaming 底层传输链路。
    pub wire_transport: ProviderStreamingWireTransport,
    /// 可选 WebSocket URL；为空时由 HTTP base URL 推导。
    pub websocket_url: Option<String>,
    /// 是否以 zstd 压缩 HTTP 请求体。
    pub enable_request_compression: bool,
    /// Responses 服务端会话状态引用。
    pub server_conversation_state_ref: Option<String>,
}

/// 单次 transport 尝试请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingTransportRequest {
    /// 当前 agent turn ID。
    pub turn_id: TurnId,
    /// runner 尝试计划。
    pub attempt: ProviderStreamingAttemptPlan,
    /// 当前请求可见的模型工具定义。
    pub model_tools: Vec<ModelToolDefinition>,
    /// 本次请求的上下文构成估算；无观测时为 None。
    pub context_composition: Option<ContextTokenComposition>,
    /// 最终 Provider 可见缓存形状。
    pub cache_shape: ProviderVisibleCacheShape,
    /// HTTP 请求计划。
    pub request: ProviderStreamingHttpRequestPlan,
    /// 第几次尝试。
    pub attempt_index: usize,
}

/// Provider streaming transport loop 完成报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingTransportReport {
    /// provider runner 完成报告。
    pub runner_report: ProviderStreamingRunnerReport,
    /// 成功尝试的 Provider 可见缓存形状。
    pub cache_shape: ProviderVisibleCacheShape,
    /// 请求尝试次数。
    pub attempt_count: usize,
    /// 是否允许远控读取完整请求 body。
    pub remote_request_body_allowed: bool,
    /// 是否允许远控读取 provider 原始 payload。
    pub remote_provider_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Provider streaming transport loop 运行结果。
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderStreamingTransportResult {
    /// 可审计的 transport 完成报告。
    pub report: ProviderStreamingTransportReport,
    /// 成功尝试实际采用的消息历史；仅供下一轮上下文推进。
    pub final_messages: AgentModelHistorySnapshot,
}

/// Provider streaming transport loop runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderStreamingTransportRuntime;

/// 规划 provider streaming prompt cache。
pub fn provider_streaming_prompt_cache_plan(
    input: &ProviderStreamingTransportInput,
) -> AxonResult<ProviderPromptCachePlan> {
    ProviderPromptCachePlanRuntime.plan(ProviderPromptCachePlanInput {
        provider_name: input.provider_name.clone(),
        format: prompt_cache_wire_format(input.protocol),
        model_id: input.model_id.clone(),
        context_window: input.context_window,
        supports_tools: input.supports_tools,
        supports_vision: input.supports_vision,
        messages: input.messages.clone(),
        model_tools: input.model_tools.clone(),
        context_section_count: input.context_section_count,
        capabilities: input.capabilities,
    })
}

impl ProviderStreamingTransportRuntime {
    /// 用外层 transport 回调执行 provider streaming loop。
    pub fn run_with_transport<F>(
        &self,
        input: &ProviderStreamingTransportInput,
        mut transport: F,
    ) -> AxonResult<ProviderStreamingTransportResult>
    where
        F: FnMut(&ProviderStreamingTransportRequest) -> ProviderStreamingTransportOutcome,
    {
        validate_input(input)?;
        let context = runner_context(input);
        let runner = ProviderStreamingRunnerRuntime;
        let mut attempt = runner.plan_initial(&context)?;
        let mut reasoning_effort = ProviderReasoningEffortNegotiation::new(
            &input.provider_name,
            input.protocol,
            &input.model_id,
            input.reasoning_effort.as_deref(),
        );
        let mut structured_output = ProviderStructuredOutputNegotiation::new(
            &input.provider_name,
            input.protocol,
            &input.model_id,
            input.structured_output.as_ref(),
        );
        let mut reasons = vec![
            PROVIDER_STREAMING_TRANSPORT_REASON_REMOTE_REQUEST_BODY_CLOSED.to_owned(),
            PROVIDER_STREAMING_TRANSPORT_REASON_REMOTE_PROVIDER_PAYLOAD_CLOSED.to_owned(),
        ];
        let mut last_transport_error = None;

        for attempt_index in 1..=PROVIDER_STREAMING_TRANSPORT_MAX_ATTEMPTS {
            let request = request_plan(
                input,
                &attempt,
                reasoning_effort.effective(),
                &structured_output,
            )?;
            let cache_shape = ProviderVisibleCacheShape::from_serialized_request(
                attempt.protocol,
                &request.body_text,
            )?;
            reasons.push(PROVIDER_STREAMING_TRANSPORT_REASON_REQUEST_PLANNED.to_owned());
            reasons.extend(request.reasons.clone());
            let outcome = transport(&ProviderStreamingTransportRequest {
                turn_id: input.turn_id,
                attempt: attempt.clone(),
                model_tools: input.model_tools.clone(),
                context_composition: input.context_composition,
                cache_shape: cache_shape.clone(),
                request,
                attempt_index,
            });
            if let ProviderStreamingTransportOutcome::Error(error) = &outcome {
                last_transport_error = Some(error.clone());
                if reasoning_effort.downgrade_for(error) {
                    reasons.push(
                        PROVIDER_STREAMING_TRANSPORT_REASON_REASONING_EFFORT_DOWNGRADED.to_owned(),
                    );
                    continue;
                }
                if structured_output.downgrade_for(error) {
                    reasons.push(
                        PROVIDER_STREAMING_TRANSPORT_REASON_STRUCTURED_OUTPUT_DOWNGRADED.to_owned(),
                    );
                    continue;
                }
            }

            let attempt_messages = attempt.messages.clone();
            match runner.complete_attempt(&context, attempt, outcome)? {
                ProviderStreamingRunnerDecision::Completed(runner_report) => {
                    reasoning_effort.remember_success();
                    structured_output.remember_success();
                    reasons.push(PROVIDER_STREAMING_TRANSPORT_REASON_COMPLETED.to_owned());
                    reasons.extend(runner_report.reasons.clone());
                    return Ok(ProviderStreamingTransportResult {
                        report: ProviderStreamingTransportReport {
                            attempt_count: attempt_index,
                            remote_request_body_allowed: false,
                            remote_provider_payload_allowed: false,
                            cache_shape,
                            runner_report,
                            reasons,
                        },
                        final_messages: attempt_messages,
                    });
                }
                ProviderStreamingRunnerDecision::Retry(next_attempt) => {
                    attempt = *next_attempt;
                }
                ProviderStreamingRunnerDecision::Failed(error) => {
                    return Err(AxonError::provider_transport_with_attribution(
                        error.message,
                        error.http_status,
                        error.provider_event_type,
                        error.request_id,
                    ));
                }
            }
        }

        if let Some(error) = last_transport_error {
            return Err(AxonError::provider_transport_with_attribution(
                error.message,
                error.http_status,
                error.provider_event_type,
                error.request_id,
            ));
        }
        Err(AxonError::provider_failure(
            "provider streaming transport 尝试次数超过上限",
        ))
    }
}

fn validate_input(input: &ProviderStreamingTransportInput) -> AxonResult<()> {
    if input.provider_name.trim().is_empty()
        || input.base_url.trim().is_empty()
        || input.model_id.trim().is_empty()
    {
        return Err(AxonError::EmptyInput);
    }
    if input.messages.is_empty() {
        return Err(AxonError::invalid_input(
            "provider streaming transport messages 不能为空",
        ));
    }
    if let Some(websocket_url) = input.websocket_url.as_deref()
        && websocket_url.trim().is_empty()
    {
        return Err(AxonError::invalid_input(
            "provider.websocket_url 不能为空字符串",
        ));
    }
    Ok(())
}

fn runner_context(input: &ProviderStreamingTransportInput) -> ProviderStreamingRunnerContext {
    ProviderStreamingRunnerContext {
        provider_name: input.provider_name.clone(),
        protocol: input.protocol,
        model_id: input.model_id.clone(),
        supports_tools: input.supports_tools,
        context_window: input.context_window,
        supports_vision: input.supports_vision,
        messages: input.messages.clone(),
        model_tools: input.model_tools.clone(),
        context_section_count: input.context_section_count,
        prompt_cache_plan: input.prompt_cache_plan.clone(),
        capabilities: input.capabilities,
        server_conversation_state_ref: input.server_conversation_state_ref.clone(),
    }
}

fn prompt_cache_wire_format(protocol: ProviderStreamProtocol) -> PromptCacheProviderWireFormat {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => PromptCacheProviderWireFormat::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible => {
            PromptCacheProviderWireFormat::OpenAiCompatibleChat
        }
        ProviderStreamProtocol::AnthropicMessages => {
            PromptCacheProviderWireFormat::AnthropicMessages
        }
    }
}

fn request_plan(
    input: &ProviderStreamingTransportInput,
    attempt: &ProviderStreamingAttemptPlan,
    reasoning_effort: Option<&str>,
    structured_output: &ProviderStructuredOutputNegotiation,
) -> AxonResult<ProviderStreamingHttpRequestPlan> {
    let server_conversation_state_ref = (input.wire_transport
        == ProviderStreamingWireTransport::WebSocket)
        .then(|| input.server_conversation_state_ref.clone())
        .flatten();
    ProviderStreamingHttpRequestRuntime.plan_with_channel(
        ProviderStreamingHttpRequestInput {
            provider_name: input.provider_name.clone(),
            protocol: attempt.protocol,
            base_url: input.base_url.clone(),
            model_id: input.model_id.clone(),
            messages: attempt.messages.clone(),
            attachment_archive_root: default_attachment_archive_root(),
            model_tools: if input.supports_tools {
                input.model_tools.clone()
            } else {
                Vec::new()
            },
            prompt_cache_plan: attempt.prompt_cache_plan.clone(),
            api_key_env: input.api_key_env.clone(),
            api_key: input.api_key.clone(),
            service_tier: input.service_tier.clone(),
            reasoning_effort: reasoning_effort.map(str::to_owned),
            reasoning_summary: input.reasoning_summary.clone(),
            structured_output: structured_output.contract(),
            anthropic_max_tokens: input.anthropic_max_tokens,
            capabilities: input.capabilities,
            supports_vision: input.supports_vision,
            server_conversation_state_ref,
        },
        input.client_channel,
    )
}

#[cfg(test)]
#[path = "provider_stream_transport_tests.rs"]
mod tests;
