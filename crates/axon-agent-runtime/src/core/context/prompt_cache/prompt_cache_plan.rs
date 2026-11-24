//! Provider 无关 prompt cache 计划。

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};

use crate::agent_model_tool_history::normalize_provider_history;
use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, ModelToolDefinition, PROMPT_CACHE_KEY_VERSION,
    PROMPT_CACHE_ROLLING_TAIL_BREAKPOINTS, PromptCacheHint, PromptCacheHintTarget,
    PromptCacheProviderWireFormat,
    agent_session_stable_context::{SessionStableContextKind, session_stable_context_kind},
};
use axon_config::ProviderCapabilities;

/// OpenAI Responses 原生 prompt cache key 已下发。
pub const PROMPT_CACHE_PROVIDER_REASON_OPENAI_KEY_APPLIED: &str =
    "prompt_cache_provider:openai_prompt_cache_key_applied";
/// Anthropic cache_control 断点按 provider cache plan 下发。
pub const PROMPT_CACHE_PROVIDER_REASON_ANTHROPIC_BREAKPOINTS_APPLIED: &str =
    "prompt_cache_provider:anthropic_breakpoints_applied";
/// 当前 provider 仅依赖稳定前缀，不支持显式缓存字段。
pub const PROMPT_CACHE_PROVIDER_REASON_STABLE_PREFIX_ONLY: &str =
    "prompt_cache_provider:stable_prefix_only";

/// Provider prompt cache 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPromptCachePlan {
    /// 基础 provider namespace key；OpenAI Responses 映射为 prompt_cache_key。
    pub namespace_key: String,
    /// 基础稳定前缀 hash，用于跨工作区缓存审计和失效判断。
    pub stable_prefix_hash: String,
    /// Soul、User Agents、Personal Memory 与 Config 的独立稳定上下文 hash。
    pub session_stable_context_hash: Option<String>,
    /// 稳定前缀片段。
    pub segments: Vec<PromptCacheSegment>,
    /// 显式缓存断点。
    pub breakpoints: Vec<PromptCacheBreakpoint>,
    /// provider 降级策略。
    pub provider_policy: PromptCacheProviderPolicy,
}

/// Prompt cache 稳定片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheSegment {
    /// 片段类型。
    pub kind: PromptCacheSegmentKind,
    /// 片段标签。
    pub label: String,
    /// 片段 hash。
    pub hash: String,
}

/// Prompt cache 稳定片段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheSegmentKind {
    /// system prompt。
    System,
    /// developer instruction。
    Developer,
    /// tool schema。
    ToolSchema,
    /// Personal Memory content revision。
    PersonalMemory,
    /// Soul revision reference。
    Soul,
    /// Config revision reference。
    Config,
    /// 大上下文稳定摘要。
    LargeContext,
    /// 模型画像。
    ModelProfile,
    /// 全局 User Agents revision reference.
    UserAgents,
}

/// Provider 显式缓存断点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheBreakpoint {
    /// 断点目标。
    pub target: PromptCacheBreakpointTarget,
}

/// Provider 显式缓存断点目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheBreakpointTarget {
    /// 最后一个 system/developer 稳定前缀。
    LastSystemPrefix,
    /// 最后一个 session-stable context segment。
    LastSessionStableContext,
    /// 最后一个 tool schema。
    LastToolSchema,
    /// 末尾最近若干条可缓存消息。
    RecentMessageTail,
}

/// Provider prompt cache 降级策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheProviderPolicy {
    /// OpenAI Responses prompt_cache_key。
    OpenAiResponsesPromptCacheKey,
    /// Anthropic Messages cache_control。
    AnthropicCacheControl,
    /// 当前协议不支持显式 provider cache 字段。
    StablePrefixOnly,
}

/// Provider prompt cache 计划输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPromptCachePlanInput {
    /// provider 名称。
    pub provider_name: String,
    /// Provider 请求历史、缓存和服务端状态能力。
    pub capabilities: ProviderCapabilities,
    /// provider wire 格式。
    pub format: PromptCacheProviderWireFormat,
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
    /// 模型可见工具。
    pub model_tools: Vec<ModelToolDefinition>,
    /// 上下文 section 数量。
    pub context_section_count: usize,
}

/// Provider prompt cache planner。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderPromptCachePlanRuntime;

impl ProviderPromptCachePlanRuntime {
    /// 生成 provider 无关 prompt cache 计划。
    pub fn plan(&self, input: ProviderPromptCachePlanInput) -> AxonResult<ProviderPromptCachePlan> {
        validate_input(&input)?;
        let normalized = normalize_provider_history(&input.messages, input.supports_vision);
        let segments = stable_segments(&input, normalized.messages.as_ref());
        let base_segments = segments
            .iter()
            .filter(|segment| is_base_stable_kind(segment.kind))
            .cloned()
            .collect::<Vec<_>>();
        let stable_prefix_hash = stable_prefix_hash(&base_segments);
        Ok(ProviderPromptCachePlan {
            namespace_key: namespace_key(&stable_prefix_hash),
            stable_prefix_hash,
            session_stable_context_hash: session_stable_context_hash(&segments),
            segments,
            breakpoints: default_breakpoints(&input.capabilities),
            provider_policy: provider_policy(&input),
        })
    }
}

impl ProviderPromptCachePlan {
    /// 从显式断点生成 provider wire hints。
    #[must_use]
    pub fn cache_hints(&self) -> Vec<PromptCacheHint> {
        self.breakpoints
            .iter()
            .map(|breakpoint| PromptCacheHint {
                target: match breakpoint.target {
                    PromptCacheBreakpointTarget::LastToolSchema => {
                        PromptCacheHintTarget::LastToolSchema
                    }
                    PromptCacheBreakpointTarget::LastSystemPrefix => {
                        PromptCacheHintTarget::LastSystemPrefix
                    }
                    PromptCacheBreakpointTarget::LastSessionStableContext => {
                        PromptCacheHintTarget::LastSessionStableContext
                    }
                    PromptCacheBreakpointTarget::RecentMessageTail => {
                        PromptCacheHintTarget::RecentMessageTail
                    }
                },
            })
            .collect()
    }
}

fn validate_input(input: &ProviderPromptCachePlanInput) -> AxonResult<()> {
    if input.provider_name.trim().is_empty() || input.model_id.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    Ok(())
}

fn stable_segments(
    input: &ProviderPromptCachePlanInput,
    messages: &[AgentModelMessage],
) -> Vec<PromptCacheSegment> {
    let mut segments = Vec::new();
    segments.extend(instruction_segments(messages));
    if !input.model_tools.is_empty() {
        segments.push(PromptCacheSegment {
            kind: PromptCacheSegmentKind::ToolSchema,
            label: "model_tools".to_owned(),
            hash: combined_hash(
                input
                    .model_tools
                    .iter()
                    .map(|tool| tool.schema_hash.as_str()),
            ),
        });
    }
    segments.push(PromptCacheSegment {
        kind: PromptCacheSegmentKind::ModelProfile,
        label: "model_profile".to_owned(),
        hash: sha256_hex(model_profile(input)),
    });
    segments
}

fn instruction_segments(messages: &[AgentModelMessage]) -> Vec<PromptCacheSegment> {
    messages
        .iter()
        .filter(|message| !message.cache_excluded)
        .filter_map(|message| {
            stable_message_segment_kind(message).map(|kind| PromptCacheSegment {
                kind,
                label: stable_message_segment_label(message, kind),
                hash: sha256_hex(&message.content),
            })
        })
        .collect()
}

fn stable_message_segment_kind(message: &AgentModelMessage) -> Option<PromptCacheSegmentKind> {
    if let Some(kind) = session_stable_context_kind(message) {
        return Some(match kind {
            SessionStableContextKind::UserAgents => PromptCacheSegmentKind::UserAgents,
            SessionStableContextKind::PersonalMemory => PromptCacheSegmentKind::PersonalMemory,
            SessionStableContextKind::Soul => PromptCacheSegmentKind::Soul,
        });
    }
    if message.role == "user" && cacheable_large_context(&message.content) {
        return Some(PromptCacheSegmentKind::LargeContext);
    }
    match message.role.as_str() {
        "system" => Some(PromptCacheSegmentKind::System),
        "developer" => Some(PromptCacheSegmentKind::Developer),
        _ => None,
    }
}

fn stable_message_segment_label(
    message: &AgentModelMessage,
    kind: PromptCacheSegmentKind,
) -> String {
    match kind {
        PromptCacheSegmentKind::UserAgents => "user_agents".to_owned(),
        PromptCacheSegmentKind::PersonalMemory => "personal_memory".to_owned(),
        PromptCacheSegmentKind::Soul => "soul".to_owned(),
        PromptCacheSegmentKind::Config => "config".to_owned(),
        PromptCacheSegmentKind::LargeContext => "large_context".to_owned(),
        _ => message.role.clone(),
    }
}

fn cacheable_large_context(content: &str) -> bool {
    content.starts_with("<context>")
        || content.starts_with("[Axon Context")
        || content.starts_with("[Axon Prompt Context]")
}

fn is_session_stable_context_kind(kind: PromptCacheSegmentKind) -> bool {
    matches!(
        kind,
        PromptCacheSegmentKind::UserAgents
            | PromptCacheSegmentKind::PersonalMemory
            | PromptCacheSegmentKind::Soul
            | PromptCacheSegmentKind::Config
    )
}

fn is_base_stable_kind(kind: PromptCacheSegmentKind) -> bool {
    matches!(
        kind,
        PromptCacheSegmentKind::System
            | PromptCacheSegmentKind::Developer
            | PromptCacheSegmentKind::ToolSchema
            | PromptCacheSegmentKind::ModelProfile
    )
}

fn session_stable_context_hash(segments: &[PromptCacheSegment]) -> Option<String> {
    let manifest = segments
        .iter()
        .filter(|segment| is_session_stable_context_kind(segment.kind))
        .map(|segment| format!("{:?}:{}:{}", segment.kind, segment.label, segment.hash))
        .collect::<Vec<_>>();
    (!manifest.is_empty()).then(|| sha256_hex(manifest.join("|")))
}

fn model_profile(input: &ProviderPromptCachePlanInput) -> String {
    format!(
        "provider={}|format={:?}|model={}|context_window={}|tools={}|vision={}",
        input.provider_name,
        input.format,
        input.model_id,
        input.context_window,
        input.supports_tools,
        input.supports_vision
    )
}

fn provider_policy(input: &ProviderPromptCachePlanInput) -> PromptCacheProviderPolicy {
    match input.format {
        PromptCacheProviderWireFormat::OpenAiResponses
            if input.capabilities.supports_prompt_cache =>
        {
            PromptCacheProviderPolicy::OpenAiResponsesPromptCacheKey
        }
        PromptCacheProviderWireFormat::AnthropicMessages
            if input.capabilities.supports_explicit_cache_breakpoint =>
        {
            PromptCacheProviderPolicy::AnthropicCacheControl
        }
        _ => PromptCacheProviderPolicy::StablePrefixOnly,
    }
}

fn default_breakpoints(capabilities: &ProviderCapabilities) -> Vec<PromptCacheBreakpoint> {
    if !capabilities.supports_explicit_cache_breakpoint {
        return Vec::new();
    }
    let mut breakpoints = vec![
        PromptCacheBreakpoint {
            target: PromptCacheBreakpointTarget::LastToolSchema,
        },
        PromptCacheBreakpoint {
            target: PromptCacheBreakpointTarget::LastSystemPrefix,
        },
        PromptCacheBreakpoint {
            target: PromptCacheBreakpointTarget::LastSessionStableContext,
        },
    ];
    breakpoints.extend(
        (0..PROMPT_CACHE_ROLLING_TAIL_BREAKPOINTS).map(|_| PromptCacheBreakpoint {
            target: PromptCacheBreakpointTarget::RecentMessageTail,
        }),
    );
    breakpoints
}

fn stable_prefix_hash(segments: &[PromptCacheSegment]) -> String {
    let manifest = segments
        .iter()
        .map(|segment| format!("{:?}:{}:{}", segment.kind, segment.label, segment.hash))
        .collect::<Vec<_>>()
        .join("|");
    sha256_hex(&manifest)
}

fn namespace_key(stable_prefix_hash: &str) -> String {
    sha256_hex(format!(
        "{PROMPT_CACHE_KEY_VERSION}|provider|{stable_prefix_hash}"
    ))
}

fn combined_hash<'a>(hashes: impl Iterator<Item = &'a str>) -> String {
    let values = hashes
        .filter(|hash| !hash.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    sha256_hex(values.join("|"))
}
