//! Provider tool-call 归一化。
//!
//! 该模块只从 provider streaming payload 中提取已完整到达的工具调用，不执行工具、
//! 不读取 provider raw payload，也不直接触发本机副作用。

use std::collections::{BTreeMap, BTreeSet};

use axon_domain::{AxonError, AxonResult, ProviderToolCallIssue, ToolCallInput, ToolInputFormat};
use serde::{Deserialize, Serialize};

use crate::{
    ModelToolDefinition, OPENAI_COMPAT_STREAM_DATA_PREFIX, OPENAI_COMPAT_STREAM_DONE_MARKER,
    ProviderStreamProtocol, provider_stream_error::provider_stream_error_event,
    resolve_provider_tool_name_candidates,
};

/// provider tool-call 归一化完成原因。
pub const PROVIDER_TOOL_CALL_REASON_NORMALIZED: &str = "provider_tool_call:normalized";
/// provider tool-call 原始 payload 关闭原因。
pub const PROVIDER_TOOL_CALL_REASON_RAW_PAYLOAD_CLOSED: &str =
    "provider_tool_call:raw_payload_closed";
/// provider tool-call 参数解析错误字段。
pub const PROVIDER_TOOL_CALL_PARSE_ERROR_FIELD: &str = "__axon_tool_call_parse_error";
/// provider tool-call 参数原文错误字段。
pub const PROVIDER_TOOL_CALL_RAW_ARGUMENTS_FIELD: &str = "__axon_tool_call_raw_arguments";
/// provider tool-call 参数原文最大字符数。
pub const PROVIDER_TOOL_CALL_RAW_ARGUMENTS_MAX_CHARS: usize = 1024;
const OPENAI_COMPAT_TOOL_CALL_ERROR_EVENT: &str =
    "OpenAI-compatible tool-call stream 返回 error 事件";
const ANTHROPIC_TOOL_CALL_ERROR_EVENT: &str = "Anthropic tool-call stream 返回 error 事件";

/// Provider 归一化工具调用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderToolCall {
    /// Provider call ID。
    pub id: String,
    /// 本机规范工具名。
    pub name: String,
    /// 协议无关的工具输入。
    pub input: ToolCallInput,
}

impl ProviderToolCall {
    /// 创建 provider tool-call。
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        input: impl Into<ToolCallInput>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input: input.into(),
        }
    }

    /// 本机执行应使用的工具名。
    #[must_use]
    pub fn dispatch_name(&self) -> &str {
        &self.name
    }

    /// 返回用于日志、缓存和本机解析的稳定输入文本。
    #[must_use]
    pub fn canonical_input(&self) -> String {
        self.input.canonical_text()
    }
}

/// Provider tool-call 归一化报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallReport {
    /// 已归一化工具调用。
    pub tool_calls: Vec<ProviderToolCall>,
    /// 是否允许远控读取 provider raw payload。
    pub remote_raw_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Provider tool-call 归一化 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderToolCallRuntime;

impl ProviderToolCallRuntime {
    /// 从完整 provider stream 中提取工具调用。
    pub fn normalize(
        &self,
        protocol: ProviderStreamProtocol,
        raw_stream: &str,
    ) -> AxonResult<ProviderToolCallReport> {
        self.normalize_with_model_tools(protocol, raw_stream, &[])
    }

    /// 从完整 provider stream 中提取工具调用，并依据模型可见工具 schema 还原 wire 工具名。
    pub fn normalize_with_model_tools(
        &self,
        protocol: ProviderStreamProtocol,
        raw_stream: &str,
        model_tools: &[ModelToolDefinition],
    ) -> AxonResult<ProviderToolCallReport> {
        if raw_stream.trim().is_empty() {
            return Err(AxonError::EmptyInput);
        }
        let mut tool_calls = match protocol {
            ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
                normalize_openai_tool_calls(protocol, raw_stream, model_tools)?
            }
            ProviderStreamProtocol::AnthropicMessages => {
                normalize_anthropic_tool_calls(raw_stream, model_tools)?
            }
        };
        restore_provider_tool_names(&mut tool_calls, model_tools);
        Ok(ProviderToolCallReport {
            tool_calls,
            remote_raw_payload_allowed: false,
            reasons: vec![
                PROVIDER_TOOL_CALL_REASON_NORMALIZED.to_owned(),
                PROVIDER_TOOL_CALL_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        })
    }
}

/// Provider 工具调用在上游协议中的输入编码类型。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProviderToolCallWireKind {
    /// JSON function call。
    #[default]
    Function,
    /// Responses custom/freeform call。
    Custom,
}

#[derive(Debug, Default)]
pub(crate) struct ToolCallAccumulator {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) arguments: String,
    pub(crate) wire_kind: ProviderToolCallWireKind,
}

fn normalize_openai_tool_calls(
    protocol: ProviderStreamProtocol,
    raw_stream: &str,
    model_tools: &[ModelToolDefinition],
) -> AxonResult<Vec<ProviderToolCall>> {
    let mut calls = BTreeMap::<usize, ToolCallAccumulator>::new();
    for line in raw_stream.lines() {
        let Some(payload) = normalize_stream_line(line)? else {
            continue;
        };
        if payload == OPENAI_COMPAT_STREAM_DONE_MARKER {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(payload).map_err(|_| {
            AxonError::provider_invalid_response(
                protocol.as_code(),
                "OpenAI-compatible tool-call JSON 无效",
            )
        })?;
        if is_openai_responses_stream_event(protocol, &value) {
            normalize_openai_responses_tool_call_event(&value, &mut calls)?;
            continue;
        }
        reject_openai_error_event(&value)?;
        let choices = value
            .get("choices")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                AxonError::provider_invalid_response(
                    protocol.as_code(),
                    "OpenAI-compatible tool-call stream 缺少 choices",
                )
            })?;
        for choice in choices {
            let Some(tool_call_deltas) = choice
                .get("delta")
                .and_then(|delta| delta.get("tool_calls"))
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            for tool_call in tool_call_deltas {
                let index = tool_call
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .ok_or_else(|| {
                        AxonError::provider_invalid_response(
                            protocol.as_code(),
                            "OpenAI-compatible tool-call 缺少 index",
                        )
                    })?;
                let entry = calls.entry(index).or_default();
                if let Some(id) = tool_call
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                {
                    entry.id = Some(id.to_owned());
                }
                if let Some(name) = tool_call
                    .pointer("/function/name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                {
                    entry.name = Some(name.to_owned());
                }
                if let Some(arguments) = tool_call
                    .pointer("/function/arguments")
                    .and_then(serde_json::Value::as_str)
                {
                    entry.arguments.push_str(arguments);
                }
            }
        }
    }
    finalize_tool_calls(protocol, calls, model_tools)
}

fn normalize_openai_responses_tool_call_event(
    value: &serde_json::Value,
    calls: &mut BTreeMap<usize, ToolCallAccumulator>,
) -> AxonResult<()> {
    reject_openai_responses_tool_call_error_event(value)?;
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("response.output_item.added" | "response.output_item.done") => {
            collect_openai_responses_tool_call_item(value, calls)?;
        }
        Some("response.function_call_arguments.delta") => {
            let index = openai_responses_output_index(value)?;
            if let Some(delta) = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .filter(|delta| !delta.is_empty())
            {
                let entry = calls.entry(index).or_default();
                entry.wire_kind = ProviderToolCallWireKind::Function;
                entry.arguments.push_str(delta);
            }
        }
        Some("response.custom_tool_call_input.delta") => {
            let index = openai_responses_output_index(value)?;
            if let Some(delta) = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .filter(|delta| !delta.is_empty())
            {
                let entry = calls.entry(index).or_default();
                entry.wire_kind = ProviderToolCallWireKind::Custom;
                entry.arguments.push_str(delta);
            }
        }
        Some("response.function_call_arguments.done") => {
            let index = openai_responses_output_index(value)?;
            if let Some(arguments) = value.get("arguments").and_then(serde_json::Value::as_str) {
                let entry = calls.entry(index).or_default();
                entry.wire_kind = ProviderToolCallWireKind::Function;
                entry.arguments.clear();
                entry.arguments.push_str(arguments);
            }
        }
        Some("response.custom_tool_call_input.done") => {
            let index = openai_responses_output_index(value)?;
            if let Some(input) = value.get("input").and_then(serde_json::Value::as_str) {
                let entry = calls.entry(index).or_default();
                entry.wire_kind = ProviderToolCallWireKind::Custom;
                entry.arguments.clear();
                entry.arguments.push_str(input);
            }
        }
        _ => {}
    }
    Ok(())
}

fn collect_openai_responses_tool_call_item(
    value: &serde_json::Value,
    calls: &mut BTreeMap<usize, ToolCallAccumulator>,
) -> AxonResult<()> {
    let Some(item) = value.get("item") else {
        return Ok(());
    };
    let Some(item_type) = item.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    if !matches!(item_type, "function_call" | "custom_tool_call") {
        return Ok(());
    }
    let index = openai_responses_output_index(value)?;
    let entry = calls.entry(index).or_default();
    entry.wire_kind = if item_type == "custom_tool_call" {
        ProviderToolCallWireKind::Custom
    } else {
        ProviderToolCallWireKind::Function
    };
    if let Some(id) = item
        .get("call_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty())
    {
        entry.id = Some(id.to_owned());
    }
    if let Some(name) = item
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.trim().is_empty())
    {
        entry.name = Some(name.to_owned());
    }
    if let Some(arguments) = item
        .get("arguments")
        .or_else(|| item.get("input"))
        .and_then(serde_json::Value::as_str)
        .filter(|arguments| !arguments.is_empty())
    {
        entry.arguments.clear();
        entry.arguments.push_str(arguments);
    }
    Ok(())
}

fn openai_responses_output_index(value: &serde_json::Value) -> AxonResult<usize> {
    value
        .get("output_index")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::OpenAiResponses.as_code(),
                "OpenAI Responses tool-call 缺少 output_index",
            )
        })
}

fn normalize_anthropic_tool_calls(
    raw_stream: &str,
    model_tools: &[ModelToolDefinition],
) -> AxonResult<Vec<ProviderToolCall>> {
    let mut calls = BTreeMap::<usize, ToolCallAccumulator>::new();
    for line in raw_stream.lines() {
        let Some(payload) = normalize_stream_line(line)? else {
            continue;
        };
        let value: serde_json::Value = serde_json::from_str(payload).map_err(|_| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::AnthropicMessages.as_code(),
                "Anthropic tool-call JSON 无效",
            )
        })?;
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("content_block_start") => {
                let Some(content_block) = value.get("content_block") else {
                    continue;
                };
                if content_block
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    != Some("tool_use")
                {
                    continue;
                }
                let index = content_index(&value)?;
                let entry = calls.entry(index).or_default();
                if let Some(id) = content_block
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                {
                    entry.id = Some(id.to_owned());
                }
                if let Some(name) = content_block
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                {
                    entry.name = Some(name.to_owned());
                }
                if let Some(input) = content_block.get("input") {
                    let input_text = serde_json::to_string(input).map_err(|_| {
                        AxonError::provider_invalid_response(
                            ProviderStreamProtocol::AnthropicMessages.as_code(),
                            "Anthropic tool-call input 序列化失败",
                        )
                    })?;
                    if input_text != "{}" || entry.arguments.is_empty() {
                        entry.arguments = input_text;
                    }
                }
            }
            Some("content_block_delta") => {
                let Some(partial_json) = value
                    .get("delta")
                    .and_then(|delta| {
                        (delta.get("type").and_then(serde_json::Value::as_str)
                            == Some("input_json_delta"))
                        .then_some(delta)
                    })
                    .and_then(|delta| delta.get("partial_json"))
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let index = content_index(&value)?;
                let entry = calls.entry(index).or_default();
                if entry.arguments != "{}"
                    && !entry.arguments.is_empty()
                    && partial_json.trim_start().starts_with('{')
                {
                    entry.arguments.clear();
                }
                if entry.arguments == "{}" {
                    entry.arguments.clear();
                }
                entry.arguments.push_str(partial_json);
            }
            Some("error") => {
                return Err(provider_stream_error_event(
                    ANTHROPIC_TOOL_CALL_ERROR_EVENT,
                    value.get("error").unwrap_or(&value),
                ));
            }
            _ => {}
        }
    }
    finalize_tool_calls(
        ProviderStreamProtocol::AnthropicMessages,
        calls,
        model_tools,
    )
}

pub(crate) fn finalize_tool_calls(
    protocol: ProviderStreamProtocol,
    calls: BTreeMap<usize, ToolCallAccumulator>,
    model_tools: &[ModelToolDefinition],
) -> AxonResult<Vec<ProviderToolCall>> {
    let mut output = Vec::with_capacity(calls.len());
    let mut seen_ids = BTreeSet::new();
    for (call_index, call) in calls {
        let id = call.id.clone().ok_or_else(|| {
            incomplete_tool_call(protocol, call_index, ProviderToolCallIssue::MissingId)
        })?;
        if id.trim().is_empty() {
            return Err(incomplete_tool_call(
                protocol,
                call_index,
                ProviderToolCallIssue::MissingId,
            ));
        }
        if !seen_ids.insert(id.clone()) {
            return Err(AxonError::provider_invalid_response(
                protocol.as_code(),
                "provider tool-call id 重复",
            ));
        }
        let name = call.name.clone().ok_or_else(|| {
            incomplete_tool_call(protocol, call_index, ProviderToolCallIssue::MissingName)
        })?;
        if name.trim().is_empty() {
            return Err(incomplete_tool_call(
                protocol,
                call_index,
                ProviderToolCallIssue::MissingName,
            ));
        }
        if call.arguments.trim().is_empty() && call.wire_kind == ProviderToolCallWireKind::Function
        {
            output.push(ProviderToolCall::new(
                id,
                name,
                ToolCallInput::json(recoverable_arguments(
                    ProviderToolCallIssue::EmptyArguments,
                    &call.arguments,
                )),
            ));
            continue;
        }
        let input = normalize_tool_call_input(&name, &call, model_tools);
        output.push(ProviderToolCall::new(id, name, input));
    }
    Ok(output)
}

fn normalize_tool_call_input(
    name: &str,
    call: &ToolCallAccumulator,
    model_tools: &[ModelToolDefinition],
) -> ToolCallInput {
    if call.wire_kind == ProviderToolCallWireKind::Custom {
        return ToolCallInput::text(call.arguments.clone());
    }
    let arguments = match serde_json::from_str::<serde_json::Value>(&call.arguments) {
        Ok(arguments) => arguments,
        Err(_) => {
            return ToolCallInput::json(recoverable_arguments(
                ProviderToolCallIssue::MalformedArguments,
                &call.arguments,
            ));
        }
    };
    let Some(tool) = resolved_model_tool(name, model_tools) else {
        return ToolCallInput::json(arguments);
    };
    match &tool.input_format {
        ToolInputFormat::JsonSchema => ToolCallInput::json(arguments),
        ToolInputFormat::Freeform { fallback_field, .. } => {
            let text = arguments
                .get(fallback_field)
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    (fallback_field == "input")
                        .then(|| arguments.get("code_edit"))
                        .flatten()
                        .and_then(serde_json::Value::as_str)
                });
            text.map_or_else(
                || {
                    ToolCallInput::json(recoverable_arguments(
                        ProviderToolCallIssue::MalformedArguments,
                        &call.arguments,
                    ))
                },
                ToolCallInput::text,
            )
        }
    }
}

fn resolved_model_tool<'a>(
    name: &str,
    model_tools: &'a [ModelToolDefinition],
) -> Option<&'a ModelToolDefinition> {
    let candidates = model_tools
        .iter()
        .map(|tool| tool.qualified_name.clone())
        .collect::<Vec<_>>();
    let resolution = resolve_provider_tool_name_candidates(&candidates, name);
    model_tools
        .iter()
        .find(|tool| tool.qualified_name == resolution.canonical_name)
}

/// 判断 tool-call 参数是否是不可执行但可回传模型修正的解析错误对象。
pub fn is_recoverable_tool_call_input(input: &ToolCallInput) -> bool {
    input
        .as_json()
        .and_then(|value| value.get(PROVIDER_TOOL_CALL_PARSE_ERROR_FIELD))
        .is_some()
}

fn recoverable_arguments(issue: ProviderToolCallIssue, raw_arguments: &str) -> serde_json::Value {
    let raw_arguments = raw_arguments
        .chars()
        .take(PROVIDER_TOOL_CALL_RAW_ARGUMENTS_MAX_CHARS)
        .collect::<String>();
    serde_json::json!({
        PROVIDER_TOOL_CALL_PARSE_ERROR_FIELD: issue.as_str(),
        PROVIDER_TOOL_CALL_RAW_ARGUMENTS_FIELD: raw_arguments,
    })
}

fn incomplete_tool_call(
    protocol: ProviderStreamProtocol,
    call_index: usize,
    issue: ProviderToolCallIssue,
) -> AxonError {
    AxonError::provider_tool_call_incomplete(protocol.as_code(), call_index, issue)
}

pub(crate) fn restore_provider_tool_names(
    tool_calls: &mut [ProviderToolCall],
    model_tools: &[ModelToolDefinition],
) {
    let candidates = model_tools
        .iter()
        .map(|tool| tool.qualified_name.clone())
        .collect::<Vec<_>>();
    for tool_call in tool_calls {
        tool_call.name =
            resolve_provider_tool_name_candidates(&candidates, &tool_call.name).canonical_name;
    }
}

fn content_index(value: &serde_json::Value) -> AxonResult<usize> {
    value
        .get("index")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::AnthropicMessages.as_code(),
                "Anthropic tool-call 缺少 index",
            )
        })
}

fn reject_openai_error_event(value: &serde_json::Value) -> AxonResult<()> {
    if let Some(error) = value.get("error") {
        return Err(provider_stream_error_event(
            OPENAI_COMPAT_TOOL_CALL_ERROR_EVENT,
            error,
        ));
    }
    Ok(())
}

fn reject_openai_responses_tool_call_error_event(value: &serde_json::Value) -> AxonResult<()> {
    if value.get("type").and_then(serde_json::Value::as_str) == Some("response.failed") {
        return Err(provider_stream_error_event(
            OPENAI_COMPAT_TOOL_CALL_ERROR_EVENT,
            value.pointer("/response/error").unwrap_or(value),
        ));
    }
    Ok(())
}

fn is_openai_responses_stream_event(
    protocol: ProviderStreamProtocol,
    value: &serde_json::Value,
) -> bool {
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return false;
    };
    kind.starts_with("response.") || protocol == ProviderStreamProtocol::OpenAiResponses
}

fn normalize_stream_line(line: &str) -> AxonResult<Option<&str>> {
    let trimmed = line.trim();
    // 跳过空行、注释行和 SSE event 行
    if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
        return Ok(None);
    }
    // 标准 SSE data 行
    if let Some(payload) = trimmed.strip_prefix(OPENAI_COMPAT_STREAM_DATA_PREFIX) {
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(None);
        }
        return Ok(Some(payload));
    }
    // 裸 JSON（某些 provider 可能不使用 SSE 格式）
    if trimmed.starts_with('{') {
        return Ok(Some(trimmed));
    }
    // 跳过其他 SSE 元数据行（如 id:, retry: 等）
    if trimmed.contains(':') && !trimmed.starts_with('{') {
        return Ok(None);
    }
    // 其他格式报错
    Err(AxonError::runtime_protocol(
        "provider tool-call stream 行格式无效",
    ))
}

#[cfg(test)]
#[path = "provider_tool_call_tests.rs"]
mod tests;
