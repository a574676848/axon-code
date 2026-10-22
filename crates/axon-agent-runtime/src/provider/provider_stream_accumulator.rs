//! Provider 流式事件的 attempt 级增量聚合。

use axon_domain::{AxonError, AxonResult};

use crate::{
    AgentResponseStreamDelta, ModelToolDefinition, ProjectionRuntimeBudget,
    ProviderStreamPartialNormalizer, ProviderStreamProtocol, ProviderToolCall,
    ProviderToolCallStreamRuntime, TokenUsageTelemetry,
    provider_text_tool_call::ProviderTextToolCallFilter,
    provider_tool_call::restore_provider_tool_names,
};

const EMPTY_PROVIDER_STREAM_ERROR: &str = "provider streaming 未返回任何事件";
const PROVIDER_VISIBLE_DELTA_OVERFLOW: &str = "provider 可见文本 delta 序号溢出";
/// OpenAI Chat 的输出上限终止原因。
const OPENAI_CHAT_FINISH_REASON_LENGTH: &str = "length";
/// Anthropic 的输出上限终止原因。
const ANTHROPIC_STOP_REASON_MAX_TOKENS: &str = "max_tokens";
/// OpenAI Responses 的「响应不完整」事件。
const OPENAI_RESPONSES_INCOMPLETE_EVENT: &str = "response.incomplete";
/// OpenAI Responses 的不完整原因：输出 token 上限。
const OPENAI_RESPONSES_INCOMPLETE_REASON_MAX_OUTPUT_TOKENS: &str = "max_output_tokens";

/// Provider 单次完整流的结构化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamAggregate {
    /// 已归一化的正文增量。
    pub deltas: Vec<AgentResponseStreamDelta>,
    /// Provider 原生 turn 终止语义；缺失时保持未知。
    pub end_turn: Option<bool>,
    /// 实际消费的 Provider 事件数。
    pub provider_event_count: usize,
    /// 流中最后一次完整 token usage 快照。
    pub token_usage: Option<TokenUsageTelemetry>,
    /// 已聚合的推理内容。
    pub reasoning_content: Option<String>,
    /// 已完成并校验的工具调用。
    pub tool_calls: Vec<ProviderToolCall>,
    /// 已完成的 OpenAI Responses 服务端会话状态引用。
    pub server_conversation_state_ref: Option<String>,
    /// 本次流是否被输出 token 上限截断过。
    ///
    /// 与 `end_turn` 分开承载：`end_turn` 只说「这一轮停没停」，而
    /// 「被预算掐断」需要自己的信号，否则运行时就只能把它当成一次普通失败，
    /// 父代理无法分辨该重试还是该换策略（合同 §5.2.3）。
    pub max_tokens_truncated: bool,
}

/// 逐事件消费 Provider 流，只保留模型有效结果和 tool-call 组装状态。
pub struct ProviderStreamAccumulator {
    normalizer: ProviderStreamPartialNormalizer,
    tool_calls: ProviderToolCallStreamRuntime,
    text_tool_calls: ProviderTextToolCallFilter,
    model_tools: Vec<ModelToolDefinition>,
    deltas: Vec<AgentResponseStreamDelta>,
    end_turn: Option<bool>,
    reasoning_content: String,
    token_usage: Option<TokenUsageTelemetry>,
    provider_event_count: usize,
    server_conversation_state_ref: Option<String>,
    max_tokens_truncated: bool,
    error: Option<AxonError>,
}

impl ProviderStreamAccumulator {
    /// 为一次 Provider attempt 创建增量聚合器。
    #[must_use]
    pub fn new(protocol: ProviderStreamProtocol, model_tools: Vec<ModelToolDefinition>) -> Self {
        Self {
            normalizer: ProviderStreamPartialNormalizer::new(protocol),
            tool_calls: ProviderToolCallStreamRuntime::default(),
            text_tool_calls: ProviderTextToolCallFilter::default(),
            model_tools,
            deltas: Vec::new(),
            end_turn: None,
            reasoning_content: String::new(),
            token_usage: None,
            provider_event_count: 0,
            server_conversation_state_ref: None,
            max_tokens_truncated: false,
            error: None,
        }
    }

    /// 消费一个已去除传输包装的 Provider 事件。
    pub fn consume(&mut self, event: &str) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.consume_inner(event) {
            self.error = Some(error);
        }
    }

    /// 消费事件并返回本事件新产生的归一化正文增量。
    pub fn consume_projected(&mut self, event: &str) -> Vec<AgentResponseStreamDelta> {
        let before = self.deltas.len();
        self.consume(event);
        self.deltas
            .get(before..)
            .map_or_else(Vec::new, <[AgentResponseStreamDelta]>::to_vec)
    }

    /// 完成聚合，并传播消费过程中记录的首个协议错误。
    pub fn finish(mut self) -> AxonResult<ProviderStreamAggregate> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let text_tool_call_finish = std::mem::take(&mut self.text_tool_calls).finish();
        self.append_visible_text(&text_tool_call_finish.visible_text)?;
        let protocol = self.normalizer.protocol();
        let mut tool_calls = self.tool_calls.finish(protocol, &self.model_tools)?;
        let has_text_tool_calls = !text_tool_call_finish.tool_calls.is_empty();
        let mut text_tool_calls = text_tool_call_finish.tool_calls;
        restore_provider_tool_names(&mut text_tool_calls, &self.model_tools);
        tool_calls.extend(text_tool_calls);
        if self.provider_event_count == 0 && tool_calls.is_empty() {
            return Err(AxonError::provider_failure(EMPTY_PROVIDER_STREAM_ERROR));
        }
        Ok(ProviderStreamAggregate {
            deltas: self.deltas,
            end_turn: has_text_tool_calls.then_some(false).or(self.end_turn),
            provider_event_count: self.provider_event_count,
            token_usage: self.token_usage,
            reasoning_content: (!self.reasoning_content.is_empty())
                .then_some(self.reasoning_content),
            tool_calls,
            server_conversation_state_ref: self.server_conversation_state_ref,
            max_tokens_truncated: self.max_tokens_truncated,
        })
    }

    fn consume_inner(&mut self, event: &str) -> AxonResult<()> {
        match self.normalizer.normalize(event) {
            Ok(report) => {
                if let Some(end_turn) = report.end_turn {
                    self.end_turn = Some(end_turn);
                }
                self.normalizer.commit_text_deltas(report.deltas.len());
                self.provider_event_count = self
                    .provider_event_count
                    .saturating_add(report.provider_event_count);
                for delta in report.deltas {
                    let visible = self.text_tool_calls.consume(&delta.content_delta);
                    self.append_visible_text(&visible)?;
                }
                for delta in report.reasoning_deltas {
                    self.reasoning_content.push_str(&delta.content_delta);
                }
            }
            Err(AxonError::EmptyInput) => {}
            Err(error) => return Err(error),
        }
        if let Some(usage) = self.normalizer.partial_token_usage(event) {
            self.token_usage = Some(usage);
        }
        self.max_tokens_truncated |= max_tokens_truncation(event);
        if self.normalizer.protocol() == ProviderStreamProtocol::OpenAiResponses
            && let Some(response_id) = completed_response_id(event)
        {
            self.server_conversation_state_ref = Some(response_id);
        }
        self.tool_calls
            .consume_line(self.normalizer.protocol(), event)?;
        Ok(())
    }

    fn append_visible_text(&mut self, text: &str) -> AxonResult<()> {
        let budget = ProjectionRuntimeBudget::current();
        let mut remaining = text;
        while !remaining.is_empty() {
            let (chunk, next) = budget.split_text_chunk(remaining);
            let sequence = u64::try_from(self.deltas.len()).map_err(|_| {
                AxonError::provider_invalid_response(
                    self.normalizer.protocol().as_code(),
                    PROVIDER_VISIBLE_DELTA_OVERFLOW,
                )
            })?;
            self.deltas.push(AgentResponseStreamDelta {
                sequence,
                content_delta: chunk.to_owned(),
            });
            remaining = next;
        }
        Ok(())
    }
}

fn completed_response_id(event: &str) -> Option<String> {
    let value = event_json(event)?;
    (value.get("type").and_then(serde_json::Value::as_str) == Some("response.completed"))
        .then(|| {
            value
                .pointer("/response/id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned)
        })
        .flatten()
}

/// Provider 原生终止原因是否表示输出 token 上限把这一轮截断了。
///
/// 三种协议各有自己的说法：OpenAI Chat 用 `finish_reason: "length"`、
/// Anthropic 用 `stop_reason: "max_tokens"`、OpenAI Responses 用
/// `response.incomplete` 配 `incomplete_details.reason`。
fn max_tokens_truncation(event: &str) -> bool {
    let Some(value) = event_json(event) else {
        return false;
    };
    if value.get("type").and_then(serde_json::Value::as_str)
        == Some(OPENAI_RESPONSES_INCOMPLETE_EVENT)
    {
        return value
            .pointer("/response/incomplete_details/reason")
            .and_then(serde_json::Value::as_str)
            == Some(OPENAI_RESPONSES_INCOMPLETE_REASON_MAX_OUTPUT_TOKENS);
    }
    let openai_chat_truncated = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice
                    .get("finish_reason")
                    .and_then(serde_json::Value::as_str)
                    == Some(OPENAI_CHAT_FINISH_REASON_LENGTH)
            })
        });
    openai_chat_truncated
        || value
            .pointer("/delta/stop_reason")
            .and_then(serde_json::Value::as_str)
            == Some(ANTHROPIC_STOP_REASON_MAX_TOKENS)
}

/// 取出事件里的 JSON 载荷；非 JSON 行返回 `None`。
fn event_json(event: &str) -> Option<serde_json::Value> {
    let payload = event
        .trim()
        .strip_prefix("data:")
        .map(str::trim)
        .unwrap_or_else(|| event.trim());
    serde_json::from_str(payload).ok()
}

#[cfg(test)]
#[path = "provider_stream_accumulator_tests.rs"]
mod tests;
