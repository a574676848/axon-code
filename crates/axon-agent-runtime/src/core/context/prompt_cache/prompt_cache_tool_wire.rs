//! Provider tool-call 消息的 wire 降级。

use axon_domain::{ToolCallInput, ToolInputFormat};

use crate::{
    AgentModelLocalImageRef, AgentModelMessage, AgentModelToolCallMetadata,
    AgentModelToolResultMetadata, AnthropicThinkingBlockMetadata, ModelToolDefinition,
    provider_custom_tool_name, provider_tool_name,
};

const OPENAI_TEXT_PART_TYPE: &str = "text";
const OPENAI_IMAGE_URL_PART_TYPE: &str = "image_url";
const ANTHROPIC_IMAGE_PART_TYPE: &str = "image";
const ANTHROPIC_BASE64_SOURCE_TYPE: &str = "base64";

pub(crate) fn openai_chat_message(
    message: &AgentModelMessage,
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    openai_message(message, DeveloperRoleLowering::System, model_tools)
}

pub(crate) fn openai_responses_message(
    message: &AgentModelMessage,
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    openai_message(message, DeveloperRoleLowering::Native, model_tools)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeveloperRoleLowering {
    Native,
    System,
}

fn openai_message(
    message: &AgentModelMessage,
    developer_lowering: DeveloperRoleLowering,
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    if let Some(tool_calls) = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_ref())
    {
        let tool_calls = match developer_lowering {
            DeveloperRoleLowering::Native => openai_responses_tool_calls(tool_calls, model_tools),
            DeveloperRoleLowering::System => openai_tool_calls(tool_calls, model_tools),
        };
        let mut value = serde_json::json!({
            "role": "assistant",
            "content": openai_content(message),
            "tool_calls": tool_calls,
        });
        attach_openai_reasoning_content(message, &mut value);
        return value;
    }
    if let Some(result) = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
    {
        return serde_json::json!({
            "role": "tool",
            "tool_call_id": result.call_id,
            "content": result.output,
        });
    }
    let content = openai_content(message);
    let role = openai_role(&message.role, developer_lowering);
    let mut value = serde_json::json!({
        "role": role,
        "content": content,
    });
    attach_openai_reasoning_content(message, &mut value);
    value
}

fn openai_role(role: &str, developer_lowering: DeveloperRoleLowering) -> &str {
    match (role, developer_lowering) {
        ("developer", DeveloperRoleLowering::System) => "system",
        _ => role,
    }
}

pub(crate) fn anthropic_content_blocks(
    message: &AgentModelMessage,
    cache_control: bool,
    model_tools: &[ModelToolDefinition],
) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    let thinking_blocks = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.anthropic_thinking_blocks.as_ref());
    if let Some(thinking_blocks) = thinking_blocks {
        blocks.extend(thinking_blocks.iter().filter_map(anthropic_thinking_block));
    }
    if let Some(tool_uses) = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_ref())
    {
        if let Some(text) = anthropic_visible_text(message) {
            blocks.push(anthropic_text_block(text, cache_control));
        }
        blocks.extend(
            tool_uses
                .iter()
                .map(|tool_use| anthropic_tool_use_block(tool_use, model_tools)),
        );
        return blocks;
    }
    if thinking_blocks.is_some() || !blocks.is_empty() {
        return blocks;
    }
    if let Some(result) = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
    {
        // tool result 属于本轮动态副作用，不能携带 provider cache_control。
        return vec![anthropic_tool_result_block(result, false)];
    }
    anthropic_user_content_blocks(message, cache_control)
}

pub(crate) fn model_visible_items(
    message: &AgentModelMessage,
    format: crate::PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
) -> Vec<serde_json::Value> {
    match format {
        crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat => {
            vec![openai_chat_message(message, model_tools)]
        }
        crate::PromptCacheProviderWireFormat::OpenAiResponses => {
            if message.role == "system" {
                vec![serde_json::json!(message.content)]
            } else {
                openai_responses_visible_items(message, model_tools)
            }
        }
        crate::PromptCacheProviderWireFormat::AnthropicMessages => {
            let blocks = anthropic_content_blocks(message, false, model_tools);
            if matches!(message.role.as_str(), "system" | "developer") {
                blocks
            } else {
                vec![serde_json::json!({
                    "role": anthropic_model_role(&message.role),
                    "content": blocks,
                })]
            }
        }
    }
}

fn openai_responses_visible_items(
    message: &AgentModelMessage,
    model_tools: &[ModelToolDefinition],
) -> Vec<serde_json::Value> {
    let projected = openai_responses_message(message, model_tools);
    let role = projected
        .get("role")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("user");
    if role == "tool" {
        return vec![serde_json::json!({
            "type": "function_call_output",
            "call_id": projected
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            "output": projected.get("content").cloned().unwrap_or(serde_json::Value::Null),
        })];
    }

    let mut items = Vec::new();
    let content = projected.get("content");
    let has_content = content.is_some_and(|value| has_responses_content(value, role));
    if let Some(tool_calls) = projected
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        if has_content {
            let mut value = serde_json::json!({
                "type": "message",
                "role": role,
                "content": responses_content_items(content.unwrap_or(&serde_json::Value::Null), role),
            });
            copy_reasoning_content(&projected, &mut value);
            items.push(value);
        }
        items.extend(tool_calls.iter().filter_map(responses_tool_call_item));
    } else if has_content {
        let mut value = serde_json::json!({
            "type": "message",
            "role": role,
            "content": responses_content_items(content.unwrap_or(&serde_json::Value::Null), role),
        });
        copy_reasoning_content(&projected, &mut value);
        items.push(value);
    }
    if items.is_empty() {
        let mut value = serde_json::json!({
            "type": "message",
            "role": role,
            "content": [{
                "type": if role == "assistant" { "output_text" } else { "input_text" },
                "text": "",
            }],
        });
        copy_reasoning_content(&projected, &mut value);
        items.push(value);
    }
    items
}

fn has_responses_content(content: &serde_json::Value, role: &str) -> bool {
    if let Some(text) = content.as_str() {
        return !text.trim().is_empty();
    }
    if let Some(parts) = content.as_array() {
        return parts.iter().any(|part| {
            match part.get("type").and_then(serde_json::Value::as_str) {
                Some("text") => part
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty()),
                Some(OPENAI_IMAGE_URL_PART_TYPE) if role == "user" => true,
                _ => false,
            }
        });
    }
    !content.is_null()
}

fn responses_content_items(content: &serde_json::Value, role: &str) -> Vec<serde_json::Value> {
    let Some(parts) = content.as_array() else {
        return vec![serde_json::json!({
            "type": if role == "assistant" { "output_text" } else { "input_text" },
            "text": content.as_str().unwrap_or_default(),
        })];
    };
    parts
        .iter()
        .filter_map(|part| match part.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => Some(serde_json::json!({
                "type": if role == "assistant" { "output_text" } else { "input_text" },
                "text": part.get("text").and_then(serde_json::Value::as_str).unwrap_or_default(),
            })),
            Some(OPENAI_IMAGE_URL_PART_TYPE) if role == "user" => part
                .get(OPENAI_IMAGE_URL_PART_TYPE)
                .and_then(|value| value.get("url"))
                .and_then(serde_json::Value::as_str)
                .map(|url| {
                    let mut value = serde_json::json!({
                        "type": "input_image",
                        "image_url": url,
                    });
                    if let Some(metadata) = part.get("axon_media") {
                        value["axon_media"] = metadata.clone();
                    }
                    value
                }),
            _ => None,
        })
        .collect()
}

fn responses_tool_call_item(tool_call: &serde_json::Value) -> Option<serde_json::Value> {
    let call_id = tool_call.get("id").and_then(serde_json::Value::as_str)?;
    if tool_call.get("type").and_then(serde_json::Value::as_str) == Some("custom") {
        return Some(serde_json::json!({
            "type": "custom_tool_call",
            "call_id": call_id,
            "name": tool_call.get("name").and_then(serde_json::Value::as_str).unwrap_or_default(),
            "input": tool_call.get("input").and_then(serde_json::Value::as_str).unwrap_or_default(),
        }));
    }
    let function = tool_call.get("function")?;
    Some(serde_json::json!({
        "type": "function_call",
        "call_id": call_id,
        "name": function.get("name").and_then(serde_json::Value::as_str).unwrap_or_default(),
        "arguments": function.get("arguments").and_then(serde_json::Value::as_str).unwrap_or_default(),
    }))
}

fn copy_reasoning_content(source: &serde_json::Value, target: &mut serde_json::Value) {
    if let Some(reasoning) = source
        .get("reasoning_content")
        .filter(|value| !value.is_null())
    {
        target["reasoning_content"] = reasoning.clone();
    }
}

fn anthropic_model_role(role: &str) -> &str {
    match role {
        "assistant" => "assistant",
        _ => "user",
    }
}

fn anthropic_visible_text(message: &AgentModelMessage) -> Option<&str> {
    (!message.content.trim().is_empty()).then_some(message.content.as_str())
}

fn anthropic_tool_result_block(
    result: &AgentModelToolResultMetadata,
    cache_control: bool,
) -> serde_json::Value {
    let mut block = serde_json::json!({
        "type": "tool_result",
        "tool_use_id": result.call_id,
        "content": result.output,
        "is_error": !result.success,
    });
    if cache_control && cacheable_text(&result.output) {
        block["cache_control"] = serde_json::json!({
            "type": crate::PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
        });
    }
    block
}

fn anthropic_text_block(content: &str, cache_control: bool) -> serde_json::Value {
    if cache_control && cacheable_text(content) {
        return serde_json::json!({
            "type": "text",
            "text": content,
            "cache_control": {"type": crate::PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL},
        });
    }
    serde_json::json!({
        "type": "text",
        "text": content,
    })
}

fn openai_content(message: &AgentModelMessage) -> serde_json::Value {
    if (message.local_images.is_empty() && message.local_files.is_empty()) || message.role != "user"
    {
        return serde_json::json!(message.content);
    }
    let mut parts = vec![serde_json::json!({
        "type": OPENAI_TEXT_PART_TYPE,
        "text": message.content,
    })];
    parts.extend(message.local_images.iter().map(|image| {
        let mut part = serde_json::json!({
            "type": OPENAI_IMAGE_URL_PART_TYPE,
            "image_url": {"url": image.path},
        });
        part["axon_media"] = media_metadata(image);
        part
    }));
    parts.extend(message.local_files.iter().map(file_preview_part));
    serde_json::json!(parts)
}

fn anthropic_user_content_blocks(
    message: &AgentModelMessage,
    cache_control: bool,
) -> Vec<serde_json::Value> {
    let mark_primary_text =
        cache_control && (message.role != "user" || message.local_files.is_empty());
    let mut blocks = vec![anthropic_text_block(&message.content, mark_primary_text)];
    if message.role != "user" {
        return blocks;
    }
    blocks.extend(message.local_images.iter().map(|image| {
        let mut block = serde_json::json!({
            "type": ANTHROPIC_IMAGE_PART_TYPE,
            "source": {
                "type": ANTHROPIC_BASE64_SOURCE_TYPE,
                "media_type": image.mime_type,
                "data": image.path,
            },
        });
        block["axon_media"] = media_metadata(image);
        block
    }));
    let last_file_index = message.local_files.len().checked_sub(1);
    blocks.extend(message.local_files.iter().enumerate().map(|(index, file)| {
        anthropic_text_block(
            &file_preview_text(file),
            cache_control && Some(index) == last_file_index,
        )
    }));
    blocks
}

fn file_preview_part(file: &crate::AgentModelLocalFileRef) -> serde_json::Value {
    serde_json::json!({
        "type": OPENAI_TEXT_PART_TYPE,
        "text": file_preview_text(file),
    })
}

fn file_preview_text(file: &crate::AgentModelLocalFileRef) -> String {
    let preview = crate::bound_provider_file_preview(&file.preview);
    // 路径排在最前：模型要接着读原文件时，第一眼就能拿到可用的绝对路径。
    format!(
        "[文件预览] path={} name={} mime={} bytes={} archiveRef={} sourceRef={}\n{}",
        file.path.as_deref().unwrap_or("unknown"),
        file.name,
        file.mime_type,
        file.byte_len
            .map_or_else(|| "unknown".to_owned(), |value| value.to_string()),
        file.archive_ref.as_deref().unwrap_or("none"),
        file.source_ref.as_deref().unwrap_or("none"),
        preview,
    )
}

fn media_metadata(image: &AgentModelLocalImageRef) -> serde_json::Value {
    serde_json::json!({
        "byteLen": image.byte_len,
        "sha256": image.sha256,
        "mimeType": image.mime_type,
        "archiveRef": image.archive_ref,
        "sourceRef": image.source_ref,
    })
}

fn anthropic_tool_use_block(
    tool_use: &AgentModelToolCallMetadata,
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    let input = function_arguments(&tool_use.input, &tool_use.name, model_tools);
    serde_json::json!({
        "type": "tool_use",
        "id": tool_use.id,
        "name": tool_use.name,
        "input": input,
    })
}

fn anthropic_thinking_block(block: &AnthropicThinkingBlockMetadata) -> Option<serde_json::Value> {
    match block.block_type.as_str() {
        "thinking" => {
            let thinking = block.thinking.as_deref()?.trim();
            let signature = block.signature.as_deref()?.trim();
            if thinking.is_empty() || signature.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "type": "thinking",
                "thinking": thinking,
                "signature": signature,
            }))
        }
        "redacted_thinking" => {
            let data = block.data.as_deref()?.trim();
            if data.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "type": "redacted_thinking",
                "data": data,
            }))
        }
        _ => None,
    }
}

fn cacheable_text(content: &str) -> bool {
    !content.trim().is_empty()
}

fn openai_tool_calls(
    tool_calls: &[AgentModelToolCallMetadata],
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    serde_json::json!(
        tool_calls
            .iter()
            .map(|tool_call| {
                serde_json::json!({
                    "id": tool_call.id,
                    "type": "function",
                    "function": {
                        "name": provider_tool_name(&tool_call.name),
                        "arguments": function_arguments(&tool_call.input, &tool_call.name, model_tools)
                            .to_string(),
                    }
                })
            })
            .collect::<Vec<_>>()
    )
}

fn openai_responses_tool_calls(
    tool_calls: &[AgentModelToolCallMetadata],
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    serde_json::json!(
        tool_calls
            .iter()
            .map(|tool_call| match &tool_call.input {
                ToolCallInput::Text(input) => serde_json::json!({
                    "id": tool_call.id,
                    "type": "custom",
                    "name": provider_custom_tool_name(&tool_call.name),
                    "input": input,
                }),
                ToolCallInput::Json(_) => serde_json::json!({
                    "id": tool_call.id,
                    "type": "function",
                    "function": {
                        "name": provider_tool_name(&tool_call.name),
                        "arguments": function_arguments(
                            &tool_call.input,
                            &tool_call.name,
                            model_tools,
                        )
                        .to_string(),
                    }
                }),
            })
            .collect::<Vec<_>>()
    )
}

fn function_arguments(
    input: &ToolCallInput,
    tool_name: &str,
    model_tools: &[ModelToolDefinition],
) -> serde_json::Value {
    let fallback_field = model_tools
        .iter()
        .find(|tool| {
            tool.qualified_name == tool_name
                || provider_tool_name(&tool.qualified_name) == tool_name
        })
        .and_then(|tool| match &tool.input_format {
            ToolInputFormat::JsonSchema => None,
            ToolInputFormat::Freeform { fallback_field, .. } => Some(fallback_field.as_str()),
        });
    input
        .to_function_arguments(fallback_field)
        .unwrap_or_else(|message| serde_json::json!({"invalid_arguments": message}))
}

fn attach_openai_reasoning_content(message: &AgentModelMessage, value: &mut serde_json::Value) {
    if let Some(reasoning_content) = message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.openai_reasoning_content.as_ref())
    {
        value["reasoning_content"] = serde_json::json!(reasoning_content);
    }
}
