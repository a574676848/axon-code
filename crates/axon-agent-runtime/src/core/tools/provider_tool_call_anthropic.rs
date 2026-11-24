//! Anthropic Messages tool-call 参数流累加、重启恢复与规范化。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult};

use crate::{
    ModelToolDefinition, ProviderStreamProtocol, ProviderToolCall,
    provider_stream_error::provider_stream_error_event,
};

use super::{ToolCallAccumulator, finalize_tool_calls, normalize_stream_line};

const ANTHROPIC_TOOL_CALL_ERROR_EVENT: &str = "Anthropic tool-call stream 返回 error 事件";

/// 单条 `input_json_delta` 对累计缓冲区产生的状态变更。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnthropicDeltaAction {
    NoOp,
    Appended,
    ReplacedEmptyPlaceholder,
    ReplacedCompleteStart,
}

#[derive(Debug, Default)]
struct AnthropicCallState {
    call: ToolCallAccumulator,
    has_complete_start_input: bool,
}

/// 累加一条 Anthropic `input_json_delta` 片段。
///
/// 不在参数值恰巧以 `{` 开头时清空已累计前缀，仅记录潜在的重启字节边界，
/// 待完整流结束后仅当拼接文本无法解析为合法 JSON 时才回溯恢复。
pub(crate) fn append_anthropic_delta(
    arguments: &mut String,
    has_complete_start_input: &mut bool,
    restart_offsets: &mut Vec<usize>,
    delta: &str,
) -> AnthropicDeltaAction {
    if delta.is_empty() {
        return AnthropicDeltaAction::NoOp;
    }
    if *has_complete_start_input && delta.trim_start().starts_with('{') {
        arguments.clear();
        arguments.push_str(delta);
        *has_complete_start_input = false;
        restart_offsets.clear();
        return AnthropicDeltaAction::ReplacedCompleteStart;
    }
    if arguments == "{}" {
        arguments.clear();
        arguments.push_str(delta);
        *has_complete_start_input = false;
        restart_offsets.clear();
        return AnthropicDeltaAction::ReplacedEmptyPlaceholder;
    }
    if !arguments.is_empty() && delta.trim_start().starts_with('{') {
        restart_offsets.push(arguments.len());
    }
    arguments.push_str(delta);
    AnthropicDeltaAction::Appended
}

/// 当最终拼接文本不是合法 JSON 时，按记录的 `{` 边界从后往前恢复完整重启负载。
pub(crate) fn recover_anthropic_restarted_arguments(
    arguments: &mut String,
    restart_offsets: &[usize],
) {
    if restart_offsets.is_empty() || serde_json::from_str::<serde_json::Value>(arguments).is_ok() {
        return;
    }
    for &offset in restart_offsets.iter().rev() {
        let Some(candidate) = arguments.get(offset..) else {
            continue;
        };
        if matches!(
            serde_json::from_str::<serde_json::Value>(candidate),
            Ok(serde_json::Value::Object(_))
        ) {
            *arguments = candidate.to_owned();
            return;
        }
    }
}

pub(super) fn normalize_anthropic_tool_calls(
    raw_stream: &str,
    model_tools: &[ModelToolDefinition],
) -> AxonResult<Vec<ProviderToolCall>> {
    let mut calls = BTreeMap::<usize, AnthropicCallState>::new();
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
                    entry.call.id = Some(id.to_owned());
                }
                if let Some(name) = content_block
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                {
                    entry.call.name = Some(name.to_owned());
                }
                if let Some(input) = content_block.get("input").filter(|value| !value.is_null()) {
                    let input_text = serde_json::to_string(input).map_err(|_| {
                        AxonError::provider_invalid_response(
                            ProviderStreamProtocol::AnthropicMessages.as_code(),
                            "Anthropic tool-call input 序列化失败",
                        )
                    })?;
                    if input_text != "{}" || entry.call.arguments.is_empty() {
                        entry.call.arguments = input_text;
                        entry.has_complete_start_input =
                            entry.call.arguments != "{}" && input.is_object();
                        entry.call.anthropic_restart_offsets.clear();
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
                let _ = append_anthropic_delta(
                    &mut entry.call.arguments,
                    &mut entry.has_complete_start_input,
                    &mut entry.call.anthropic_restart_offsets,
                    partial_json,
                );
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
    let accumulators = calls
        .into_iter()
        .map(|(index, state)| (index, state.call))
        .collect();
    finalize_tool_calls(
        ProviderStreamProtocol::AnthropicMessages,
        accumulators,
        model_tools,
    )
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
