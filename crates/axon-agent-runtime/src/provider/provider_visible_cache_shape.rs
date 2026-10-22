//! Provider 最终可见请求的缓存形状诊断。
//!
//! 仅记录 SHA-256 与消息数量，不保留 prompt、tool schema 或消息正文。

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};
use serde::Serialize;

use crate::ProviderStreamProtocol;

const BODY_MODEL_FIELD: &str = "model";
const BODY_INSTRUCTIONS_FIELD: &str = "instructions";
const BODY_SYSTEM_FIELD: &str = "system";
const BODY_TOOLS_FIELD: &str = "tools";
const BODY_INPUT_FIELD: &str = "input";
const BODY_MESSAGES_FIELD: &str = "messages";
const MESSAGE_ROLE_FIELD: &str = "role";
const MESSAGE_SYSTEM_ROLE: &str = "system";

const METADATA_PROTOCOL_FIELD: &str = "protocol";
const METADATA_MODEL_HASH_FIELD: &str = "modelHash";
const METADATA_SYSTEM_HASH_FIELD: &str = "systemHash";
const METADATA_TOOLS_HASH_FIELD: &str = "toolsHash";
const METADATA_OPTIONS_HASH_FIELD: &str = "optionsHash";
const METADATA_PROMPT_HASH_FIELD: &str = "promptHash";
const METADATA_MESSAGE_COUNT_FIELD: &str = "messageCount";
const METADATA_BREAK_REASON_FIELD: &str = "breakReason";
const METADATA_CACHE_BREAK_REASON_FIELD: &str = "cacheBreakReason";
const METADATA_PROJECTION_BREAK_REASON_FIELD: &str = "projectionBreakReason";
const METADATA_COMMON_PREFIX_MESSAGES_FIELD: &str = "commonPrefixMessages";
const METADATA_PREVIOUS_MESSAGE_COUNT_FIELD: &str = "previousMessageCount";
const METADATA_CURRENT_MESSAGE_COUNT_FIELD: &str = "currentMessageCount";
const METADATA_CANONICAL_PREFIX_HASH_FIELD: &str = "canonicalPrefixHash";
const METADATA_PROJECTION_INPUT_HASH_FIELD: &str = "projectionInputHash";
const METADATA_PROJECTION_OUTPUT_HASH_FIELD: &str = "projectionOutputHash";
const METADATA_COVERED_MESSAGE_COUNT_FIELD: &str = "coveredMessageCount";
const METADATA_SOURCE_TOKENS_FIELD: &str = "sourceTokens";
const METADATA_RESULT_TOKENS_FIELD: &str = "resultTokens";
const METADATA_SAVED_TOKENS_FIELD: &str = "savedTokens";
const METADATA_TRANSCRIPT_GENERATION_FIELD: &str = "transcriptGeneration";
const METADATA_PROJECTION_GENERATION_FIELD: &str = "projectionGeneration";
const METADATA_CONVERSATION_REWRITTEN_FIELD: &str = "conversationRewritten";
const METADATA_MESSAGE_PREFIX_CHANGED_FIELD: &str = "messagePrefixChanged";

/// Provider 可见缓存形状发生变化的首要原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderVisibleCacheBreakReason {
    /// 当前 runtime 尚无上一轮请求。
    FirstRequest,
    /// 消息历史仅在尾部追加。
    HistoryAppended,
    /// Provider 可见缓存形状完全相同。
    ExactReplay,
    /// 压缩 projection 改写了 provider-visible 消息历史。
    CompactionProjection,
    /// Provider 协议发生变化。
    ProtocolChanged,
    /// 模型发生变化。
    ModelChanged,
    /// system/instructions 发生变化。
    SystemChanged,
    /// 工具定义发生变化。
    ToolsChanged,
    /// reasoning、prompt cache key 或其他 provider 能力选项发生变化。
    RequestOptionsChanged,
    /// 当前历史是上一轮历史的截断或清理结果。
    ConversationRewritten,
    /// 既有消息前缀发生变化。
    MessagePrefixChanged,
}

/// 两次 Provider 可见缓存形状的比较结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVisibleCacheShapeComparison {
    /// 首要变化原因。
    pub reason: ProviderVisibleCacheBreakReason,
    /// 两轮消息哈希的最长公共前缀长度。
    pub common_prefix_messages: usize,
    /// 上一轮消息数。
    pub previous_message_count: usize,
    /// 当前轮消息数。
    pub current_message_count: usize,
    /// 当前历史是否比上一轮更短。
    pub conversation_rewritten: bool,
    /// 既有消息公共前缀是否发生变化。
    pub message_prefix_changed: bool,
}

/// 压缩 projection 的隐私安全计量，仅保存数量与 generation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVisibleProjectionMetrics {
    /// 压缩前 provider-visible token 数。
    pub source_tokens: u64,
    /// 压缩后 provider-visible token 数。
    pub result_tokens: u64,
    /// 压缩节省的 token 数。
    pub saved_tokens: u64,
    /// 本次 projection 覆盖的消息数。
    pub covered_message_count: usize,
    /// 压缩前 provider-visible 历史哈希。
    pub projection_input_hash: String,
    /// 压缩后 provider-visible 历史哈希。
    pub projection_output_hash: String,
    /// transcript generation。
    pub transcript_generation: u64,
    /// projection generation。
    pub projection_generation: u64,
}

/// 从最终 Provider 请求体提取的隐私安全缓存形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderVisibleCacheShape {
    /// Provider 请求协议。
    pub protocol: ProviderStreamProtocol,
    /// 模型字段哈希。
    pub model_hash: String,
    /// system/instructions 字段哈希。
    pub system_hash: String,
    /// tools 字段哈希。
    pub tools_hash: String,
    /// reasoning、prompt cache key 与能力选项哈希。
    pub options_hash: String,
    /// 按 Provider 可见顺序排列的非 system 消息哈希。
    pub message_hashes: Vec<String>,
    /// 由上述缓存相关字段构成的聚合哈希。
    pub prompt_hash: String,
    /// canonical provider-visible 消息前缀哈希。
    pub canonical_prefix_hash: String,
    /// 已覆盖的 provider-visible 消息数量。
    pub covered_message_count: usize,
}

impl ProviderVisibleCacheShape {
    /// 从最终序列化请求体提取缓存形状。
    pub fn from_serialized_request(
        protocol: ProviderStreamProtocol,
        body_text: &str,
    ) -> AxonResult<Self> {
        let body: serde_json::Value = serde_json::from_str(body_text)
            .map_err(|_| AxonError::provider_failure("Provider 可见缓存形状无法解析最终请求体"))?;
        let model = required_field(&body, BODY_MODEL_FIELD)?;
        let tools = required_field(&body, BODY_TOOLS_FIELD)?;
        let (system, messages) = visible_sections(protocol, &body)?;
        let model_hash = hash_json(model)?;
        let system_hash = hash_json(&system)?;
        let tools_hash = hash_json(tools)?;
        let options_hash = hash_request_options(&body)?;
        let message_hashes = messages
            .iter()
            .map(hash_json)
            .collect::<AxonResult<Vec<_>>>()?;
        let prompt_hash = hash_json(&serde_json::json!({
            BODY_MODEL_FIELD: model,
            BODY_SYSTEM_FIELD: system,
            BODY_TOOLS_FIELD: tools,
            BODY_MESSAGES_FIELD: messages,
            METADATA_OPTIONS_HASH_FIELD: options_hash,
        }))?;
        let canonical_prefix_hash = hash_json(&serde_json::json!({
            BODY_MODEL_FIELD: model_hash,
            BODY_SYSTEM_FIELD: system_hash,
            BODY_TOOLS_FIELD: tools_hash,
            METADATA_OPTIONS_HASH_FIELD: options_hash,
            METADATA_PROTOCOL_FIELD: protocol_name(protocol),
        }))?;
        let covered_message_count = messages.len();
        Ok(Self {
            protocol,
            model_hash,
            system_hash,
            tools_hash,
            options_hash,
            message_hashes,
            prompt_hash,
            canonical_prefix_hash,
            covered_message_count,
        })
    }

    /// 与上一轮缓存形状比较。
    #[must_use]
    pub fn compare(&self, previous: Option<&Self>) -> ProviderVisibleCacheShapeComparison {
        let Some(previous) = previous else {
            return comparison(ProviderVisibleCacheBreakReason::FirstRequest, 0, 0, self);
        };
        let common_prefix_messages = previous
            .message_hashes
            .iter()
            .zip(&self.message_hashes)
            .take_while(|(left, right)| left == right)
            .count();
        let message_prefix_changed =
            common_prefix_messages < self.message_hashes.len().min(previous.message_hashes.len());
        let conversation_rewritten = previous.message_hashes.len() > self.message_hashes.len()
            || (self.message_hashes.len() <= previous.message_hashes.len()
                && message_prefix_changed);
        let reason = if previous.protocol != self.protocol {
            ProviderVisibleCacheBreakReason::ProtocolChanged
        } else if previous.model_hash != self.model_hash {
            ProviderVisibleCacheBreakReason::ModelChanged
        } else if previous.system_hash != self.system_hash {
            ProviderVisibleCacheBreakReason::SystemChanged
        } else if previous.tools_hash != self.tools_hash {
            ProviderVisibleCacheBreakReason::ToolsChanged
        } else if previous.options_hash != self.options_hash {
            ProviderVisibleCacheBreakReason::RequestOptionsChanged
        } else if previous.message_hashes == self.message_hashes {
            ProviderVisibleCacheBreakReason::ExactReplay
        } else if common_prefix_messages == previous.message_hashes.len()
            && self.message_hashes.len() > previous.message_hashes.len()
        {
            ProviderVisibleCacheBreakReason::HistoryAppended
        } else if common_prefix_messages == self.message_hashes.len()
            && self.message_hashes.len() < previous.message_hashes.len()
        {
            ProviderVisibleCacheBreakReason::ConversationRewritten
        } else {
            ProviderVisibleCacheBreakReason::MessagePrefixChanged
        };
        ProviderVisibleCacheShapeComparison {
            reason,
            common_prefix_messages,
            previous_message_count: previous.message_hashes.len(),
            current_message_count: self.message_hashes.len(),
            conversation_rewritten,
            message_prefix_changed,
        }
    }

    /// 投影为可写入既有 ProviderCompleted 事件的诊断元数据。
    #[must_use]
    pub fn event_metadata(
        &self,
        comparison: &ProviderVisibleCacheShapeComparison,
    ) -> serde_json::Value {
        self.event_metadata_with_projection(comparison, None)
    }

    /// 投影缓存形状并附加最近一次压缩的计量信息。
    #[must_use]
    pub fn event_metadata_with_projection(
        &self,
        comparison: &ProviderVisibleCacheShapeComparison,
        projection: Option<ProviderVisibleProjectionMetrics>,
    ) -> serde_json::Value {
        let projection_break_reason = if projection.is_some()
            && matches!(
                comparison.reason,
                ProviderVisibleCacheBreakReason::ConversationRewritten
                    | ProviderVisibleCacheBreakReason::MessagePrefixChanged
            ) {
            Some(ProviderVisibleCacheBreakReason::CompactionProjection)
        } else {
            None
        };
        let cache_break_reason = projection_break_reason.unwrap_or(comparison.reason);
        let (
            source_tokens,
            result_tokens,
            saved_tokens,
            covered_message_count,
            transcript_generation,
            projection_generation,
        ) = projection.as_ref().map_or(
            (None, None, None, self.covered_message_count, None, None),
            |metrics| {
                (
                    Some(metrics.source_tokens),
                    Some(metrics.result_tokens),
                    Some(metrics.saved_tokens),
                    metrics.covered_message_count,
                    Some(metrics.transcript_generation),
                    Some(metrics.projection_generation),
                )
            },
        );
        serde_json::json!({
            METADATA_PROTOCOL_FIELD: protocol_name(self.protocol),
            METADATA_MODEL_HASH_FIELD: self.model_hash,
            METADATA_SYSTEM_HASH_FIELD: self.system_hash,
            METADATA_TOOLS_HASH_FIELD: self.tools_hash,
            METADATA_OPTIONS_HASH_FIELD: self.options_hash,
            METADATA_PROMPT_HASH_FIELD: self.prompt_hash,
            METADATA_MESSAGE_COUNT_FIELD: self.message_hashes.len(),
            METADATA_BREAK_REASON_FIELD: comparison.reason,
            METADATA_CACHE_BREAK_REASON_FIELD: cache_break_reason,
            METADATA_PROJECTION_BREAK_REASON_FIELD: projection_break_reason,
            METADATA_COMMON_PREFIX_MESSAGES_FIELD: comparison.common_prefix_messages,
            METADATA_PREVIOUS_MESSAGE_COUNT_FIELD: comparison.previous_message_count,
            METADATA_CURRENT_MESSAGE_COUNT_FIELD: comparison.current_message_count,
            METADATA_CONVERSATION_REWRITTEN_FIELD:
                comparison.conversation_rewritten,
            METADATA_MESSAGE_PREFIX_CHANGED_FIELD:
                comparison.message_prefix_changed,
            METADATA_CANONICAL_PREFIX_HASH_FIELD: self.canonical_prefix_hash,
            METADATA_PROJECTION_INPUT_HASH_FIELD: projection
                .as_ref()
                .map(|metrics| metrics.projection_input_hash.clone()),
            METADATA_PROJECTION_OUTPUT_HASH_FIELD: projection
                .as_ref()
                .map(|metrics| metrics.projection_output_hash.clone()),
            METADATA_COVERED_MESSAGE_COUNT_FIELD: covered_message_count,
            METADATA_SOURCE_TOKENS_FIELD: source_tokens,
            METADATA_RESULT_TOKENS_FIELD: result_tokens,
            METADATA_SAVED_TOKENS_FIELD: saved_tokens,
            METADATA_TRANSCRIPT_GENERATION_FIELD: transcript_generation,
            METADATA_PROJECTION_GENERATION_FIELD: projection_generation,
        })
    }
}

fn visible_sections(
    protocol: ProviderStreamProtocol,
    body: &serde_json::Value,
) -> AxonResult<(serde_json::Value, Vec<serde_json::Value>)> {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => Ok((
            body.get(BODY_INSTRUCTIONS_FIELD)
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            required_array(body, BODY_INPUT_FIELD)?.to_vec(),
        )),
        ProviderStreamProtocol::OpenAiCompatible => {
            let messages = required_array(body, BODY_MESSAGES_FIELD)?;
            let system = messages
                .iter()
                .filter(|message| {
                    message
                        .get(MESSAGE_ROLE_FIELD)
                        .and_then(serde_json::Value::as_str)
                        == Some(MESSAGE_SYSTEM_ROLE)
                })
                .cloned()
                .collect::<Vec<_>>();
            let conversation = messages
                .iter()
                .filter(|message| {
                    message
                        .get(MESSAGE_ROLE_FIELD)
                        .and_then(serde_json::Value::as_str)
                        != Some(MESSAGE_SYSTEM_ROLE)
                })
                .cloned()
                .collect();
            Ok((serde_json::Value::Array(system), conversation))
        }
        ProviderStreamProtocol::AnthropicMessages => Ok((
            body.get(BODY_SYSTEM_FIELD)
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            required_array(body, BODY_MESSAGES_FIELD)?.to_vec(),
        )),
    }
}

fn required_field<'a>(
    body: &'a serde_json::Value,
    field: &str,
) -> AxonResult<&'a serde_json::Value> {
    body.get(field).ok_or_else(|| {
        AxonError::provider_failure(format!("Provider 可见缓存形状缺少 {field} 字段"))
    })
}

fn required_array<'a>(
    body: &'a serde_json::Value,
    field: &str,
) -> AxonResult<&'a [serde_json::Value]> {
    required_field(body, field)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| {
            AxonError::provider_failure(format!("Provider 可见缓存形状的 {field} 字段必须为数组"))
        })
}

fn hash_json(value: &serde_json::Value) -> AxonResult<String> {
    serde_json::to_vec(value)
        .map(sha256_hex)
        .map_err(|_| AxonError::provider_failure("Provider 可见缓存形状序列化失败"))
}

fn hash_request_options(body: &serde_json::Value) -> AxonResult<String> {
    let excluded = [
        BODY_MODEL_FIELD,
        BODY_INSTRUCTIONS_FIELD,
        BODY_SYSTEM_FIELD,
        BODY_TOOLS_FIELD,
        BODY_INPUT_FIELD,
        BODY_MESSAGES_FIELD,
        "previous_response_id",
    ];
    let options = body
        .as_object()
        .ok_or_else(|| AxonError::provider_failure("Provider 请求体必须为 JSON 对象"))?
        .iter()
        .filter(|(field, _)| !excluded.contains(&field.as_str()))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    hash_json(&serde_json::Value::Object(options.into_iter().collect()))
}

fn comparison(
    reason: ProviderVisibleCacheBreakReason,
    common_prefix_messages: usize,
    previous_message_count: usize,
    current: &ProviderVisibleCacheShape,
) -> ProviderVisibleCacheShapeComparison {
    ProviderVisibleCacheShapeComparison {
        reason,
        common_prefix_messages,
        previous_message_count,
        current_message_count: current.message_hashes.len(),
        conversation_rewritten: false,
        message_prefix_changed: false,
    }
}

fn protocol_name(protocol: ProviderStreamProtocol) -> &'static str {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => "openai_responses",
        ProviderStreamProtocol::OpenAiCompatible => "openai_compatible",
        ProviderStreamProtocol::AnthropicMessages => "anthropic_messages",
    }
}
