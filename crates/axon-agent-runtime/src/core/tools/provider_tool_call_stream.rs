//! Provider tool-call 参数流的增量累计。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult};

use crate::{
    ModelToolDefinition, OPENAI_COMPAT_STREAM_DATA_PREFIX, OPENAI_COMPAT_STREAM_DONE_MARKER,
    ProviderStreamProtocol, ProviderToolCall,
    provider_tool_call::{
        ProviderToolCallWireKind, ToolCallAccumulator, finalize_tool_calls,
        restore_provider_tool_names,
    },
};

/// 单个 tool-call 当前已完整到达的累计参数快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderToolCallPartial {
    /// Provider output index。
    pub index: usize,
    /// Provider call ID。
    pub id: String,
    /// Provider tool name。
    pub name: String,
    /// 当前完整输入文本。
    pub input_text: String,
    /// 本次新增输入片段。
    pub input_delta: String,
    /// 本次输入是否替换了此前累计值。
    pub input_reset: bool,
    /// Provider wire 输入类型。
    pub wire_kind: ProviderToolCallWireKind,
}

#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
    pending_delta: String,
    arguments_reset: bool,
    wire_kind: ProviderToolCallWireKind,
}

/// 跨 SSE 行保存并输出累计 tool-call 参数。
#[derive(Debug, Default)]
pub struct ProviderToolCallStreamRuntime {
    calls: BTreeMap<usize, PartialCall>,
}

impl ProviderToolCallStreamRuntime {
    /// 是否尚未记录任何 tool-call。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// 消费一个完整 SSE/JSONL 行，返回本行发生变化的累计 tool-call 快照。
    pub fn consume_line(
        &mut self,
        protocol: ProviderStreamProtocol,
        line: &str,
    ) -> AxonResult<Vec<ProviderToolCallPartial>> {
        let Some(payload) = stream_payload(line)? else {
            return Ok(Vec::new());
        };
        if payload == OPENAI_COMPAT_STREAM_DONE_MARKER {
            return Ok(Vec::new());
        }
        let value: serde_json::Value = serde_json::from_str(payload).map_err(|_| {
            if protocol == ProviderStreamProtocol::AnthropicMessages {
                AxonError::provider_invalid_response(
                    protocol.as_code(),
                    "Anthropic tool-call delta JSON 无效",
                )
            } else {
                AxonError::provider_invalid_response(
                    protocol.as_code(),
                    "provider tool-call delta JSON 无效",
                )
            }
        })?;
        let changed = match protocol {
            ProviderStreamProtocol::OpenAiResponses => self.consume_responses(&value)?,
            ProviderStreamProtocol::OpenAiCompatible => self.consume_chat_completions(&value)?,
            ProviderStreamProtocol::AnthropicMessages => self.consume_anthropic(&value)?,
        };
        Ok(changed
            .into_iter()
            .filter_map(|index| self.snapshot_if_ready(index))
            .collect())
    }

    pub(crate) fn finish(
        self,
        protocol: ProviderStreamProtocol,
        model_tools: &[ModelToolDefinition],
    ) -> AxonResult<Vec<ProviderToolCall>> {
        let calls = self
            .calls
            .into_iter()
            .map(|(index, call)| {
                (
                    index,
                    ToolCallAccumulator {
                        id: (!call.id.is_empty()).then_some(call.id),
                        name: (!call.name.is_empty()).then_some(call.name),
                        arguments: call.arguments,
                        wire_kind: call.wire_kind,
                    },
                )
            })
            .collect();
        let mut tool_calls = finalize_tool_calls(protocol, calls, model_tools)?;
        restore_provider_tool_names(&mut tool_calls, model_tools);
        Ok(tool_calls)
    }

    fn consume_chat_completions(&mut self, value: &serde_json::Value) -> AxonResult<Vec<usize>> {
        let mut changed = Vec::new();
        for tool_call in value
            .pointer("/choices/0/delta/tool_calls")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let index = json_index(tool_call, "index")?;
            let call = self.calls.entry(index).or_default();
            assign_nonempty(&mut call.id, tool_call.get("id"));
            assign_nonempty(&mut call.name, tool_call.pointer("/function/name"));
            if let Some(delta) = tool_call
                .pointer("/function/arguments")
                .and_then(serde_json::Value::as_str)
            {
                append_arguments(call, delta);
            }
            changed.push(index);
        }
        Ok(changed)
    }

    fn consume_responses(&mut self, value: &serde_json::Value) -> AxonResult<Vec<usize>> {
        let event_type = value.get("type").and_then(serde_json::Value::as_str);
        match event_type {
            Some("response.output_item.added" | "response.output_item.done") => {
                let index = json_index(value, "output_index")?;
                let Some(item) = value.get("item") else {
                    return Ok(Vec::new());
                };
                if !matches!(
                    item.get("type").and_then(serde_json::Value::as_str),
                    Some("function_call" | "custom_tool_call")
                ) {
                    return Ok(Vec::new());
                }
                let call = self.calls.entry(index).or_default();
                let item_type = item.get("type").and_then(serde_json::Value::as_str);
                call.wire_kind = if item_type == Some("custom_tool_call") {
                    ProviderToolCallWireKind::Custom
                } else {
                    ProviderToolCallWireKind::Function
                };
                assign_nonempty(&mut call.id, item.get("call_id"));
                assign_nonempty(&mut call.name, item.get("name"));
                let full = item
                    .get("arguments")
                    .or_else(|| item.get("input"))
                    .and_then(serde_json::Value::as_str);
                if let Some(full) = full.filter(|value| !value.is_empty()) {
                    replace_arguments(call, full);
                }
                Ok(vec![index])
            }
            Some("response.function_call_arguments.delta") => {
                let index = json_index(value, "output_index")?;
                let call = self.calls.entry(index).or_default();
                call.wire_kind = ProviderToolCallWireKind::Function;
                if let Some(delta) = value.get("delta").and_then(serde_json::Value::as_str) {
                    append_arguments(call, delta);
                }
                Ok(vec![index])
            }
            Some("response.custom_tool_call_input.delta") => {
                let index = json_index(value, "output_index")?;
                let call = self.calls.entry(index).or_default();
                call.wire_kind = ProviderToolCallWireKind::Custom;
                if let Some(delta) = value.get("delta").and_then(serde_json::Value::as_str) {
                    append_arguments(call, delta);
                }
                Ok(vec![index])
            }
            Some("response.function_call_arguments.done") => {
                let index = json_index(value, "output_index")?;
                let call = self.calls.entry(index).or_default();
                call.wire_kind = ProviderToolCallWireKind::Function;
                if let Some(full) = value.get("arguments").and_then(serde_json::Value::as_str) {
                    replace_arguments(call, full);
                }
                Ok(vec![index])
            }
            Some("response.custom_tool_call_input.done") => {
                let index = json_index(value, "output_index")?;
                let call = self.calls.entry(index).or_default();
                call.wire_kind = ProviderToolCallWireKind::Custom;
                if let Some(full) = value.get("input").and_then(serde_json::Value::as_str) {
                    replace_arguments(call, full);
                }
                Ok(vec![index])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn consume_anthropic(&mut self, value: &serde_json::Value) -> AxonResult<Vec<usize>> {
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("content_block_start") => {
                let index = json_index(value, "index")?;
                let Some(block) = value.get("content_block") else {
                    return Ok(Vec::new());
                };
                if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_use") {
                    return Ok(Vec::new());
                }
                let call = self.calls.entry(index).or_default();
                assign_nonempty(&mut call.id, block.get("id"));
                assign_nonempty(&mut call.name, block.get("name"));
                if let Some(input) = block.get("input") {
                    let input_json = serde_json::to_string(input).map_err(|_| {
                        AxonError::provider_invalid_response(
                            ProviderStreamProtocol::AnthropicMessages.as_code(),
                            "Anthropic tool-call input 序列化失败",
                        )
                    })?;
                    replace_arguments(call, &input_json);
                }
                Ok(vec![index])
            }
            Some("content_block_delta")
                if value
                    .pointer("/delta/type")
                    .and_then(serde_json::Value::as_str)
                    == Some("input_json_delta") =>
            {
                let index = json_index(value, "index")?;
                if let Some(delta) = value
                    .pointer("/delta/partial_json")
                    .and_then(serde_json::Value::as_str)
                {
                    append_anthropic_arguments(self.calls.entry(index).or_default(), delta);
                }
                Ok(vec![index])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn snapshot_if_ready(&mut self, index: usize) -> Option<ProviderToolCallPartial> {
        let call = self.calls.get_mut(&index)?;
        if call.id.is_empty()
            || call.name.is_empty()
            || (call.pending_delta.is_empty() && !call.arguments_reset)
        {
            return None;
        }
        Some(ProviderToolCallPartial {
            index,
            id: call.id.clone(),
            name: call.name.clone(),
            input_text: call.arguments.clone(),
            input_delta: std::mem::take(&mut call.pending_delta),
            input_reset: std::mem::take(&mut call.arguments_reset),
            wire_kind: call.wire_kind,
        })
    }
}

fn append_arguments(call: &mut PartialCall, delta: &str) {
    if call.arguments == "{}" {
        call.arguments.clear();
        call.arguments_reset = true;
    }
    call.arguments.push_str(delta);
    call.pending_delta.push_str(delta);
}

fn append_anthropic_arguments(call: &mut PartialCall, delta: &str) {
    if call.arguments != "{}" && !call.arguments.is_empty() && delta.trim_start().starts_with('{') {
        call.arguments.clear();
        call.pending_delta.clear();
        call.arguments_reset = true;
    }
    append_arguments(call, delta);
}

fn replace_arguments(call: &mut PartialCall, full: &str) {
    if full == "{}" && call.arguments.is_empty() {
        call.arguments.push_str(full);
        return;
    }
    if let Some(delta) = full.strip_prefix(&call.arguments) {
        append_arguments(call, delta);
        return;
    }
    call.arguments.clear();
    call.arguments.push_str(full);
    call.pending_delta.clear();
    call.pending_delta.push_str(full);
    call.arguments_reset = true;
}

fn assign_nonempty(target: &mut String, value: Option<&serde_json::Value>) {
    if let Some(value) = value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
    {
        target.clear();
        target.push_str(value);
    }
}

fn json_index(value: &serde_json::Value, field: &str) -> AxonResult<usize> {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::OpenAiCompatible.as_code(),
                "provider tool-call delta 缺少有效 index",
            )
        })
}

fn stream_payload(line: &str) -> AxonResult<Option<&str>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
        return Ok(None);
    }
    if let Some(payload) = trimmed.strip_prefix(OPENAI_COMPAT_STREAM_DATA_PREFIX) {
        return Ok((!payload.trim().is_empty()).then(|| payload.trim()));
    }
    if trimmed.starts_with('{') {
        return Ok(Some(trimmed));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_chat_emits_cumulative_arguments() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        let first = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"edit","arguments":"{\"input\":\"src/"}}]}}]}"#;
        let second = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"lib.rs\"}"}}]}}]}"#;

        let first = runtime
            .consume_line(ProviderStreamProtocol::OpenAiCompatible, first)
            .unwrap();
        let second = runtime
            .consume_line(ProviderStreamProtocol::OpenAiCompatible, second)
            .unwrap();

        assert_eq!(first[0].input_text, r#"{"input":"src/"#);
        assert_eq!(first[0].input_delta, r#"{"input":"src/"#);
        assert!(!first[0].input_reset);
        assert_eq!(second[0].input_text, r#"{"input":"src/lib.rs"}"#);
        assert_eq!(second[0].input_delta, r#"lib.rs"}"#);
    }

    #[test]
    fn responses_done_event_replaces_accumulated_arguments() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        runtime
            .consume_line(
                ProviderStreamProtocol::OpenAiResponses,
                r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call-1","name":"edit"}}"#,
            )
            .unwrap();
        runtime
            .consume_line(
                ProviderStreamProtocol::OpenAiResponses,
                r#"data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"input\":\"partial\"}"}"#,
            )
            .unwrap();
        let partial = runtime
            .consume_line(
                ProviderStreamProtocol::OpenAiResponses,
                r#"data: {"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"input\":\"complete\"}"}"#,
            )
            .unwrap();

        assert_eq!(partial[0].input_text, r#"{"input":"complete"}"#);
        assert!(partial[0].input_reset);
        assert_eq!(partial[0].wire_kind, ProviderToolCallWireKind::Function);
        let calls = runtime
            .finish(ProviderStreamProtocol::OpenAiResponses, &[])
            .unwrap();
        assert_eq!(
            calls[0].input.as_json(),
            Some(&serde_json::json!({"input": "complete"}))
        );
    }

    #[test]
    fn responses_custom_tool_done_event_replaces_accumulated_input() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        runtime
            .consume_line(
                ProviderStreamProtocol::OpenAiResponses,
                r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","call_id":"call-1","name":"edit"}}"#,
            )
            .unwrap();
        runtime
            .consume_line(
                ProviderStreamProtocol::OpenAiResponses,
                r#"data: {"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"partial"}"#,
            )
            .unwrap();
        let partial = runtime
            .consume_line(
                ProviderStreamProtocol::OpenAiResponses,
                r#"data: {"type":"response.custom_tool_call_input.done","output_index":0,"input":"complete"}"#,
            )
            .unwrap();

        assert_eq!(partial[0].input_text, "complete");
        assert!(partial[0].input_reset);
        assert_eq!(partial[0].wire_kind, ProviderToolCallWireKind::Custom);
        let calls = runtime
            .finish(ProviderStreamProtocol::OpenAiResponses, &[])
            .unwrap();
        assert_eq!(calls[0].input.as_text(), Some("complete"));
    }

    #[test]
    fn anthropic_emits_partial_json_after_tool_start() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool-1","name":"edit","input":{}}}"#,
            )
            .unwrap();
        let partial = runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"a.rs\"}"}}"#,
            )
            .unwrap();

        assert_eq!(partial[0].id, "tool-1");
        assert_eq!(partial[0].input_text, r#"{"input":"a.rs"}"#);
    }

    #[test]
    fn anthropic_uses_complete_input_when_provider_omits_json_deltas() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"edit","input":{"input":"new"}}}"#,
            )
            .unwrap();

        let calls = runtime
            .finish(ProviderStreamProtocol::AnthropicMessages, &[])
            .unwrap();

        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].input.as_json(),
            Some(&serde_json::json!({"input": "new"}))
        );
    }

    #[test]
    fn anthropic_replaces_full_start_input_when_delta_repeats_complete_json() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"edit","input":{"input":"old"}}}"#,
            )
            .unwrap();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"new\"}"}}"#,
            )
            .unwrap();

        let calls = runtime
            .finish(ProviderStreamProtocol::AnthropicMessages, &[])
            .unwrap();

        assert_eq!(
            calls[0].input.as_json(),
            Some(&serde_json::json!({"input": "new"}))
        );
    }

    #[test]
    fn anthropic_replaces_incomplete_arguments_when_delta_restarts_full_json() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool-2","name":"edit","input":{}}}"#,
            )
            .unwrap();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"a"}}"#,
            )
            .unwrap();
        runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"new\"}"}}"#,
            )
            .unwrap();

        let calls = runtime
            .finish(ProviderStreamProtocol::AnthropicMessages, &[])
            .unwrap();

        assert_eq!(
            calls[0].input.as_json(),
            Some(&serde_json::json!({"input": "new"}))
        );
    }

    #[test]
    fn anthropic_malformed_event_json_is_provider_failure() {
        let mut runtime = ProviderToolCallStreamRuntime::default();
        let error = runtime
            .consume_line(
                ProviderStreamProtocol::AnthropicMessages,
                "data: {\"type\":\"content_block_delta\"",
            )
            .unwrap_err();

        assert!(matches!(
            error,
            AxonError::Provider(axon_domain::ProviderError::InvalidResponse(_))
        ));
    }
}
