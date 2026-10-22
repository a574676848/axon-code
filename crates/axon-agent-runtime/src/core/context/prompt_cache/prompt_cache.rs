//! Prompt cache anchor 运行时。

use axon_config::ProviderCapabilities;
use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};

use crate::{AgentModelHistorySnapshot, ModelToolDefinition};

/// Prompt cache key 版本。
pub const PROMPT_CACHE_KEY_VERSION: &str = "axon-prompt-cache-v1";
/// Prompt cache anchor hash 字符长度。
pub const PROMPT_CACHE_ANCHOR_HASH_LEN: usize = 64;
/// Prompt cache 自动 hint 数量。
pub const PROMPT_CACHE_HINT_COUNT: usize = 3;
/// 滚动缓存断点数量：在末尾最近 N 条非 system 消息上各打 cache_control。
/// 上一轮 tail 断点会成为本轮命中的 READ 前缀（对齐 openclacky 滚动双断点）。
pub const PROMPT_CACHE_ROLLING_TAIL_BREAKPOINTS: usize = 2;
const PROMPT_CACHE_REASON_COLD_START: &str = "cache_cold_start";
const PROMPT_CACHE_REASON_SYSTEM_PROMPT_CHANGED: &str = "system_prompt_changed";
const PROMPT_CACHE_REASON_DEVELOPER_POLICY_CHANGED: &str = "developer_policy_changed";
const PROMPT_CACHE_REASON_TOOL_SCHEMA_CHANGED: &str = "tool_schema_changed";
const PROMPT_CACHE_REASON_SOUL_CHANGED: &str = "soul_changed";
const PROMPT_CACHE_REASON_USER_AGENTS_CHANGED: &str = "user_agents_changed";
const PROMPT_CACHE_REASON_PERSONAL_MEMORY_CHANGED: &str = "personal_memory_changed";
const PROMPT_CACHE_REASON_CONFIG_CHANGED: &str = "config_changed";
const PROMPT_CACHE_REASON_MODEL_PROFILE_CHANGED: &str = "model_profile_changed";
/// Anthropic prompt cache_control 类型。
pub const PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL: &str = "ephemeral";
/// Prompt cache hint 已下发到 provider wire。
pub const PROMPT_CACHE_WIRE_REASON_HINT_APPLIED: &str = "prompt_cache_wire:hint_applied";
/// OpenAI-compatible 请求保持 provider 原生自动缓存，不下发 Anthropic cache_control。
pub const PROMPT_CACHE_WIRE_REASON_OPENAI_UNCHANGED: &str =
    "prompt_cache_wire:openai_compatible_unchanged";
/// system prefix hint 因文本为空而未下发。
pub const PROMPT_CACHE_WIRE_REASON_SYSTEM_EMPTY: &str = "prompt_cache_wire:system_empty";
/// recent message tail hint 因没有可缓存内容块而未下发。
pub const PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE: &str = "prompt_cache_wire:tail_uncacheable";
/// tool schema hint 因本轮没有 provider 可见工具而未下发。
pub const PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY: &str = "prompt_cache_wire:tool_schema_empty";
/// cache_control 断点预算耗尽，后续 hint 不再下发。
pub const PROMPT_CACHE_WIRE_REASON_BREAKPOINT_BUDGET_EXHAUSTED: &str =
    "prompt_cache_wire:breakpoint_budget_exhausted";

/// Prompt cache anchor 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheAnchorInput {
    /// system prompt hash。
    pub system_prompt_hash: String,
    /// developer policy hash。
    pub developer_policy_hash: String,
    /// tool schema hash 列表。
    pub tool_schema_hashes: Vec<String>,
    /// Soul 内容 hash。
    pub soul_hash: String,
    /// 全局 User Agents 内容 hash。
    pub user_agents_hash: String,
    /// 全局 Personal Memory 内容 hash。
    pub personal_memory_hash: String,
    /// 不含 Workspace 地点信息的通用 Config hash。
    pub config_hash: String,
    /// 模型画像 hash。
    pub model_profile_hash: String,
    /// 动态上下文审计引用；不参与 cache key。
    pub dynamic_context: Vec<PromptCacheDynamicContextRef>,
    /// 上一次 anchor，用于计算失效原因。
    pub previous_anchor: Option<PromptCacheAnchor>,
}

/// 动态上下文审计引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheDynamicContextRef {
    /// 本机 source ref。
    pub source_ref: String,
    /// 有界摘要 hash。
    pub summary_hash: String,
    /// token 估算。
    pub token_estimate: u64,
}

/// Prompt cache anchor 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheAnchor {
    /// cache key 版本。
    pub version: String,
    /// 稳定 cache key。
    pub cache_key: String,
    /// 稳定前缀 hash。
    pub stable_prefix_hash: String,
    /// system prompt hash。
    pub system_prompt_hash: String,
    /// developer policy hash。
    pub developer_policy_hash: String,
    /// 归一化 tool schema hash。
    pub tool_schema_hash: String,
    /// Soul 内容 hash。
    pub soul_hash: String,
    /// 全局 User Agents 内容 hash。
    pub user_agents_hash: String,
    /// 全局 Personal Memory 内容 hash。
    pub personal_memory_hash: String,
    /// 不含 Workspace 地点信息的通用 Config hash。
    pub config_hash: String,
    /// 模型画像 hash。
    pub model_profile_hash: String,
    /// 稳定前缀清单。
    pub stable_prefix_manifest: Vec<PromptCacheStablePrefix>,
    /// 动态上下文审计引用。
    pub dynamic_context_audit: Vec<PromptCacheDynamicContextRef>,
    /// 动态上下文是否参与 cache key。
    pub dynamic_context_in_cache_key: bool,
    /// 自动 cache hint。
    pub cache_hints: Vec<PromptCacheHint>,
    /// 相对上一 anchor 的失效原因。
    pub invalidation_reasons: Vec<String>,
}

/// 稳定前缀片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheStablePrefix {
    /// 片段类型。
    pub kind: PromptCacheStablePrefixKind,
    /// 片段标签。
    pub label: String,
    /// 片段 hash。
    pub hash: String,
}

/// 稳定前缀片段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheStablePrefixKind {
    /// system prompt。
    SystemPrompt,
    /// developer policy。
    DeveloperPolicy,
    /// Soul。
    Soul,
    /// 全局 User Agents。
    UserAgents,
    /// 全局 Personal Memory。
    PersonalMemory,
    /// 不含 Workspace 地点信息的通用 Config。
    Config,
    /// tool schema。
    ToolSchema,
    /// 模型画像。
    ModelProfile,
}

/// Prompt cache hint。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheHint {
    /// hint 目标。
    pub target: PromptCacheHintTarget,
}

/// Prompt cache hint 目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheHintTarget {
    /// 最后一个 tool schema。
    LastToolSchema,
    /// 最后一个 system/developer 稳定前缀。
    LastSystemPrefix,
    /// 最后一个 session-stable context segment。
    LastSessionStableContext,
    /// 末尾最近若干条非 system 消息（滚动双断点）。
    RecentMessageTail,
}

/// Prompt cache provider wire 格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheProviderWireFormat {
    /// OpenAI Responses。
    OpenAiResponses,
    /// OpenAI-compatible chat/completions。
    OpenAiCompatibleChat,
    /// Anthropic Messages。
    AnthropicMessages,
}

/// Prompt cache provider wire 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheProviderWireInput {
    /// Provider wire 格式。
    pub format: PromptCacheProviderWireFormat,
    /// Provider 请求历史、缓存和服务端状态能力。
    pub capabilities: ProviderCapabilities,
    /// 当前模型是否支持图片输入。
    pub supports_vision: bool,
    /// 模型请求消息。
    pub messages: AgentModelHistorySnapshot,
    /// 模型可见工具 schema。
    pub model_tools: Vec<ModelToolDefinition>,
    /// Provider prompt cache 计划。
    pub prompt_cache_plan: crate::ProviderPromptCachePlan,
}

/// Prompt cache provider wire 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheProviderWirePlan {
    /// OpenAI-compatible messages。
    pub openai_messages: Vec<serde_json::Value>,
    /// Anthropic system blocks。
    pub anthropic_system: Vec<serde_json::Value>,
    /// Anthropic messages。
    pub anthropic_messages: Vec<serde_json::Value>,
    /// Anthropic tools。
    pub anthropic_tools: Vec<serde_json::Value>,
    /// 已下发的 hint 目标。
    pub applied_hints: Vec<PromptCacheHintTarget>,
    /// 是否允许远控读取 provider 原始 payload。
    pub remote_raw_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Prompt cache anchor runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct PromptCacheAnchorRuntime;

/// Prompt cache provider wire runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct PromptCacheProviderWireRuntime;

impl PromptCacheAnchorRuntime {
    /// 生成稳定 prompt cache anchor。
    pub fn plan(&self, input: PromptCacheAnchorInput) -> AxonResult<PromptCacheAnchor> {
        validate_input(&input)?;

        let tool_schema_hash = combined_hash(&input.tool_schema_hashes);
        let stable_prefix_manifest = stable_prefix_manifest(&input, tool_schema_hash.clone());
        let stable_prefix_hash = stable_prefix_hash(&stable_prefix_manifest);
        let cache_key = cache_key(&stable_prefix_hash);
        let invalidation_reasons =
            invalidation_reasons(input.previous_anchor.as_ref(), &input, &tool_schema_hash);

        Ok(PromptCacheAnchor {
            version: PROMPT_CACHE_KEY_VERSION.to_owned(),
            cache_key,
            stable_prefix_hash,
            system_prompt_hash: input.system_prompt_hash,
            developer_policy_hash: input.developer_policy_hash,
            tool_schema_hash,
            soul_hash: input.soul_hash,
            user_agents_hash: input.user_agents_hash,
            personal_memory_hash: input.personal_memory_hash,
            config_hash: input.config_hash,
            model_profile_hash: input.model_profile_hash,
            stable_prefix_manifest,
            dynamic_context_audit: input.dynamic_context,
            dynamic_context_in_cache_key: false,
            cache_hints: cache_hints(),
            invalidation_reasons,
        })
    }

    /// 返回默认 cache hint 计划。
    #[must_use]
    pub fn default_hints(&self) -> Vec<PromptCacheHint> {
        cache_hints()
    }
}

impl PromptCacheProviderWireRuntime {
    // provider wire 降级实现见 `prompt_cache_wire` 模块（SRP 拆分）。
}

fn validate_input(input: &PromptCacheAnchorInput) -> AxonResult<()> {
    validate_hash(&input.system_prompt_hash, "system prompt hash")?;
    validate_hash(&input.developer_policy_hash, "developer policy hash")?;
    validate_hash(&input.soul_hash, "soul hash")?;
    validate_hash(&input.user_agents_hash, "user agents hash")?;
    validate_hash(&input.personal_memory_hash, "personal memory hash")?;
    validate_hash(&input.config_hash, "config hash")?;
    validate_hash(&input.model_profile_hash, "model profile hash")?;
    for hash in &input.tool_schema_hashes {
        validate_hash(hash, "prompt cache component hash")?;
    }
    for dynamic in &input.dynamic_context {
        if dynamic.source_ref.trim().is_empty() {
            return Err(AxonError::runtime_failure("动态上下文 source ref 不能为空"));
        }
        validate_hash(&dynamic.summary_hash, "dynamic context summary hash")?;
    }
    Ok(())
}

fn validate_hash(value: &str, label: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > PROMPT_CACHE_ANCHOR_HASH_LEN {
        return Err(AxonError::runtime_failure(label));
    }
    Ok(())
}

fn stable_prefix_manifest(
    input: &PromptCacheAnchorInput,
    tool_schema_hash: String,
) -> Vec<PromptCacheStablePrefix> {
    vec![
        stable_prefix(
            PromptCacheStablePrefixKind::SystemPrompt,
            "system_prompt",
            input.system_prompt_hash.clone(),
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::DeveloperPolicy,
            "developer_policy",
            input.developer_policy_hash.clone(),
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::Soul,
            "soul",
            input.soul_hash.clone(),
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::UserAgents,
            "user_agents",
            input.user_agents_hash.clone(),
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::PersonalMemory,
            "personal_memory",
            input.personal_memory_hash.clone(),
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::Config,
            "config",
            input.config_hash.clone(),
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::ToolSchema,
            "tool_schema",
            tool_schema_hash,
        ),
        stable_prefix(
            PromptCacheStablePrefixKind::ModelProfile,
            "model_profile",
            input.model_profile_hash.clone(),
        ),
    ]
}

fn stable_prefix(
    kind: PromptCacheStablePrefixKind,
    label: impl Into<String>,
    hash: String,
) -> PromptCacheStablePrefix {
    PromptCacheStablePrefix {
        kind,
        label: label.into(),
        hash,
    }
}

fn combined_hash(values: &[String]) -> String {
    let mut sorted = values.to_vec();
    sorted.sort();
    hash_text(&sorted.join("|"))
}

fn stable_prefix_hash(manifest: &[PromptCacheStablePrefix]) -> String {
    let payload = manifest
        .iter()
        .map(|entry| format!("{}={}", entry.label, entry.hash))
        .collect::<Vec<_>>()
        .join("|");
    hash_text(&payload)
}

fn cache_key(stable_prefix_hash: &str) -> String {
    hash_text(&format!("{PROMPT_CACHE_KEY_VERSION}|{stable_prefix_hash}"))
}

fn invalidation_reasons(
    previous: Option<&PromptCacheAnchor>,
    input: &PromptCacheAnchorInput,
    tool_schema_hash: &str,
) -> Vec<String> {
    let Some(previous) = previous else {
        return vec![PROMPT_CACHE_REASON_COLD_START.to_owned()];
    };
    let mut reasons = Vec::new();
    push_changed(
        &mut reasons,
        previous.system_prompt_hash != input.system_prompt_hash,
        PROMPT_CACHE_REASON_SYSTEM_PROMPT_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.developer_policy_hash != input.developer_policy_hash,
        PROMPT_CACHE_REASON_DEVELOPER_POLICY_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.soul_hash != input.soul_hash,
        PROMPT_CACHE_REASON_SOUL_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.user_agents_hash != input.user_agents_hash,
        PROMPT_CACHE_REASON_USER_AGENTS_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.personal_memory_hash != input.personal_memory_hash,
        PROMPT_CACHE_REASON_PERSONAL_MEMORY_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.config_hash != input.config_hash,
        PROMPT_CACHE_REASON_CONFIG_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.tool_schema_hash != tool_schema_hash,
        PROMPT_CACHE_REASON_TOOL_SCHEMA_CHANGED,
    );
    push_changed(
        &mut reasons,
        previous.model_profile_hash != input.model_profile_hash,
        PROMPT_CACHE_REASON_MODEL_PROFILE_CHANGED,
    );
    reasons
}

fn push_changed(reasons: &mut Vec<String>, changed: bool, reason: &'static str) {
    if changed {
        reasons.push(reason.to_owned());
    }
}

fn cache_hints() -> Vec<PromptCacheHint> {
    vec![
        PromptCacheHint {
            target: PromptCacheHintTarget::LastToolSchema,
        },
        PromptCacheHint {
            target: PromptCacheHintTarget::LastSystemPrefix,
        },
        PromptCacheHint {
            target: PromptCacheHintTarget::RecentMessageTail,
        },
    ]
}

fn hash_text(value: &str) -> String {
    sha256_hex(value.as_bytes())
}
