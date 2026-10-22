//! Provider streaming runner 合同。
//!
//! 该模块只管理 provider streaming 的协议归一化、上下文溢出恢复计划与审计报告。
//! HTTP 请求、鉴权、超时与具体传输由 CLI / Desktop 等外层适配器负责。

use axon_config::ProviderCapabilities;
use axon_domain::{AxonError, AxonResult};

use crate::{
    AgentModelHistorySnapshot, AgentResponseStreamDelta, ModelToolDefinition, PromptCacheAnchor,
    ProviderPromptCachePlan, ProviderStreamAggregate, ProviderToolCall, TokenUsageTelemetry,
};

#[path = "provider_stream_runner/anchor.rs"]
mod anchor;
#[path = "provider_stream_runner/retry.rs"]
mod retry;

use self::anchor::provider_streaming_anchor;
use self::retry::handle_transport_error;

/// Provider transport 的统一重试策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderStreamingRetryPolicy {
    /// Provider stream 错误的最大重试次数。
    pub stream_max_retries: u32,
    /// WebSocket 连续失败多少次后 sticky fallback 到 HTTP。
    pub websocket_failures_before_fallback: u32,
    /// 指数退避基础毫秒数。
    pub base_delay_ms: u64,
    /// 指数退避最大毫秒数。
    pub max_delay_ms: u64,
    /// 抖动百分比上下界。
    pub jitter_percent: u64,
}

/// 返回 Provider transport 唯一事实源的重试策略。
#[must_use]
pub const fn provider_streaming_retry_policy() -> ProviderStreamingRetryPolicy {
    ProviderStreamingRetryPolicy {
        stream_max_retries: 5,
        websocket_failures_before_fallback: 3,
        base_delay_ms: 250,
        max_delay_ms: 4_000,
        jitter_percent: 10,
    }
}
/// runner 合同版本。
pub const PROVIDER_STREAMING_RUNNER_POLICY_HASH_SOURCE: &str = "axon-provider-streaming-runner-v1";
/// 动态上下文来源。
pub const PROVIDER_STREAMING_RUNNER_SOURCE_REF: &str =
    "axon-agent-runtime:provider-streaming-runner";
/// provider 原始 payload 关闭原因。
pub const PROVIDER_STREAMING_REASON_RAW_PAYLOAD_CLOSED: &str =
    "provider_streaming_runner:raw_payload_closed";
/// 初始请求计划原因。
pub const PROVIDER_STREAMING_REASON_INITIAL_ATTEMPT_PLANNED: &str =
    "provider_streaming_runner:initial_attempt_planned";
/// 请求已归一化原因。
pub const PROVIDER_STREAMING_REASON_STREAM_NORMALIZED: &str =
    "provider_streaming_runner:stream_normalized";
/// provider 错误向外层返回原因。
pub const PROVIDER_STREAMING_REASON_PROVIDER_ERROR: &str =
    "provider_streaming_runner:provider_error";
/// provider 瞬时传输错误重试已规划原因。
pub const PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED: &str =
    "provider_streaming_runner:transient_retry_planned";

/// Provider streaming 协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStreamProtocol {
    /// OpenAI Responses SSE。
    OpenAiResponses,
    /// OpenAI-compatible chat completions SSE。
    OpenAiCompatible,
    /// Anthropic Messages SSE。
    AnthropicMessages,
}

impl ProviderStreamProtocol {
    /// 返回配置与 wire 共同使用的稳定协议编码。
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "openai",
            Self::OpenAiCompatible => "openai-chatcompletions",
            Self::AnthropicMessages => "anthropic",
        }
    }

    /// 从配置中的协议编码解析。
    pub fn from_code(code: &str) -> AxonResult<Self> {
        match code {
            "openai" => Ok(Self::OpenAiResponses),
            "openai-chatcompletions" => Ok(Self::OpenAiCompatible),
            "anthropic" => Ok(Self::AnthropicMessages),
            _ => Err(AxonError::config_invalid(
                "Chat 协议必须为 openai、openai-chatcompletions 或 anthropic",
            )),
        }
    }
}

/// Provider streaming runner 上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingRunnerContext {
    /// provider 名称。
    pub provider_name: String,
    /// Provider 请求历史、缓存和服务端状态能力。
    pub capabilities: ProviderCapabilities,
    /// provider 协议。
    pub protocol: ProviderStreamProtocol,
    /// 模型 ID。
    pub model_id: String,
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
    /// 上下文 section 数量。
    pub context_section_count: usize,
    /// provider prompt cache 计划。
    pub prompt_cache_plan: ProviderPromptCachePlan,
    /// 已完成 Responses 会话的服务端状态引用。
    pub server_conversation_state_ref: Option<String>,
}

/// 单次 provider streaming 尝试计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingAttemptPlan {
    /// provider 协议。
    pub protocol: ProviderStreamProtocol,
    /// 本次尝试应发送的消息。
    pub messages: AgentModelHistorySnapshot,
    /// prompt cache anchor。
    pub anchor: PromptCacheAnchor,
    /// provider prompt cache 计划。
    pub prompt_cache_plan: ProviderPromptCachePlan,
    /// 剩余瞬时错误重试预算。
    pub transient_retry_budget_remaining: u32,
    /// 是否允许远控读取完整 provider payload。
    pub remote_provider_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Provider transport 失败类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStreamingTransportFailureKind {
    /// 用户或上层主动取消。
    Cancelled,
    /// 当前 provider 不支持所选 transport。
    Capability,
    /// TCP/TLS/WebSocket 连接失败或关闭。
    Connection,
    /// 建连、发送或读取超时。
    Timeout,
    /// Provider 限流。
    RateLimited,
    /// Provider 5xx 或过载。
    Upstream,
    /// Provider 明确返回 incomplete，或流在完成标记前结束。
    Incomplete,
    /// 鉴权失败。
    Authentication,
    /// 不可重试的请求参数错误。
    Request,
    /// Provider 明确报告输入超过模型上下文窗口。
    ContextOverflow,
    /// Provider 协议事件或响应不符合合同。
    Protocol,
    /// 无法进一步归类的 transport 错误。
    Unknown,
}

impl ProviderStreamingTransportFailureKind {
    /// 是否允许在当前 provider 内重试。
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Connection
                | Self::Timeout
                | Self::RateLimited
                | Self::Upstream
                | Self::Incomplete
                | Self::Unknown
        )
    }
}

/// provider 传输错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingTransportError {
    /// 错误消息。
    pub message: String,
    /// HTTP 状态码。
    pub http_status: Option<u16>,
    /// 稳定失败类别。
    pub failure_kind: ProviderStreamingTransportFailureKind,
    /// Provider 要求的重试等待时间。
    pub retry_after_ms: Option<u64>,
    /// Provider 或边缘网关返回的请求 ID。
    pub request_id: Option<String>,
    /// 触发失败的 Provider 终态事件类型（如 `response.failed`）；非事件类失败为 None。
    ///
    /// 该字段界定「流内终态失败」：流已经建立、Provider 自己宣告了失败结论。
    /// 这类失败在同渠道重试的收益远低于换渠道，重试策略据此单独限流。
    pub provider_event_type: Option<String>,
}

impl ProviderStreamingTransportError {
    /// 根据状态码和消息创建结构化 transport 错误。
    #[must_use]
    pub fn new(message: impl Into<String>, http_status: Option<u16>) -> Self {
        let message = message.into();
        Self {
            failure_kind: classify_transport_failure(http_status, &message),
            message,
            http_status,
            retry_after_ms: None,
            request_id: None,
            provider_event_type: None,
        }
    }

    /// 覆盖自动推导的失败类别。
    #[must_use]
    pub const fn with_failure_kind(
        mut self,
        failure_kind: ProviderStreamingTransportFailureKind,
    ) -> Self {
        self.failure_kind = failure_kind;
        self
    }

    /// 附加 Provider 指定的重试等待时间。
    #[must_use]
    pub const fn with_retry_after_ms(mut self, retry_after_ms: Option<u64>) -> Self {
        self.retry_after_ms = retry_after_ms;
        self
    }

    /// 附加 Provider 或边缘网关请求 ID。
    #[must_use]
    pub fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    /// 附加触发失败的 Provider 终态事件类型。
    #[must_use]
    pub fn with_provider_event_type(mut self, provider_event_type: Option<String>) -> Self {
        self.provider_event_type = provider_event_type;
        self
    }
}

fn classify_transport_failure(
    http_status: Option<u16>,
    message: &str,
) -> ProviderStreamingTransportFailureKind {
    if is_provider_context_overflow_message(message) {
        return ProviderStreamingTransportFailureKind::ContextOverflow;
    }
    match http_status {
        Some(426) => return ProviderStreamingTransportFailureKind::Capability,
        Some(408) => return ProviderStreamingTransportFailureKind::Timeout,
        Some(429) => return ProviderStreamingTransportFailureKind::RateLimited,
        Some(401 | 403) => return ProviderStreamingTransportFailureKind::Authentication,
        Some(status) if status >= 500 => return ProviderStreamingTransportFailureKind::Upstream,
        Some(status) if (400..500).contains(&status) => {
            return ProviderStreamingTransportFailureKind::Request;
        }
        _ => {}
    }
    let message = message.to_lowercase();
    if message.contains("已取消") || message.contains("cancelled") || message.contains("canceled")
    {
        ProviderStreamingTransportFailureKind::Cancelled
    } else if message.contains("426")
        || message.contains("upgrade required")
        || message.contains("not supported")
    {
        ProviderStreamingTransportFailureKind::Capability
    } else if message.contains("timeout") || message.contains("timed out") {
        ProviderStreamingTransportFailureKind::Timeout
    } else if message.contains("incomplete") || message.contains("缺少完成标记") {
        ProviderStreamingTransportFailureKind::Incomplete
    } else if UPSTREAM_STREAM_ABORTED_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
    {
        // 中继/网关自己宣告「无法继续转发上游流」：上游链路已断，与渠道能力无关。
        ProviderStreamingTransportFailureKind::Upstream
    } else if message.contains("connection")
        || message.contains("connect")
        || message.contains("closed")
        || message.contains("握手失败")
    {
        ProviderStreamingTransportFailureKind::Connection
    } else if message.contains("protocol")
        || message.contains("response.completed")
        || message.contains("previous_response")
        || message.contains("响应读取失败")
    {
        ProviderStreamingTransportFailureKind::Protocol
    } else {
        ProviderStreamingTransportFailureKind::Unknown
    }
}

const PROVIDER_CONTEXT_OVERFLOW_MARKERS: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "context length",
    "context window",
    "input token count exceeds",
    "too many tokens",
    "prompt is too long",
    "input is too long",
];

/// 上游流转发中断的稳定标记；命中即判定为上游侧失败而非请求参数问题。
const UPSTREAM_STREAM_ABORTED_MARKERS: &[&str] = &[
    "forward upstream",
    "upstream stream",
    "upstream connection",
    "stream aborted",
    "无法转发上游",
];

/// 判断 Provider 错误是否明确表示输入超过上下文窗口。
#[must_use]
pub fn is_provider_context_overflow_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    PROVIDER_CONTEXT_OVERFLOW_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
}

/// provider 传输结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStreamingTransportOutcome {
    /// 网络流已收到协议完成标记，等待 adapter 完成结构化聚合。
    StreamCompleted {
        /// HTTP 或逻辑 transport 状态码。
        http_status: u16,
    },
    /// 已完成增量结构化聚合的 Provider 流。
    AggregatedStream {
        /// HTTP 或逻辑 transport 状态码。
        http_status: u16,
        /// 不含原始协议包装的结构化结果。
        stream: Box<ProviderStreamAggregate>,
    },
    /// 传输或 provider 错误。
    Error(ProviderStreamingTransportError),
    /// 本次 transport 已经通过增量回调产生用户可见投影。
    ///
    /// 内层结果仍用于结构化归一化和错误传播，但不得重试或重放请求。
    VisibleOutput(Box<ProviderStreamingTransportOutcome>),
}

impl ProviderStreamingTransportOutcome {
    /// 标记本次 transport 已产生用户可见投影。
    #[must_use]
    pub fn with_visible_output(self) -> Self {
        Self::VisibleOutput(Box::new(self))
    }

    fn into_attempt_outcome(self) -> (Self, bool) {
        match self {
            Self::VisibleOutput(outcome) => (*outcome, true),
            outcome => (outcome, false),
        }
    }
}

/// provider streaming 完成报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingRunnerReport {
    /// 已归一化文本 delta。
    pub deltas: Vec<AgentResponseStreamDelta>,
    /// 聚合文本。
    pub content: String,
    /// Provider 原生 turn 终止语义；缺失时保持未知。
    pub end_turn: Option<bool>,
    /// HTTP 状态码。
    pub http_status: u16,
    /// provider 事件数量。
    pub provider_event_count: usize,
    /// token usage。
    pub token_usage: Option<TokenUsageTelemetry>,
    /// provider reasoning 内容，包含 Anthropic thinking 映射。
    pub reasoning_content: Option<String>,
    /// provider tool-call。
    pub tool_calls: Vec<ProviderToolCall>,
    /// 已完成的 OpenAI Responses 服务端会话状态引用。
    pub server_conversation_state_ref: Option<String>,
    /// 本次流是否被输出 token 上限截断过。
    ///
    /// 与 `end_turn` 分开承载：`end_turn` 只说「这一轮停没停」，而
    /// 「被预算掐断」需要自己的信号，否则运行时就只能把它当成普通失败，
    /// 父代理无法分辨该重试还是该换策略（合同 §5.2.3）。
    pub max_tokens_truncated: bool,
    /// 是否允许远控读取完整 provider payload。
    pub remote_provider_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// provider streaming runner 决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStreamingRunnerDecision {
    /// 请求完成。
    Completed(ProviderStreamingRunnerReport),
    /// 需要用新消息重试。
    Retry(Box<ProviderStreamingAttemptPlan>),
    /// 请求失败。
    Failed(ProviderStreamingTransportError),
}

/// Provider streaming runner runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderStreamingRunnerRuntime;

impl ProviderStreamingRunnerRuntime {
    /// 规划首轮 provider streaming 尝试。
    pub fn plan_initial(
        &self,
        context: &ProviderStreamingRunnerContext,
    ) -> AxonResult<ProviderStreamingAttemptPlan> {
        Ok(ProviderStreamingAttemptPlan {
            protocol: context.protocol,
            messages: context.messages.clone(),
            anchor: provider_streaming_anchor(context)?,
            prompt_cache_plan: context.prompt_cache_plan.clone(),
            transient_retry_budget_remaining: provider_streaming_retry_policy().stream_max_retries,
            remote_provider_payload_allowed: false,
            reasons: vec![
                PROVIDER_STREAMING_REASON_INITIAL_ATTEMPT_PLANNED.to_owned(),
                PROVIDER_STREAMING_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        })
    }

    /// 根据传输结果完成、重试或失败。
    pub fn complete_attempt(
        &self,
        context: &ProviderStreamingRunnerContext,
        plan: ProviderStreamingAttemptPlan,
        outcome: ProviderStreamingTransportOutcome,
    ) -> AxonResult<ProviderStreamingRunnerDecision> {
        let (outcome, visible_output_emitted) = outcome.into_attempt_outcome();
        match outcome {
            ProviderStreamingTransportOutcome::AggregatedStream {
                http_status,
                stream,
            } => Ok(Self::complete_aggregated_stream(plan, http_status, *stream)),
            ProviderStreamingTransportOutcome::StreamCompleted { .. } => {
                unreachable!("网络完成结果必须先由 transport adapter 聚合")
            }
            ProviderStreamingTransportOutcome::Error(error) if visible_output_emitted => {
                Ok(ProviderStreamingRunnerDecision::Failed(error))
            }
            ProviderStreamingTransportOutcome::Error(error) => {
                Ok(handle_transport_error(context, plan, error))
            }
            ProviderStreamingTransportOutcome::VisibleOutput(_) => {
                unreachable!("可见输出标记必须在 attempt 边界解包")
            }
        }
    }

    fn complete_aggregated_stream(
        plan: ProviderStreamingAttemptPlan,
        http_status: u16,
        stream: ProviderStreamAggregate,
    ) -> ProviderStreamingRunnerDecision {
        let content = stream
            .deltas
            .iter()
            .map(|delta| delta.content_delta.as_str())
            .collect::<String>();
        let mut reasons = plan.reasons.clone();
        reasons.push(PROVIDER_STREAMING_REASON_STREAM_NORMALIZED.to_owned());

        ProviderStreamingRunnerDecision::Completed(ProviderStreamingRunnerReport {
            deltas: stream.deltas,
            content,
            end_turn: stream.end_turn,
            http_status,
            provider_event_count: stream.provider_event_count,
            token_usage: stream.token_usage,
            reasoning_content: stream.reasoning_content,
            tool_calls: stream.tool_calls,
            server_conversation_state_ref: stream.server_conversation_state_ref,
            max_tokens_truncated: stream.max_tokens_truncated,
            remote_provider_payload_allowed: false,
            reasons,
        })
    }
}
