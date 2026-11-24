//! Provider streaming 请求 JSON body 构造。

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_config::normalize_reasoning_effort;
use axon_domain::{AxonError, AxonResult};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, PromptCacheProviderWireFormat,
    PromptCacheProviderWireInput, PromptCacheProviderWireRuntime, ProviderStreamProtocol,
    ProviderStructuredOutputMode,
};

use super::{ProviderStreamingHttpRequestInput, provider_stream_tools};

pub(super) const SERVICE_TIER_DEFAULT: &str = "default";
pub(super) const REASONING_SUMMARY_AUTO: &str = "auto";
pub(super) const OPENAI_REASONING_INCLUDE_ENCRYPTED_CONTENT: &str = "reasoning.encrypted_content";
pub(super) const ANTHROPIC_THINKING_TYPE_ADAPTIVE: &str = "adaptive";
pub(super) const ANTHROPIC_THINKING_DISPLAY_SUMMARIZED: &str = "summarized";
const OPENAI_RESPONSES_TOOL_CHOICE_AUTO: &str = "auto";
const OPENAI_INPUT_TEXT_TYPE: &str = "input_text";
const OPENAI_OUTPUT_TEXT_TYPE: &str = "output_text";
const OPENAI_INPUT_IMAGE_TYPE: &str = "input_image";
const OPENAI_IMAGE_URL_TYPE: &str = "image_url";
const STRUCTURED_OUTPUT_NAME: &str = "axon_output";
const JSON_SCHEMA_FORMAT_TYPE: &str = "json_schema";

pub(super) struct ProviderRequestBody {
    pub(super) body: serde_json::Value,
    pub(super) reasons: Vec<String>,
}

pub(super) fn body_for_protocol(
    input: &ProviderStreamingHttpRequestInput,
) -> AxonResult<ProviderRequestBody> {
    let messages = structured_output_messages(input);
    let wire = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: wire_format(input.protocol),
        capabilities: input.capabilities,
        supports_vision: input.supports_vision,
        messages: crate::annotate_history_item_ids(&messages, input.supports_vision),
        model_tools: input.model_tools.clone(),
        prompt_cache_plan: input.prompt_cache_plan.clone(),
    });
    let body = match input.protocol {
        ProviderStreamProtocol::OpenAiResponses => {
            let mut messages = wire.openai_messages;
            rewrite_openai_image_urls(&mut messages, input.attachment_archive_root.as_deref())?;
            openai_responses_body(input, &messages)
        }
        ProviderStreamProtocol::OpenAiCompatible => {
            let mut messages = wire.openai_messages;
            rewrite_openai_image_urls(&mut messages, input.attachment_archive_root.as_deref())?;
            openai_body(input, &messages)
        }
        ProviderStreamProtocol::AnthropicMessages => {
            let mut messages = wire.anthropic_messages;
            rewrite_anthropic_image_blocks(
                &mut messages,
                input.attachment_archive_root.as_deref(),
            )?;
            let tools = wire.anthropic_tools;
            let mut body = serde_json::json!({
                "model": input.model_id,
                "system": wire.anthropic_system,
                "messages": messages,
                "stream": true,
                "max_tokens": input.anthropic_max_tokens,
                "tools": tools,
            });
            apply_anthropic_reasoning(input, &mut body);
            apply_structured_output(input, &mut body);
            body
        }
    };
    Ok(ProviderRequestBody {
        body,
        reasons: wire.reasons,
    })
}

/// 仅在有工具时声明 tool_choice。
///
/// 部分 OpenAI 兼容层（例如 Gemini/第三方网关）在 `tools` 为空时拒绝携带
/// `tool_choice`，因此无工具请求必须省略该字段。
fn apply_openai_tool_choice(body: &mut serde_json::Value, has_tools: bool) {
    if has_tools {
        body["tool_choice"] = serde_json::json!(OPENAI_RESPONSES_TOOL_CHOICE_AUTO);
    }
}

fn openai_responses_body(
    input: &ProviderStreamingHttpRequestInput,
    messages: &[serde_json::Value],
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": input.model_id,
        "instructions": openai_responses_instructions(messages),
        "input": responses_input(messages),
        "tools": provider_stream_tools::openai_responses_tools(&input.model_tools),
        "parallel_tool_calls": !input.model_tools.is_empty(),
        "store": false,
        "stream": true,
        "include": [],
    });
    apply_openai_tool_choice(&mut body, !input.model_tools.is_empty());
    if input.capabilities.supports_prompt_cache {
        body["prompt_cache_key"] = serde_json::json!(input.prompt_cache_plan.namespace_key);
    }
    if input.capabilities.supports_server_conversation_state
        && let Some(state_ref) = input.server_conversation_state_ref.as_deref()
        && !state_ref.trim().is_empty()
    {
        body["previous_response_id"] = serde_json::json!(state_ref);
    }
    if let Some(service_tier) = service_tier_for_openai_request(input) {
        body["service_tier"] = serde_json::json!(service_tier);
    }
    if let Some(reasoning_effort) = input
        .reasoning_effort
        .as_deref()
        .and_then(normalize_reasoning_effort)
    {
        body["reasoning"] = serde_json::json!({
            "effort": reasoning_effort,
            "summary": non_empty_option(input.reasoning_summary.as_deref())
                .unwrap_or(REASONING_SUMMARY_AUTO),
        });
        body["include"] = serde_json::json!([OPENAI_REASONING_INCLUDE_ENCRYPTED_CONTENT]);
    }
    apply_structured_output(input, &mut body);
    body
}

fn openai_responses_instructions(messages: &[serde_json::Value]) -> String {
    messages
        .iter()
        .filter(|message| message.get("role").and_then(serde_json::Value::as_str) == Some("system"))
        .filter_map(|message| message.get("content").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn openai_body(
    input: &ProviderStreamingHttpRequestInput,
    messages: &[serde_json::Value],
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": input.model_id,
        "messages": messages,
        "tools": provider_stream_tools::openai_tools(&input.model_tools),
        "parallel_tool_calls": !input.model_tools.is_empty(),
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    apply_openai_tool_choice(&mut body, !input.model_tools.is_empty());
    if let Some(service_tier) = service_tier_for_openai_request(input) {
        body["service_tier"] = serde_json::json!(service_tier);
    }
    if let Some(reasoning_effort) = input
        .reasoning_effort
        .as_deref()
        .and_then(normalize_reasoning_effort)
    {
        body["reasoning_effort"] = serde_json::json!(reasoning_effort);
    }
    // 刻意不发 `reasoning_summary`：那是 OpenAI Responses 的字段，Chat Completions
    // 协议里没有它，严格校验的 Provider 会直接 400（真机回执：UNKNOWN_FIELD
    // 未知请求字段 reasoning_summary）。Chat 侧的推理正文走 `reasoning_content` 增量。
    apply_structured_output(input, &mut body);
    body
}

fn responses_input(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let custom_call_ids = messages
        .iter()
        .filter_map(|message| {
            message
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
        })
        .flatten()
        .filter(|call| call.get("type").and_then(serde_json::Value::as_str) == Some("custom"))
        .filter_map(|call| call.get("id").and_then(serde_json::Value::as_str))
        .collect::<std::collections::HashSet<_>>();
    messages
        .iter()
        .filter(|message| message.get("role").and_then(serde_json::Value::as_str) != Some("system"))
        .flat_map(|message| {
            let role = message
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("user");
            let mut items = Vec::new();
            if role != "tool" && has_responses_content(message, role) {
                let mut value = serde_json::json!({
                    "type": "message",
                    "role": role,
                    "content": responses_content(message, role),
                });
                attach_openai_reasoning_content_field(message, &mut value);
                items.push(value);
            }
            if let Some(tool_calls) = message
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
            {
                items.extend(tool_calls.iter().filter_map(responses_tool_call_item));
                return items;
            }
            if role == "tool" {
                let call_id = message
                    .get("tool_call_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let output_type = if custom_call_ids.contains(call_id) {
                    "custom_tool_call_output"
                } else {
                    "function_call_output"
                };
                return vec![serde_json::json!({
                    "type": output_type,
                    "call_id": call_id,
                    "output": message
                        .get("content")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!("")),
                })];
            }
            if !items.is_empty() {
                return items;
            }
            let mut value = serde_json::json!({
                "type": "message",
                "role": role,
                "content": responses_content(message, role),
            });
            attach_openai_reasoning_content_field(message, &mut value);
            vec![value]
        })
        .collect()
}

fn has_responses_content(message: &serde_json::Value, role: &str) -> bool {
    let Some(content) = message.get("content") else {
        return false;
    };
    if let Some(text) = content.as_str() {
        return !text.trim().is_empty();
    }
    if let Some(parts) = content.as_array() {
        return parts
            .iter()
            .any(|part| !responses_content_part(part, role).is_empty());
    }
    !content.is_null()
}

fn responses_tool_call_item(tool_call: &serde_json::Value) -> Option<serde_json::Value> {
    let call_id = tool_call.get("id").and_then(serde_json::Value::as_str)?;
    if tool_call.get("type").and_then(serde_json::Value::as_str) == Some("custom") {
        return Some(serde_json::json!({
            "type": "custom_tool_call",
            "call_id": call_id,
            "name": tool_call
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            "input": tool_call
                .get("input")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            "status": "completed",
        }));
    }
    let function = tool_call.get("function")?;
    Some(serde_json::json!({
        "type": "function_call",
        "call_id": call_id,
        "name": function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        "arguments": function
            .get("arguments")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default(),
        "status": "completed",
    }))
}

fn responses_content(message: &serde_json::Value, role: &str) -> Vec<serde_json::Value> {
    let content = message.get("content").unwrap_or(&serde_json::Value::Null);
    if let Some(parts) = content.as_array() {
        return parts
            .iter()
            .flat_map(|part| responses_content_part(part, role))
            .collect();
    }
    vec![serde_json::json!({
        "type": if role == "assistant" { OPENAI_OUTPUT_TEXT_TYPE } else { OPENAI_INPUT_TEXT_TYPE },
        "text": content.as_str().unwrap_or_default(),
    })]
}

fn responses_content_part(part: &serde_json::Value, role: &str) -> Vec<serde_json::Value> {
    let Some(part_type) = part.get("type").and_then(serde_json::Value::as_str) else {
        return Vec::new();
    };
    match part_type {
        "text" => vec![serde_json::json!({
            "type": if role == "assistant" { OPENAI_OUTPUT_TEXT_TYPE } else { OPENAI_INPUT_TEXT_TYPE },
            "text": part.get("text").and_then(serde_json::Value::as_str).unwrap_or_default(),
        })],
        OPENAI_IMAGE_URL_TYPE if role == "user" => part
            .get(OPENAI_IMAGE_URL_TYPE)
            .and_then(|value| value.get("url"))
            .and_then(serde_json::Value::as_str)
            .map(|url| {
                serde_json::json!({
                    "type": OPENAI_INPUT_IMAGE_TYPE,
                    "image_url": url,
                })
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn service_tier_for_openai_request(input: &ProviderStreamingHttpRequestInput) -> Option<&str> {
    let service_tier = input.service_tier.as_deref()?.trim();
    if service_tier.eq_ignore_ascii_case(SERVICE_TIER_DEFAULT) {
        return None;
    }
    Some(service_tier)
}

fn non_empty_option(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn attach_openai_reasoning_content_field(
    source_message: &serde_json::Value,
    target_message: &mut serde_json::Value,
) {
    if let Some(reasoning_content) = source_message
        .get("reasoning_content")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        target_message["reasoning_content"] = serde_json::json!(reasoning_content);
    }
}

fn image_data_url(
    path: &str,
    metadata: &serde_json::Value,
    archive_root: Option<&Path>,
) -> AxonResult<(String, Option<&'static str>)> {
    let archive_ref = metadata
        .get("archiveRef")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty());
    if archive_ref.is_none()
        && (path.starts_with("data:")
            || path.starts_with("http://")
            || path.starts_with("https://"))
    {
        return Ok((path.to_owned(), None));
    }
    let source_path = match archive_ref {
        Some(archive_ref) => resolve_attachment_archive_path(archive_root, archive_ref)?,
        None => PathBuf::from(path),
    };
    let bytes =
        fs::read(source_path).map_err(|_| AxonError::runtime_failure("本地图片附件读取失败"))?;
    if let Some(expected) = metadata.get("byteLen").and_then(serde_json::Value::as_u64)
        && expected != bytes.len() as u64
    {
        return Err(AxonError::runtime_failure("图片附件字节数校验失败"));
    }
    if let Some(expected) = metadata.get("sha256").and_then(serde_json::Value::as_str)
        && axon_crypto::sha256_hex(&bytes) != expected
    {
        return Err(AxonError::runtime_failure("图片附件 SHA-256 校验失败"));
    }
    let mime_type = image_mime_type_from_signature(&bytes)
        .ok_or_else(|| AxonError::runtime_failure("本地图片附件格式无效或不受支持"))?;
    Ok((
        format!("data:{mime_type};base64,{}", STANDARD.encode(bytes)),
        Some(mime_type),
    ))
}

fn image_mime_type_from_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

fn rewrite_openai_image_urls(
    messages: &mut [serde_json::Value],
    archive_root: Option<&Path>,
) -> AxonResult<()> {
    for message in messages {
        let Some(parts) = message
            .get_mut("content")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for part in parts {
            let metadata = part.get("axon_media").cloned().unwrap_or_default();
            let Some(image_url) = part
                .get_mut(OPENAI_IMAGE_URL_TYPE)
                .and_then(|value| value.get_mut("url"))
            else {
                continue;
            };
            let Some(path) = image_url.as_str().map(ToOwned::to_owned) else {
                continue;
            };
            let (data_url, _) = image_data_url(&path, &metadata, archive_root)?;
            *image_url = serde_json::json!(data_url);
            if let Some(object) = part.as_object_mut() {
                object.remove("axon_media");
            }
        }
    }
    Ok(())
}

fn rewrite_anthropic_image_blocks(
    messages: &mut [serde_json::Value],
    archive_root: Option<&Path>,
) -> AxonResult<()> {
    for message in messages {
        let Some(blocks) = message
            .get_mut("content")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(serde_json::Value::as_str) != Some("image") {
                continue;
            }
            let metadata = block.get("axon_media").cloned().unwrap_or_default();
            let Some(source) = block
                .get_mut("source")
                .and_then(serde_json::Value::as_object_mut)
            else {
                continue;
            };
            let Some(path) = source
                .get("data")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
            else {
                continue;
            };
            let (data_url, detected_mime_type) = image_data_url(&path, &metadata, archive_root)?;
            if let Some(mime_type) = detected_mime_type {
                source.insert("media_type".to_owned(), serde_json::json!(mime_type));
            }
            if let Some((_, base64_payload)) = data_url.split_once(',') {
                source.insert("data".to_owned(), serde_json::json!(base64_payload));
            }
            if let Some(object) = block.as_object_mut() {
                object.remove("axon_media");
            }
        }
    }
    Ok(())
}

fn resolve_attachment_archive_path(
    archive_root: Option<&Path>,
    archive_ref: &str,
) -> AxonResult<PathBuf> {
    let root =
        archive_root.ok_or_else(|| AxonError::runtime_failure("图片附件缺少受控归档根目录"))?;
    let relative = Path::new(archive_ref);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(AxonError::runtime_failure("图片附件归档路径越界"));
    }
    Ok(root.join(relative))
}

fn apply_anthropic_reasoning(
    input: &ProviderStreamingHttpRequestInput,
    body: &mut serde_json::Value,
) {
    if let Some(reasoning_effort) = input
        .reasoning_effort
        .as_deref()
        .and_then(normalize_reasoning_effort)
    {
        body["thinking"] = serde_json::json!({
            "type": ANTHROPIC_THINKING_TYPE_ADAPTIVE,
            "display": ANTHROPIC_THINKING_DISPLAY_SUMMARIZED,
        });
        body["output_config"]["effort"] = serde_json::json!(reasoning_effort);
    }
}

fn apply_structured_output(
    input: &ProviderStreamingHttpRequestInput,
    body: &mut serde_json::Value,
) {
    let Some(contract) = input
        .structured_output
        .as_ref()
        .filter(|contract| contract.mode == ProviderStructuredOutputMode::JsonSchema)
    else {
        return;
    };
    let schema = &contract.schema;
    match input.protocol {
        ProviderStreamProtocol::OpenAiResponses => {
            body["text"]["format"] = serde_json::json!({
                "type": JSON_SCHEMA_FORMAT_TYPE,
                "name": STRUCTURED_OUTPUT_NAME,
                "schema": schema,
                "strict": true,
            });
        }
        ProviderStreamProtocol::OpenAiCompatible => {
            body["response_format"] = serde_json::json!({
                "type": JSON_SCHEMA_FORMAT_TYPE,
                "json_schema": {
                    "name": STRUCTURED_OUTPUT_NAME,
                    "schema": schema,
                    "strict": true,
                },
            });
        }
        ProviderStreamProtocol::AnthropicMessages => {
            body["output_config"]["format"] = serde_json::json!({
                "type": JSON_SCHEMA_FORMAT_TYPE,
                "schema": schema,
            });
        }
    }
}

/// Prompt-only 降级时把 JSON Schema 契约并入 system prompt。
fn structured_output_messages(
    input: &ProviderStreamingHttpRequestInput,
) -> AgentModelHistorySnapshot {
    let mut messages = input.messages.clone();
    let Some(contract) = input
        .structured_output
        .as_ref()
        .filter(|contract| contract.mode == ProviderStructuredOutputMode::PromptOnly)
    else {
        return messages;
    };
    let contract_text = structured_output_prompt_contract(&contract.schema);
    match messages.iter().position(|message| message.role == "system") {
        Some(index) => messages[index].content.push_str(&contract_text),
        None => messages.insert(
            0,
            AgentModelMessage::text("system", contract_text.trim_start().to_owned()),
        ),
    }
    messages
}

fn structured_output_prompt_contract(schema: &serde_json::Value) -> String {
    format!(
        "\n\n当前 Provider 不支持 JSON Schema 严格模式。你必须直接输出一个 JSON 对象，\
         不得输出 Markdown 代码块、前后缀说明或额外文本；该对象必须严格符合以下 JSON Schema：\n{schema}"
    )
}

fn wire_format(protocol: ProviderStreamProtocol) -> PromptCacheProviderWireFormat {
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
