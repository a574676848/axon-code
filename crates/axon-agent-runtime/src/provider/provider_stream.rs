//! OpenAI-compatible provider streaming 归一化。
//!
//! 该模块只把 provider SSE / JSONL 文本块归一化为 Axon 内部 streaming delta，
//! 不执行 HTTP 请求，也不直接更新终端 UI。

#[path = "provider_stream_partial.rs"]
mod partial;
#[path = "provider_stream/usage.rs"]
mod usage;

use axon_domain::{AxonError, AxonResult};

use crate::provider_stream_error::provider_stream_error_event;
use crate::{AgentResponseStreamDelta, ProviderStreamProtocol, TokenUsageTelemetry};

use self::usage::{merge_anthropic_usage, parse_openai_responses_usage, parse_openai_stream_usage};
use partial::{
    append_anthropic_partial_event, append_openai_partial_event, normalize_partial_stream,
};

/// OpenAI-compatible SSE data 前缀。
pub const OPENAI_COMPAT_STREAM_DATA_PREFIX: &str = "data:";
/// OpenAI-compatible SSE 完成标记。
pub const OPENAI_COMPAT_STREAM_DONE_MARKER: &str = "[DONE]";
/// OpenAI-compatible streaming 归一化完成原因。
pub const OPENAI_COMPAT_STREAM_REASON_NORMALIZED: &str =
    "openai_compatible_stream:normalized_text_delta";
/// OpenAI-compatible streaming 完整 provider payload 关闭原因。
pub const OPENAI_COMPAT_STREAM_REASON_RAW_PAYLOAD_CLOSED: &str =
    "openai_compatible_stream:raw_payload_closed";
/// Anthropic Messages streaming 文本归一化完成原因。
pub const ANTHROPIC_MESSAGES_STREAM_REASON_NORMALIZED: &str =
    "anthropic_messages_stream:normalized_text_delta";
/// Anthropic Messages streaming 完整 provider payload 关闭原因。
pub const ANTHROPIC_MESSAGES_STREAM_REASON_RAW_PAYLOAD_CLOSED: &str =
    "anthropic_messages_stream:raw_payload_closed";
/// Provider streaming 部分 chunk 归一化完成原因。
pub const PROVIDER_STREAM_PARTIAL_REASON_NORMALIZED: &str = "provider_stream:partial_normalized";
/// Provider streaming 部分 chunk 原始 payload 关闭原因。
pub const PROVIDER_STREAM_PARTIAL_REASON_RAW_PAYLOAD_CLOSED: &str =
    "provider_stream:partial_raw_payload_closed";
const OPENAI_COMPAT_STREAM_ERROR_EVENT: &str = "OpenAI-compatible stream 返回 error 事件";
pub(super) const OPENAI_RESPONSES_STREAM_ERROR_EVENT: &str =
    "OpenAI Responses stream 返回 error 事件";
pub(super) const ANTHROPIC_MESSAGES_STREAM_ERROR_EVENT: &str =
    "Anthropic Messages stream 返回 error 事件";
const OPENAI_RESPONSES_REASONING_TEXT_DELTA_EVENT: &str = "response.reasoning_text.delta";
const OPENAI_RESPONSES_REASONING_SUMMARY_TEXT_DELTA_EVENT: &str =
    "response.reasoning_summary_text.delta";
const OPENAI_COMPAT_REASONING_PRIMARY_FIELD: &str = "reasoning";
const OPENAI_COMPAT_REASONING_CONTENT_FIELD: &str = "reasoning_content";

/// Provider streaming 部分 chunk 归一化报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamPartialReport {
    /// 已归一化文本 delta。
    pub deltas: Vec<AgentResponseStreamDelta>,
    /// 已归一化 reasoning delta，包含 provider thinking_delta 映射。
    pub reasoning_deltas: Vec<AgentResponseStreamDelta>,
    /// Provider 原生 reasoning block 标识，用于区分同一 turn 内的多个思考事件。
    pub reasoning_event_key: Option<String>,
    /// 当前 provider 事件是否结束了 reasoning part。
    pub reasoning_end: bool,
    /// Provider 原生 turn 终止语义；缺失时保持未知。
    pub end_turn: Option<bool>,
    /// 已消费 provider 事件数量。
    pub provider_event_count: usize,
    /// 是否允许远控读取 provider 原始 payload。
    pub remote_raw_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// OpenAI-compatible streaming 归一化报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiCompatibleStreamReport {
    /// 已归一化文本 delta。
    pub deltas: Vec<AgentResponseStreamDelta>,
    /// 已消费 provider 事件数量。
    pub provider_event_count: usize,
    /// 是否收到 `[DONE]`。
    pub done_received: bool,
    /// Provider finish reason。
    pub finish_reason: Option<String>,
    /// Provider streaming final usage。
    pub token_usage: Option<TokenUsageTelemetry>,
    /// provider reasoning 内容，包含 Anthropic thinking 映射。
    pub reasoning_content: Option<String>,
    /// 是否允许远控读取 provider 原始 payload。
    pub remote_raw_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Anthropic Messages streaming 归一化报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicMessagesStreamReport {
    /// 已归一化文本 delta。
    pub deltas: Vec<AgentResponseStreamDelta>,
    /// 已消费 provider 事件数量。
    pub provider_event_count: usize,
    /// 是否收到 message_stop。
    pub message_stop_received: bool,
    /// Provider stop reason。
    pub stop_reason: Option<String>,
    /// Provider token usage。
    pub token_usage: Option<TokenUsageTelemetry>,
    /// provider reasoning 内容，包含 Anthropic thinking 映射。
    pub reasoning_content: Option<String>,
    /// 是否允许远控读取 provider 原始 payload。
    pub remote_raw_payload_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// OpenAI-compatible streaming adapter。
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenAiCompatibleStreamRuntime;

/// Anthropic Messages streaming adapter。
#[derive(Debug, Default, Clone, Copy)]
pub struct AnthropicMessagesStreamRuntime;

/// 跨交互适配器复用的 provider 部分流归一化器。
///
/// 它只保存协议与已确认文本序号，不缓存原始 provider 数据。调用方应在自己的
/// 投影成功写出后提交文本数量，避免输出失败时跳过 chunk 序号。
#[derive(Debug, Clone)]
pub struct ProviderStreamPartialNormalizer {
    protocol: ProviderStreamProtocol,
    next_text_sequence: u64,
}

impl ProviderStreamPartialNormalizer {
    /// 创建当前 provider 协议的部分流归一化器。
    #[must_use]
    pub fn new(protocol: ProviderStreamProtocol) -> Self {
        Self {
            protocol,
            next_text_sequence: 0,
        }
    }

    /// 从当前帧提取可投影文本、reasoning delta 与 Provider 原生结束语义。
    pub fn normalize(&self, raw_stream_chunk: &str) -> AxonResult<ProviderStreamPartialReport> {
        match self.protocol {
            ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
                OpenAiCompatibleStreamRuntime
                    .normalize_partial_text_deltas(raw_stream_chunk, self.next_text_sequence)
            }
            ProviderStreamProtocol::AnthropicMessages => AnthropicMessagesStreamRuntime
                .normalize_partial_text_deltas(raw_stream_chunk, self.next_text_sequence),
        }
    }

    /// 提交已成功投影的文本 delta 数量。
    pub fn commit_text_deltas(&mut self, count: usize) {
        self.next_text_sequence = self
            .next_text_sequence
            .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }

    /// 从当前帧提取可用 token usage。
    #[must_use]
    pub fn partial_token_usage(&self, raw_stream_chunk: &str) -> Option<TokenUsageTelemetry> {
        match self.protocol {
            ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
                OpenAiCompatibleStreamRuntime
                    .normalize_partial_token_usage(self.protocol, raw_stream_chunk)
            }
            ProviderStreamProtocol::AnthropicMessages => {
                AnthropicMessagesStreamRuntime.normalize_partial_token_usage(raw_stream_chunk)
            }
        }
    }

    /// 当前协议，供同一帧的附加投影使用。
    #[must_use]
    pub fn protocol(&self) -> ProviderStreamProtocol {
        self.protocol
    }
}

impl OpenAiCompatibleStreamRuntime {
    /// 从 OpenAI-compatible SSE / JSONL 片段中提取 provider token usage。
    #[must_use]
    pub fn normalize_partial_token_usage(
        &self,
        protocol: ProviderStreamProtocol,
        raw_stream_chunk: &str,
    ) -> Option<TokenUsageTelemetry> {
        partial_openai_token_usage(protocol, raw_stream_chunk)
    }

    /// 解析 OpenAI-compatible SSE / JSONL 文本为内部 streaming delta。
    pub fn normalize_text_deltas(
        &self,
        protocol: ProviderStreamProtocol,
        raw_stream: &str,
    ) -> AxonResult<OpenAiCompatibleStreamReport> {
        if raw_stream.trim().is_empty() {
            return Err(AxonError::EmptyInput);
        }

        let mut deltas = Vec::new();
        let mut provider_event_count = 0usize;
        let mut done_received = false;
        let mut finish_reason = None;
        let mut token_usage = None;
        let mut reasoning_content = String::new();

        for line in raw_stream.lines() {
            let Some(payload) = normalize_stream_line(line)? else {
                continue;
            };
            if payload == OPENAI_COMPAT_STREAM_DONE_MARKER {
                done_received = true;
                continue;
            }

            let value: serde_json::Value = serde_json::from_str(payload).map_err(|_| {
                AxonError::provider_invalid_response(
                    protocol.as_code(),
                    "OpenAI-compatible stream JSON 无效",
                )
            })?;
            if is_openai_responses_event(protocol, &value) {
                provider_event_count += 1;
                append_openai_responses_event(
                    &value,
                    &mut deltas,
                    &mut done_received,
                    &mut finish_reason,
                    &mut token_usage,
                    &mut reasoning_content,
                )?;
                continue;
            }
            provider_event_count += 1;
            if let Some(usage) = parse_openai_stream_usage(&value) {
                token_usage = Some(usage);
                if value.get("choices").is_none() {
                    continue;
                }
            }
            append_choice_deltas(
                &value,
                &mut deltas,
                &mut finish_reason,
                &mut reasoning_content,
            )?;
        }

        if deltas.is_empty()
            && reasoning_content.trim().is_empty()
            && !done_received
            && finish_reason.is_none()
        {
            return Err(AxonError::EmptyInput);
        }
        if !done_received {
            return Err(AxonError::provider_invalid_response(
                protocol.as_code(),
                "OpenAI-compatible stream 缺少完成标记",
            ));
        }

        Ok(OpenAiCompatibleStreamReport {
            deltas,
            provider_event_count,
            done_received,
            finish_reason,
            token_usage,
            reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
            remote_raw_payload_allowed: false,
            reasons: vec![
                OPENAI_COMPAT_STREAM_REASON_NORMALIZED.to_owned(),
                OPENAI_COMPAT_STREAM_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        })
    }

    /// 从尚未完成的 OpenAI-compatible SSE / JSONL 文本中提取已完整到达的文本 delta。
    pub fn normalize_partial_text_deltas(
        &self,
        raw_stream_chunk: &str,
        sequence_offset: u64,
    ) -> AxonResult<ProviderStreamPartialReport> {
        normalize_partial_stream(
            raw_stream_chunk,
            sequence_offset,
            append_openai_partial_event,
        )
    }
}

impl AnthropicMessagesStreamRuntime {
    /// 从 Anthropic Messages SSE 片段中提取 provider token usage。
    #[must_use]
    pub fn normalize_partial_token_usage(
        &self,
        raw_stream_chunk: &str,
    ) -> Option<TokenUsageTelemetry> {
        partial_anthropic_token_usage(raw_stream_chunk)
    }

    /// 解析 Anthropic Messages SSE 文本为内部 streaming delta。
    pub fn normalize_text_deltas(
        &self,
        raw_stream: &str,
    ) -> AxonResult<AnthropicMessagesStreamReport> {
        if raw_stream.trim().is_empty() {
            return Err(AxonError::EmptyInput);
        }

        let mut deltas = Vec::new();
        let mut provider_event_count = 0usize;
        let mut message_stop_received = false;
        let mut stop_reason = None;
        let mut token_usage = None;
        let mut reasoning_content = String::new();

        for line in raw_stream.lines() {
            let Some(payload) = normalize_stream_line(line)? else {
                continue;
            };
            let value: serde_json::Value = serde_json::from_str(payload).map_err(|_| {
                AxonError::provider_invalid_response(
                    ProviderStreamProtocol::AnthropicMessages.as_code(),
                    "Anthropic Messages stream JSON 无效",
                )
            })?;
            provider_event_count += 1;
            append_anthropic_event(
                &value,
                &mut deltas,
                &mut message_stop_received,
                &mut stop_reason,
                &mut token_usage,
                &mut reasoning_content,
            )?;
        }

        if deltas.is_empty()
            && reasoning_content.trim().is_empty()
            && !message_stop_received
            && stop_reason.is_none()
        {
            return Err(AxonError::EmptyInput);
        }
        if !message_stop_received {
            return Err(AxonError::provider_invalid_response(
                ProviderStreamProtocol::AnthropicMessages.as_code(),
                "Anthropic Messages stream 缺少完成标记",
            ));
        }

        Ok(AnthropicMessagesStreamReport {
            deltas,
            provider_event_count,
            message_stop_received,
            stop_reason,
            token_usage,
            reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
            remote_raw_payload_allowed: false,
            reasons: vec![
                ANTHROPIC_MESSAGES_STREAM_REASON_NORMALIZED.to_owned(),
                ANTHROPIC_MESSAGES_STREAM_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        })
    }

    /// 从尚未完成的 Anthropic Messages SSE 文本中提取已完整到达的文本 delta。
    pub fn normalize_partial_text_deltas(
        &self,
        raw_stream_chunk: &str,
        sequence_offset: u64,
    ) -> AxonResult<ProviderStreamPartialReport> {
        normalize_partial_stream(
            raw_stream_chunk,
            sequence_offset,
            append_anthropic_partial_event,
        )
    }
}

pub(super) fn normalize_stream_line(line: &str) -> AxonResult<Option<&str>> {
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
    Err(AxonError::runtime_protocol("Provider stream 行格式无效"))
}

fn append_choice_deltas(
    value: &serde_json::Value,
    deltas: &mut Vec<AgentResponseStreamDelta>,
    finish_reason: &mut Option<String>,
    reasoning_content: &mut String,
) -> AxonResult<()> {
    reject_openai_error_event(value)?;
    let choices = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::OpenAiCompatible.as_code(),
                "OpenAI-compatible stream 缺少 choices",
            )
        })?;

    for choice in choices {
        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(serde_json::Value::as_str)
            .filter(|reason| !reason.trim().is_empty())
        {
            *finish_reason = Some(reason.to_owned());
        }
        if let Some(content_delta) = choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(serde_json::Value::as_str)
            .filter(|content| !content.is_empty())
        {
            deltas.push(AgentResponseStreamDelta {
                sequence: u64::try_from(deltas.len()).map_err(|_| {
                    AxonError::provider_invalid_response(
                        ProviderStreamProtocol::OpenAiCompatible.as_code(),
                        "OpenAI-compatible stream delta 过多",
                    )
                })?,
                content_delta: content_delta.to_owned(),
            });
        }
        if let Some(reasoning_delta) = openai_compatible_reasoning_delta(choice) {
            reasoning_content.push_str(reasoning_delta);
        }
    }

    Ok(())
}

pub(super) fn openai_compatible_reasoning_delta(choice: &serde_json::Value) -> Option<&str> {
    let delta = choice.get("delta")?;
    delta
        .get(OPENAI_COMPAT_REASONING_PRIMARY_FIELD)
        .and_then(serde_json::Value::as_str)
        .filter(|content| !content.is_empty())
        .or_else(|| {
            delta
                .get(OPENAI_COMPAT_REASONING_CONTENT_FIELD)
                .and_then(serde_json::Value::as_str)
                .filter(|content| !content.is_empty())
        })
}

fn append_openai_responses_event(
    value: &serde_json::Value,
    deltas: &mut Vec<AgentResponseStreamDelta>,
    done_received: &mut bool,
    finish_reason: &mut Option<String>,
    token_usage: &mut Option<TokenUsageTelemetry>,
    reasoning_content: &mut String,
) -> AxonResult<()> {
    reject_openai_responses_error_event(value)?;
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("response.output_text.delta") => {
            if let Some(content_delta) = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
            {
                deltas.push(AgentResponseStreamDelta {
                    sequence: u64::try_from(deltas.len()).map_err(|_| {
                        AxonError::provider_invalid_response(
                            ProviderStreamProtocol::OpenAiResponses.as_code(),
                            "OpenAI Responses stream delta 过多",
                        )
                    })?,
                    content_delta: content_delta.to_owned(),
                });
            }
        }
        event_type if is_openai_responses_reasoning_delta_event(event_type) => {
            if let Some(delta) = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
            {
                reasoning_content.push_str(delta);
            }
        }
        Some("response.completed") => {
            *done_received = true;
            *finish_reason = Some("stop".to_owned());
            *token_usage = parse_openai_responses_usage(Some(value));
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn reject_openai_error_event(value: &serde_json::Value) -> AxonResult<()> {
    if let Some(error) = value.get("error") {
        return Err(provider_stream_error_event(
            OPENAI_COMPAT_STREAM_ERROR_EVENT,
            error,
        ));
    }
    Ok(())
}

pub(super) fn reject_openai_responses_error_event(value: &serde_json::Value) -> AxonResult<()> {
    if value.get("type").and_then(serde_json::Value::as_str) == Some("response.failed") {
        return Err(provider_stream_error_event(
            OPENAI_RESPONSES_STREAM_ERROR_EVENT,
            value.pointer("/response/error").unwrap_or(value),
        ));
    }
    Ok(())
}

fn append_anthropic_event(
    value: &serde_json::Value,
    deltas: &mut Vec<AgentResponseStreamDelta>,
    message_stop_received: &mut bool,
    stop_reason: &mut Option<String>,
    token_usage: &mut Option<TokenUsageTelemetry>,
    reasoning_content: &mut String,
) -> AxonResult<()> {
    let event_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::AnthropicMessages.as_code(),
                "Anthropic Messages stream 缺少 type",
            )
        })?;

    if event_type == "message_stop" {
        *message_stop_received = true;
        return Ok(());
    }
    if event_type == "message_start" {
        merge_anthropic_usage(token_usage, value.pointer("/message/usage"));
        return Ok(());
    }
    if event_type == "message_delta" {
        if let Some(reason) = value
            .pointer("/delta/stop_reason")
            .and_then(serde_json::Value::as_str)
            .filter(|reason| !reason.trim().is_empty())
        {
            *stop_reason = Some(reason.to_owned());
        }
        merge_anthropic_usage(token_usage, value.get("usage"));
        return Ok(());
    }
    if event_type == "content_block_delta" {
        if let Some(thinking_delta) = value
            .get("delta")
            .and_then(|delta| {
                (delta.get("type").and_then(serde_json::Value::as_str) == Some("thinking_delta"))
                    .then_some(delta)
            })
            .and_then(|delta| delta.get("thinking"))
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
        {
            reasoning_content.push_str(thinking_delta);
        }
        if let Some(text_delta) = value
            .get("delta")
            .and_then(|delta| {
                (delta.get("type").and_then(serde_json::Value::as_str) == Some("text_delta"))
                    .then_some(delta)
            })
            .and_then(|delta| delta.get("text"))
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
        {
            deltas.push(AgentResponseStreamDelta {
                sequence: u64::try_from(deltas.len()).map_err(|_| {
                    AxonError::provider_invalid_response(
                        ProviderStreamProtocol::AnthropicMessages.as_code(),
                        "Anthropic Messages stream delta 过多",
                    )
                })?,
                content_delta: text_delta.to_owned(),
            });
        }
        return Ok(());
    }
    if event_type == "error" {
        return Err(provider_stream_error_event(
            ANTHROPIC_MESSAGES_STREAM_ERROR_EVENT,
            value.get("error").unwrap_or(value),
        ));
    }
    Ok(())
}

pub(super) fn is_openai_responses_reasoning_delta_event(event_type: Option<&str>) -> bool {
    matches!(
        event_type,
        Some(
            OPENAI_RESPONSES_REASONING_TEXT_DELTA_EVENT
                | OPENAI_RESPONSES_REASONING_SUMMARY_TEXT_DELTA_EVENT
        )
    )
}

fn partial_openai_token_usage(
    protocol: ProviderStreamProtocol,
    raw_stream_chunk: &str,
) -> Option<TokenUsageTelemetry> {
    let mut token_usage = None;
    for value in complete_json_values(raw_stream_chunk) {
        if is_openai_responses_event(protocol, &value) {
            if value.get("type").and_then(serde_json::Value::as_str) == Some("response.completed") {
                token_usage = parse_openai_responses_usage(Some(&value));
            }
            continue;
        }
        if let Some(usage) = parse_openai_stream_usage(&value) {
            token_usage = Some(usage);
        }
    }
    token_usage
}

fn partial_anthropic_token_usage(raw_stream_chunk: &str) -> Option<TokenUsageTelemetry> {
    let mut token_usage = None;
    for value in complete_json_values(raw_stream_chunk) {
        let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if event_type == "message_start" {
            merge_anthropic_usage(&mut token_usage, value.pointer("/message/usage"));
        } else if event_type == "message_delta" {
            merge_anthropic_usage(&mut token_usage, value.get("usage"));
        }
    }
    token_usage
}

fn complete_json_values(raw_stream_chunk: &str) -> Vec<serde_json::Value> {
    let lines: Vec<&str> = raw_stream_chunk.lines().collect();
    let has_complete_tail = raw_stream_chunk.ends_with('\n') || raw_stream_chunk.ends_with('\r');
    let mut values = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Ok(Some(payload)) = normalize_stream_line(line) else {
            continue;
        };
        if payload == OPENAI_COMPAT_STREAM_DONE_MARKER {
            continue;
        }
        match serde_json::from_str(payload) {
            Ok(value) => values.push(value),
            Err(_) if index + 1 == lines.len() && !has_complete_tail => {}
            Err(_) => {}
        }
    }
    values
}

pub(super) fn is_openai_responses_event(
    protocol: ProviderStreamProtocol,
    value: &serde_json::Value,
) -> bool {
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return false;
    };
    kind.starts_with("response.") || protocol == ProviderStreamProtocol::OpenAiResponses
}

#[cfg(test)]
#[path = "provider_stream_tests.rs"]
mod tests;
