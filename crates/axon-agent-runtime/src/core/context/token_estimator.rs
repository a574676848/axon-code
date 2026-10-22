//! Provider 可见消息的 Codex 风格 token 粗估算。

use crate::{
    AgentModelMessage, ModelToolDefinition, PromptCacheProviderWireFormat,
    agent_model_tool_history::{normalize_provider_history, normalize_provider_message_modalities},
    prompt_cache_tool_wire,
    provider_stream_request::provider_stream_tools,
};

pub(crate) const TOKEN_ESTIMATE_DIVISOR: u64 = 4;
const RESIZED_IMAGE_BYTES_ESTIMATE: u64 = 7_373;
const ENCRYPTED_REASONING_OVERHEAD_BYTES: u64 = 650;
const AXON_MEDIA_MEMBER_PREFIX: &str = ",\"axon_media\":";
const JSON_STRING_QUOTE_BYTES: u64 = 2;

/// 估算消息历史 token 数（按 Provider 可见 item 的 UTF-8 字节数 / 4 向上取整）。
#[must_use]
pub fn estimated_history_tokens(messages: &[AgentModelMessage]) -> u64 {
    estimated_history_tokens_for_provider(
        messages,
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        &[],
    )
}

/// 按指定 Provider wire 格式估算消息历史 token 数。
#[must_use]
pub fn estimated_history_tokens_for_provider(
    messages: &[AgentModelMessage],
    format: PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
) -> u64 {
    estimated_history_tokens_for_provider_with_capabilities(messages, format, model_tools, true)
}

/// 按指定 Provider wire 格式和模型视觉能力估算消息历史 token 数。
#[must_use]
pub fn estimated_history_tokens_for_provider_with_capabilities(
    messages: &[AgentModelMessage],
    format: PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
) -> u64 {
    let normalized = normalize_provider_history(messages, supports_vision);
    normalized.messages.iter().fold(0_u64, |tokens, message| {
        let message_tokens = if supports_vision {
            estimated_message_tokens(message, format, model_tools)
        } else {
            estimated_message_tokens_with_capabilities(
                message,
                format,
                model_tools,
                supports_vision,
            )
        };
        tokens.saturating_add(message_tokens)
    })
}

/// 上下文构成估算：系统提示词、工具定义与对话消息各占多少 token。
///
/// 三个数都是本地粗估算（Provider 可见 item 的 UTF-8 字节 / 4），与 Provider 自己
/// 报出的上下文总量口径天然不完全一致：界面据此展示构成比例，总量仍以 Provider 为准。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextTokenComposition {
    /// 注入的系统提示词与上下文 fragment。
    pub system_prompt_tokens: u64,
    /// 模型可见的工具定义 schema。
    pub tool_definition_tokens: u64,
    /// 真实对话消息（含工具回执）。
    pub conversation_tokens: u64,
}

/// 估算上下文构成。
///
/// 系统提示词按消息来源划分：`AgentModelMessageSource::Context` 是 Axon 注入的初始
/// 上下文与稳定前缀，system/developer 角色同归此列；其余消息都是对话消息。
#[must_use]
pub fn estimate_context_composition(
    messages: &[AgentModelMessage],
    format: PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
) -> ContextTokenComposition {
    let normalized = normalize_provider_history(messages, supports_vision);
    let mut composition = ContextTokenComposition {
        tool_definition_tokens: estimated_tool_definition_tokens(model_tools, format),
        ..ContextTokenComposition::default()
    };
    for message in normalized.messages.iter() {
        let tokens = estimated_message_tokens_with_capabilities(
            message,
            format,
            model_tools,
            supports_vision,
        );
        if is_injected_context_message(message) {
            composition.system_prompt_tokens =
                composition.system_prompt_tokens.saturating_add(tokens);
        } else {
            composition.conversation_tokens =
                composition.conversation_tokens.saturating_add(tokens);
        }
    }
    composition
}

fn is_injected_context_message(message: &AgentModelMessage) -> bool {
    message.source == crate::AgentModelMessageSource::Context
        || matches!(message.role.as_str(), "system" | "developer")
}

/// 估算模型可见工具定义的 token 数。
///
/// 按目标 Provider 的工具 wire 形状序列化后整体折算，与消息估算同一把尺子。
#[must_use]
pub fn estimated_tool_definition_tokens(
    model_tools: &[ModelToolDefinition],
    format: PromptCacheProviderWireFormat,
) -> u64 {
    if model_tools.is_empty() {
        return 0;
    }
    let tools = match format {
        PromptCacheProviderWireFormat::OpenAiResponses => {
            provider_stream_tools::openai_responses_tools(model_tools)
        }
        PromptCacheProviderWireFormat::OpenAiCompatibleChat => {
            provider_stream_tools::openai_tools(model_tools)
        }
        PromptCacheProviderWireFormat::AnthropicMessages => model_tools
            .iter()
            .map(|tool| crate::prompt_cache_wire::anthropic_tool(tool, false))
            .collect(),
    };
    serde_json::to_vec(&tools).map_or(0_u64, |bytes| {
        u64::try_from(bytes.len())
            .unwrap_or(u64::MAX)
            .div_ceil(TOKEN_ESTIMATE_DIVISOR)
    })
}

/// 估算单条消息在指定 Provider 上的可见 token 数。
#[must_use]
pub fn estimated_message_tokens(
    message: &AgentModelMessage,
    format: PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
) -> u64 {
    estimated_message_tokens_with_capabilities(message, format, model_tools, true)
}

pub(crate) fn estimated_message_tokens_with_capabilities(
    message: &AgentModelMessage,
    format: PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
) -> u64 {
    let normalized = normalize_provider_message_modalities(message, supports_vision);
    prompt_cache_tool_wire::model_visible_items(normalized.as_ref(), format, model_tools)
        .iter()
        .fold(0_u64, |tokens, item| {
            tokens.saturating_add(estimate_visible_item_tokens(item))
        })
}

fn estimate_visible_item_tokens(item: &serde_json::Value) -> u64 {
    let raw_bytes = serde_json::to_vec(item).map_or(0_u64, |bytes| {
        u64::try_from(bytes.len()).unwrap_or(u64::MAX)
    });
    let (payload_bytes, replacement_bytes) = media_and_encrypted_adjustment(item);
    raw_bytes
        .saturating_sub(payload_bytes)
        .saturating_add(replacement_bytes)
        .saturating_add(TOKEN_ESTIMATE_DIVISOR - 1)
        / TOKEN_ESTIMATE_DIVISOR
}

fn media_and_encrypted_adjustment(item: &serde_json::Value) -> (u64, u64) {
    let mut payload_bytes = 0_u64;
    let mut replacement_bytes = 0_u64;
    visit_visible_values(item, false, None, &mut |value, encrypted_kind| {
        let Some(text) = value.as_str() else {
            return;
        };
        if let Some(payload) = base64_data_url_payload(text, "image/") {
            payload_bytes = payload_bytes.saturating_add(payload.len() as u64);
            replacement_bytes = replacement_bytes.saturating_add(RESIZED_IMAGE_BYTES_ESTIMATE);
        } else if let Some(kind) = encrypted_kind {
            payload_bytes = payload_bytes.saturating_add(text.len() as u64);
            replacement_bytes = replacement_bytes.saturating_add(match kind {
                EncryptedPayloadKind::Reasoning => text
                    .len()
                    .try_into()
                    .unwrap_or(u64::MAX)
                    .saturating_mul(3)
                    .saturating_div(4)
                    .saturating_sub(ENCRYPTED_REASONING_OVERHEAD_BYTES),
                EncryptedPayloadKind::FunctionOutput => text
                    .len()
                    .try_into()
                    .unwrap_or(u64::MAX)
                    .saturating_mul(9)
                    .div_ceil(16),
            });
        }
    });
    let (image_reference_bytes, image_reference_replacement) = image_reference_adjustment(item);
    payload_bytes = payload_bytes.saturating_add(image_reference_bytes);
    replacement_bytes = replacement_bytes.saturating_add(image_reference_replacement);
    (payload_bytes, replacement_bytes)
}

fn image_reference_adjustment(item: &serde_json::Value) -> (u64, u64) {
    let mut removed_bytes = 0_u64;
    let mut replacement_bytes = 0_u64;
    visit_json_objects(item, &mut |object| {
        let Some(metadata) = object.get("axon_media") else {
            return;
        };
        let Some(mime_type) = metadata
            .get("mimeType")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        let archive_ref_present = metadata
            .get("archiveRef")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.is_empty());
        let (path, anthropic) = match object.get("type").and_then(serde_json::Value::as_str) {
            Some("image_url") => object
                .get("image_url")
                .and_then(|value| value.get("url"))
                .and_then(serde_json::Value::as_str)
                .map(|path| (path, false)),
            Some("input_image") => object
                .get("image_url")
                .and_then(serde_json::Value::as_str)
                .map(|path| (path, false)),
            Some("image") => object
                .get("source")
                .and_then(|value| value.get("data"))
                .and_then(serde_json::Value::as_str)
                .map(|path| (path, true)),
            _ => None,
        }
        .unwrap_or_default();
        if path.is_empty() && !archive_ref_present {
            return;
        }
        let metadata_bytes = json_byte_len(metadata)
            .saturating_add(u64::try_from(AXON_MEDIA_MEMBER_PREFIX.len()).unwrap_or(u64::MAX));
        if path.starts_with("data:") && !archive_ref_present {
            removed_bytes = removed_bytes.saturating_add(metadata_bytes);
            if anthropic && let Some((prefix, _)) = path.split_once(',') {
                removed_bytes = removed_bytes.saturating_add(prefix.len() as u64);
            }
            return;
        }
        if !archive_ref_present && (path.starts_with("http://") || path.starts_with("https://")) {
            removed_bytes = removed_bytes.saturating_add(metadata_bytes);
            return;
        }
        removed_bytes = removed_bytes
            .saturating_add(metadata_bytes)
            .saturating_add(json_byte_len(&path));
        replacement_bytes = replacement_bytes.saturating_add(if anthropic {
            JSON_STRING_QUOTE_BYTES.saturating_add(RESIZED_IMAGE_BYTES_ESTIMATE)
        } else {
            let prefix = format!("data:{mime_type};base64,");
            json_byte_len(&prefix).saturating_add(RESIZED_IMAGE_BYTES_ESTIMATE)
        });
    });
    (removed_bytes, replacement_bytes)
}

fn visit_json_objects(
    value: &serde_json::Value,
    visit: &mut impl FnMut(&serde_json::Map<String, serde_json::Value>),
) {
    match value {
        serde_json::Value::Object(object) => {
            visit(object);
            for child in object.values() {
                visit_json_objects(child, visit);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                visit_json_objects(child, visit);
            }
        }
        _ => {}
    }
}

fn json_byte_len<T: serde::Serialize>(value: &T) -> u64 {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len() as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EncryptedPayloadKind {
    Reasoning,
    FunctionOutput,
}

fn visit_visible_values(
    value: &serde_json::Value,
    function_output_scope: bool,
    encrypted_kind: Option<EncryptedPayloadKind>,
    visit: &mut impl FnMut(&serde_json::Value, Option<EncryptedPayloadKind>),
) {
    match value {
        serde_json::Value::Object(object) => {
            let object_type = object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let function_output_scope = function_output_scope
                || matches!(
                    object_type,
                    "function_call_output" | "custom_tool_call_output"
                );
            let reasoning_scope = matches!(object_type, "redacted_thinking" | "reasoning");
            for (key, child) in object {
                let child_encrypted_kind =
                    if reasoning_scope && matches!(key.as_str(), "data" | "encrypted_content") {
                        Some(EncryptedPayloadKind::Reasoning)
                    } else if function_output_scope && key == "encrypted_content" {
                        Some(EncryptedPayloadKind::FunctionOutput)
                    } else {
                        encrypted_kind
                    };
                visit_visible_values(child, function_output_scope, child_encrypted_kind, visit);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                visit_visible_values(child, function_output_scope, encrypted_kind, visit);
            }
        }
        serde_json::Value::String(_) => visit(value, encrypted_kind),
        _ => {}
    }
}

fn base64_data_url_payload<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let (metadata, payload) = value.split_once(',')?;
    if !metadata
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
    {
        return None;
    }
    let mut parts = metadata[5..].split(';');
    let mime = parts.next().unwrap_or_default();
    if !mime
        .get(..prefix.len())
        .is_some_and(|kind| kind.eq_ignore_ascii_case(prefix))
        || !parts.any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return None;
    }
    Some(payload)
}

#[cfg(test)]
mod tests {
    use super::{estimate_visible_item_tokens, media_and_encrypted_adjustment};
    use crate::{AgentModelMessage, ModelToolDefinition, PromptCacheProviderWireFormat};

    #[test]
    fn ordinary_item_rounds_once_after_serialization() {
        let item = serde_json::json!({"role": "user", "content": "abc"});
        let bytes = serde_json::to_vec(&item)
            .expect("item should serialize")
            .len() as u64;
        assert_eq!(estimate_visible_item_tokens(&item), bytes.div_ceil(4));
    }

    #[test]
    fn encrypted_reasoning_uses_codex_replacement() {
        let encrypted = "A".repeat(1_868);
        let item = serde_json::json!({
            "type": "reasoning",
            "encrypted_content": encrypted,
        });
        let raw = serde_json::to_vec(&item)
            .expect("item should serialize")
            .len() as u64;
        let expected_replacement = (1_868_u64 * 3 / 4).saturating_sub(650);
        let (payload, replacement) = media_and_encrypted_adjustment(&item);
        assert_eq!(payload, 1_868);
        assert_eq!(replacement, expected_replacement);
        assert_eq!(
            estimate_visible_item_tokens(&item),
            (raw - payload + replacement).div_ceil(4)
        );
    }

    #[test]
    fn encrypted_function_output_uses_codex_replacement() {
        let encrypted = "B".repeat(1_868);
        let item = serde_json::json!({
            "type": "function_call_output",
            "encrypted_content": encrypted,
        });
        let (payload, replacement) = media_and_encrypted_adjustment(&item);
        assert_eq!(payload, 1_868);
        assert_eq!(replacement, (1_868_u64 * 9).div_ceil(16));
    }

    #[test]
    fn context_composition_splits_system_prompt_tools_and_conversation() {
        let messages = vec![
            AgentModelMessage::context("system", "注入的系统提示词与工作区约定。".repeat(20)),
            AgentModelMessage::text("user", "用户这一轮的问题。".repeat(20)),
        ];
        let tools = vec![ModelToolDefinition {
            qualified_name: "builtin.shell_command".to_owned(),
            description: "在本机 shell 中运行命令。".to_owned(),
            schema_hash: "tool-schema-shell-command".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "command": { "type": "string" } },
                "required": ["command"]
            }),
            input_format: axon_domain::ToolInputFormat::JsonSchema,
        }];
        let format = PromptCacheProviderWireFormat::OpenAiCompatibleChat;

        let composition = super::estimate_context_composition(&messages, format, &tools, true);

        assert!(composition.system_prompt_tokens > 0);
        assert!(composition.tool_definition_tokens > 0);
        assert!(composition.conversation_tokens > 0);
        assert_eq!(
            composition
                .system_prompt_tokens
                .saturating_add(composition.conversation_tokens),
            super::estimated_history_tokens_for_provider(&messages, format, &tools),
            "两类消息之和必须等于既有历史估算，构成不能凭空多算"
        );
    }

    #[test]
    fn empty_tool_set_costs_nothing() {
        let composition = super::estimate_context_composition(
            &[AgentModelMessage::text("user", "只问一句")],
            PromptCacheProviderWireFormat::OpenAiCompatibleChat,
            &[],
            true,
        );

        assert_eq!(composition.tool_definition_tokens, 0);
        assert_eq!(composition.system_prompt_tokens, 0);
        assert!(composition.conversation_tokens > 0);
    }

    #[test]
    #[ignore = "性能采样：cargo test --release -- --ignored --nocapture"]
    fn measure_composition_cost_on_a_large_history() {
        let tools: Vec<ModelToolDefinition> = (0..28)
            .map(|index| ModelToolDefinition {
                qualified_name: format!("builtin.tool_{index}"),
                description: "工具描述".repeat(40),
                schema_hash: format!("tool-schema-{index}"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "字段说明".repeat(30) },
                        "workdir": { "type": "string" }
                    },
                    "required": ["command"]
                }),
                input_format: axon_domain::ToolInputFormat::JsonSchema,
            })
            .collect();
        let mut messages = vec![AgentModelMessage::context(
            "system",
            "系统提示词".repeat(400),
        )];
        for index in 0..240 {
            if index % 3 == 0 {
                messages.push(AgentModelMessage::tool_result(
                    format!("call-{index}"),
                    "工具回执正文".repeat(400),
                    true,
                ));
            } else {
                messages.push(AgentModelMessage::text("assistant", "助手正文".repeat(400)));
            }
        }
        let bytes = serde_json::to_vec(&messages).map_or(0, |value| value.len());
        let format = PromptCacheProviderWireFormat::OpenAiCompatibleChat;
        let started = std::time::Instant::now();
        let mut total = super::ContextTokenComposition::default();
        for _ in 0..20 {
            total = super::estimate_context_composition(&messages, format, &tools, true);
        }
        let per_call = started.elapsed() / 20;
        eprintln!("history_bytes={bytes} per_call={per_call:?} composition={total:?}");
        assert!(total.conversation_tokens > 0);
        // 只做防呆上界：本机实测 1.4MB 历史加 28 个工具 schema 约 1.1ms，
        // 一旦量级跳变（例如误改成逐条二次遍历）这条采样会立刻变红。
        assert!(
            per_call < std::time::Duration::from_millis(25),
            "构成估算不得退化成秒级开销：{per_call:?}"
        );
    }
}
