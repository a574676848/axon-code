//! Headless agent run contracts and runtime-owned execution helpers.

use std::fmt;

use axon_client_identity::ClientChannel;
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use serde::Serialize;

use crate::{
    AgentModelMessage, AgentTurnEvent, AgentTurnIterationReport, AgentTurnRuntimeReport,
    ModelToolDefinition, PromptCacheProviderWireFormat, ProviderPromptCachePlanInput,
    ProviderPromptCachePlanRuntime, ProviderStreamProtocol, ProviderStreamingTransportInput,
    ProviderStreamingWireTransport, ProviderStructuredOutputContract, TokenUsageTelemetry,
};

#[path = "agent_run_runtime/failover.rs"]
mod failover;
#[path = "agent_run_runtime/provider_error.rs"]
mod provider_error;

#[cfg(test)]
pub(crate) use self::failover::OUTCOME_VISIBLE_OUTPUT;

pub use self::failover::{
    AgentRunChannelEvent, AgentRunChannelEventKind, AgentRunChannelTransport,
    classify_agent_run_failover_error, run_agent_channel_failover,
};
pub use self::provider_error::{
    agent_run_configuration_error, agent_run_execution_error_from_axon_error,
    agent_run_interrupted_error, agent_run_provider_error_projection,
};

/// Error type used for provider transport failures. These can participate in
/// channel failover classification.
pub const AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT: &str = "provider_transport";
/// Error type used for provider response protocol failures.
pub const AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL: &str = "provider_protocol";
/// Error type used when the local user interrupts an active run.
pub const AGENT_RUN_ERROR_TYPE_INTERRUPTED: &str = "interrupted";
/// Error type used for orchestration/runtime failures.
pub const AGENT_RUN_ERROR_TYPE_RUNTIME: &str = "runtime";
/// Error type used for invalid runtime configuration.
pub const AGENT_RUN_ERROR_TYPE_CONFIGURATION: &str = "configuration";
/// Error type used for Memory persistence failures.
pub const AGENT_RUN_ERROR_TYPE_MEMORY_PERSISTENCE: &str = "memory_persistence";

/// Provider reasoning part ID 前缀。
pub const AGENT_RUN_PROVIDER_REASONING_ID_PREFIX: &str = "provider-reasoning";
/// Provider assistant part ID 前缀。
pub const AGENT_RUN_PROVIDER_ASSISTANT_ID_PREFIX: &str = "provider-assistant";
/// Assistant transcript part ID 前缀。
pub const AGENT_RUN_ASSISTANT_PART_ID_PREFIX: &str = "assistant-part";

/// Single channel attempt audit record for a headless agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunChannelAttempt {
    /// Attempted provider name.
    pub provider: String,
    /// Attempted model id.
    pub model: String,
    /// HTTP status returned by the provider, if available.
    pub http_status: Option<u16>,
    /// Attempt outcome: success / transient_failure / permanent_failure.
    pub outcome: &'static str,
}

/// Successful headless agent run response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunResponse<ToolReceipt> {
    /// Final assistant content returned by the run.
    pub content: String,
    /// Stable transcript part ID for the final assistant content.
    pub assistant_part_id: String,
    /// HTTP status from the successful provider response.
    pub http_status: u16,
    /// Token usage reported by the provider/runtime, if available.
    pub token_usage: Option<TokenUsageTelemetry>,
    /// Tool receipts emitted during the provider tool loop.
    pub tool_receipts: Vec<ToolReceipt>,
    /// Provider-native tool calls emitted during the provider tool loop.
    pub tool_calls: Vec<crate::ProviderToolCall>,
    /// Provider that actually succeeded after failover.
    pub provider: String,
    /// Model id that actually succeeded after failover.
    pub model: String,
    /// Channel attempt audit chain.
    pub channel_attempts: Vec<AgentRunChannelAttempt>,
    /// Provider reasoning parts emitted by the assistant run.
    pub reasoning_messages: Vec<AgentRunReasoningMessage>,
    /// Unified turn runtime events.
    pub turn_events: Vec<AgentTurnEvent>,
    /// 本轮是否被输出 token 上限截断过（任一 step 被截断即为真）。
    pub max_tokens_truncated: bool,
}

/// Provider reasoning content persisted as an assistant transcript part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunReasoningMessage {
    /// Stable transcript part ID.
    pub part_id: String,
    /// Provider reasoning block ID.
    pub reasoning_id: String,
    /// Completed reasoning text.
    pub content: String,
}

impl AgentRunReasoningMessage {
    /// Build the provider reasoning record for a turn.
    #[must_use]
    pub fn provider_turn(
        part_id: impl Into<String>,
        turn_id: u64,
        content: impl Into<String>,
    ) -> Self {
        Self {
            part_id: part_id.into(),
            reasoning_id: agent_run_provider_reasoning_id(turn_id),
            content: content.into(),
        }
    }
}

/// Build the stable provider reasoning ID for a turn.
#[must_use]
pub fn agent_run_provider_reasoning_id(turn_id: u64) -> String {
    format!("{AGENT_RUN_PROVIDER_REASONING_ID_PREFIX}:{turn_id}")
}

/// Build the stable local reasoning transcript part ID for a turn.
#[must_use]
pub fn agent_run_reasoning_part_id(turn_id: u64) -> String {
    format!("reasoning-part:{turn_id}")
}

/// Build the stable local assistant transcript part ID for a turn.
#[must_use]
pub fn agent_run_assistant_part_id(turn_id: u64) -> String {
    format!("{AGENT_RUN_ASSISTANT_PART_ID_PREFIX}:{turn_id}")
}

/// Build the stable provider assistant message ID for a turn.
#[must_use]
pub fn agent_run_provider_assistant_id(turn_id: u64) -> String {
    format!("{AGENT_RUN_PROVIDER_ASSISTANT_ID_PREFIX}:{turn_id}")
}

/// Provider-native error projection for structured run error reporting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunProviderErrorProjection {
    /// Provider-native error type.
    pub error_type: Option<String>,
    /// Provider-native error code.
    pub code: Option<String>,
    /// Provider-native error parameter name.
    pub param: Option<String>,
    /// Human-readable provider error message.
    pub message: String,
}

/// Failed headless agent run response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunExecutionError<ToolReceipt> {
    /// Human-readable run failure message.
    pub message: String,
    /// HTTP status associated with the failure, if available.
    pub http_status: Option<u16>,
    /// Stable runtime error class.
    pub error_type: &'static str,
    /// Provider/runtime-specific error code.
    pub error_code: Option<String>,
    /// 发生 Provider 失败时使用的稳定协议编码。
    pub provider_protocol: Option<String>,
    /// Provider/runtime-specific error parameter.
    pub error_param: Option<String>,
    /// Provider/transport failure kind.
    pub failure_kind: Option<String>,
    /// Provider tool-call index associated with the failure.
    pub call_index: Option<usize>,
    /// Structured provider-native error projection.
    pub provider_error: Option<Box<AgentRunProviderErrorProjection>>,
    /// Token usage reported before the failure, if available.
    pub token_usage: Option<TokenUsageTelemetry>,
    /// Tool receipts emitted before the failure.
    pub tool_receipts: Box<[ToolReceipt]>,
    /// 当前渠道失败前是否已产生用户可见的流式投影。
    ///
    /// 此标记仅用于界定跨渠道重放边界；一旦为 `true`，不得切换渠道重跑完整 turn。
    pub visible_output_emitted: bool,
    /// Channel attempt audit chain for failed runs.
    pub channel_attempts: Vec<AgentRunChannelAttempt>,
    /// 触发失败的 Provider 终态事件类型（如 `response.failed`）；非事件类失败为 None。
    pub provider_event_type: Option<String>,
    /// Provider 或边缘网关返回的请求 ID；上游未回传时为 None。
    pub upstream_request_id: Option<String>,
    /// 失败前是否已提交不可重放的副作用（文件写入等）。
    ///
    /// 与 `visible_output_emitted` 共同界定跨渠道重放边界：有可见输出但无副作用时，
    /// 换渠道续跑仍安全；已经写过文件则必须停下，否则重跑会把副作用做第二遍。
    pub committed_side_effects: bool,
}

impl<ToolReceipt> fmt::Display for AgentRunExecutionError<ToolReceipt> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.http_status {
            Some(status) => write!(
                formatter,
                "agent run failed: HTTP {status}: {}",
                self.message
            ),
            None => write!(formatter, "agent run failed: {}", self.message),
        }
    }
}

/// Static channel failover context for a headless agent run.
pub struct AgentRunFailoverPlan<'a> {
    /// Ordered channel attempt chain.
    pub chain: &'a [(ModelConfig, ProviderConfig)],
    /// Model routing key used for health and affinity state.
    pub routing_key: &'a str,
    /// Session id used for provider affinity.
    pub session_id: &'a str,
    /// Channel balancer settings.
    pub balancer: ChannelBalancerConfig,
    /// Current wall clock in milliseconds.
    pub now_ms: u64,
}

/// Completed channel run response assembly input.
pub struct AgentRunCompletedInput<ToolReceipt> {
    /// Final turn runtime report.
    pub report: AgentTurnRuntimeReport<ToolReceipt>,
    /// Provider that completed the channel run.
    pub provider: String,
    /// Model id that completed the channel run.
    pub model: String,
    /// Turn events after caller-specific enrichment.
    pub turn_events: Vec<AgentTurnEvent>,
}

/// Provider transport input assembly data for a single run channel.
pub struct AgentRunProviderTransportInput<'a> {
    /// 请求入口渠道。
    pub client_channel: ClientChannel,
    /// Channel-specific model config.
    pub model: &'a ModelConfig,
    /// Channel-specific provider config.
    pub provider: &'a ProviderConfig,
    /// Agent-level reasoning effort for this request.
    pub reasoning_effort: Option<String>,
    /// Optional structured output contract for this ordinary coding turn.
    pub structured_output: Option<ProviderStructuredOutputContract>,
    /// Model messages for this request.
    pub messages: Vec<AgentModelMessage>,
    /// Model-visible tool definitions.
    pub model_tools: Vec<ModelToolDefinition>,
    /// Context section count included in the model payload.
    pub context_section_count: usize,
}

/// Validate that a provider uses a stream protocol supported by agent runs.
pub fn validate_agent_run_provider_protocol(provider: &ProviderConfig) -> Result<(), String> {
    ProviderStreamProtocol::from_code(&provider.protocol)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

/// Build provider streaming transport input for a headless run channel.
pub fn agent_run_provider_streaming_transport_input<ToolReceipt>(
    input: AgentRunProviderTransportInput<'_>,
) -> Result<ProviderStreamingTransportInput, Box<AgentRunExecutionError<ToolReceipt>>> {
    let protocol = ProviderStreamProtocol::from_code(&input.provider.protocol)
        .map_err(|error| Box::new(agent_run_configuration_error(format!("{error:?}"))))?;
    let max_output_tokens = input
        .model
        .max_output_tokens
        .unwrap_or(axon_config::DEFAULT_MODEL_MAX_OUTPUT_TOKENS);
    let anthropic_max_tokens = if protocol == ProviderStreamProtocol::AnthropicMessages {
        max_output_tokens
    } else {
        0
    };
    let messages = crate::AgentModelHistorySnapshot::from(input.messages);
    let prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: input.provider.name.clone(),
            capabilities: input.provider.capabilities(),
            format: prompt_cache_wire_format_for_protocol(protocol),
            model_id: input.model.id.clone(),
            context_window: input.model.context_window,
            supports_tools: !input.model.supports_embedding,
            supports_vision: input.model.supports_vision,
            messages: messages.clone(),
            model_tools: input.model_tools.clone(),
            context_section_count: input.context_section_count,
        })
        .map_err(|error| Box::new(agent_run_configuration_error(format!("{error:?}"))))?;

    Ok(ProviderStreamingTransportInput {
        client_channel: input.client_channel,
        turn_id: axon_domain::TurnId(0),
        provider_name: input.provider.name.clone(),
        capabilities: input.provider.capabilities(),
        protocol,
        base_url: input.provider.base_url.clone(),
        api_key_env: input.provider.api_key_env.clone(),
        api_key: input.provider.api_key.clone(),
        model_id: input.model.id.clone(),
        service_tier: None,
        reasoning_effort: input.reasoning_effort,
        reasoning_summary: Some("auto".to_owned()),
        structured_output: input.structured_output,
        context_window: input.model.context_window,
        supports_tools: !input.model.supports_embedding,
        supports_vision: input.model.supports_vision,
        messages,
        model_tools: input.model_tools,
        context_composition: None,
        context_section_count: input.context_section_count,
        prompt_cache_plan,
        anthropic_max_tokens,
        wire_transport: agent_run_wire_transport_from_config(&input.provider.wire_transport)?,
        websocket_url: input.provider.websocket_url.clone(),
        enable_request_compression: input.provider.enable_request_compression,
        server_conversation_state_ref: None,
    })
}

fn prompt_cache_wire_format_for_protocol(
    protocol: ProviderStreamProtocol,
) -> PromptCacheProviderWireFormat {
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

/// Build a headless run response from a completed turn runtime report.
#[must_use]
pub fn agent_run_response_from_turn_report<ToolReceipt>(
    input: AgentRunCompletedInput<ToolReceipt>,
) -> AgentRunResponse<ToolReceipt> {
    let assistant_part_id = agent_run_final_assistant_part_id(&input.report.turn_reports);
    let reasoning_messages = agent_run_reasoning_messages(&input.report.turn_reports);
    let tool_calls = input
        .report
        .turn_reports
        .iter()
        .flat_map(|report| report.transport_report.runner_report.tool_calls.clone())
        .collect();
    AgentRunResponse {
        content: input.report.content,
        assistant_part_id,
        http_status: input.report.http_status,
        token_usage: input.report.token_usage,
        tool_receipts: input.report.tool_receipts,
        tool_calls,
        provider: input.provider,
        model: input.model,
        channel_attempts: Vec::new(),
        reasoning_messages,
        turn_events: input.turn_events,
        max_tokens_truncated: input.report.max_tokens_truncated,
    }
}

fn agent_run_final_assistant_part_id(turn_reports: &[AgentTurnIterationReport]) -> String {
    let turn_id = turn_reports
        .last()
        .map_or(1, |turn_report| turn_report.turn_id.0);
    agent_run_assistant_part_id(turn_id)
}

fn agent_run_reasoning_messages(
    turn_reports: &[AgentTurnIterationReport],
) -> Vec<AgentRunReasoningMessage> {
    turn_reports
        .iter()
        .filter_map(|turn_report| {
            let content = turn_report
                .transport_report
                .runner_report
                .reasoning_content
                .as_deref()?
                .trim();
            (!content.is_empty()).then(|| {
                AgentRunReasoningMessage::provider_turn(
                    agent_run_reasoning_part_id(turn_report.turn_id.0),
                    turn_report.turn_id.0,
                    content.to_owned(),
                )
            })
        })
        .collect()
}

/// Convert provider wire transport config into the runtime transport enum.
pub fn agent_run_wire_transport_from_config<ToolReceipt>(
    value: &str,
) -> Result<ProviderStreamingWireTransport, Box<AgentRunExecutionError<ToolReceipt>>> {
    match value {
        axon_config::PROVIDER_WIRE_TRANSPORT_HTTP => Ok(ProviderStreamingWireTransport::Http),
        axon_config::PROVIDER_WIRE_TRANSPORT_WEBSOCKET => {
            Ok(ProviderStreamingWireTransport::WebSocket)
        }
        _ => Err(Box::new(agent_run_configuration_error(
            "provider.wire_transport 必须为 http 或 websocket",
        ))),
    }
}
