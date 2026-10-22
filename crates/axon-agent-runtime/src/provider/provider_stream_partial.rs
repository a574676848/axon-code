use axon_domain::{AxonError, AxonResult};

use crate::provider_stream_error::provider_stream_error_event;
use crate::{AgentResponseStreamDelta, ProjectionRuntimeBudget, ProviderStreamProtocol};

use super::{
    ANTHROPIC_MESSAGES_STREAM_ERROR_EVENT, OPENAI_COMPAT_STREAM_DONE_MARKER,
    PROVIDER_STREAM_PARTIAL_REASON_NORMALIZED, PROVIDER_STREAM_PARTIAL_REASON_RAW_PAYLOAD_CLOSED,
    ProviderStreamPartialReport, is_openai_responses_event,
    is_openai_responses_reasoning_delta_event, normalize_stream_line,
    openai_compatible_reasoning_delta, reject_openai_error_event,
    reject_openai_responses_error_event,
};

const OPENAI_RESPONSES_COMPLETED_EVENT: &str = "response.completed";
const OPENAI_RESPONSES_OUTPUT_ITEM_DONE_EVENT: &str = "response.output_item.done";
const OPENAI_RESPONSES_REASONING_ITEM_TYPE: &str = "reasoning";
const OPENAI_CHAT_FINISH_REASON_STOP: &str = "stop";
const ANTHROPIC_MESSAGE_DELTA_EVENT: &str = "message_delta";
const ANTHROPIC_CONTENT_BLOCK_STOP_EVENT: &str = "content_block_stop";
const ANTHROPIC_STOP_REASON_END_TURN: &str = "end_turn";

pub(super) fn normalize_partial_stream(
    raw_stream_chunk: &str,
    sequence_offset: u64,
    mut append_event: impl FnMut(
        &serde_json::Value,
        &mut Vec<String>,
        &mut Vec<String>,
        &mut Option<bool>,
        &mut bool,
        &mut Option<String>,
    ) -> AxonResult<()>,
) -> AxonResult<ProviderStreamPartialReport> {
    if raw_stream_chunk.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }

    let mut text_deltas = Vec::new();
    let mut reasoning_deltas = Vec::new();
    let mut end_turn = None;
    let mut reasoning_end = false;
    let mut reasoning_event_key = None;
    let mut provider_event_count = 0usize;
    let lines: Vec<&str> = raw_stream_chunk.lines().collect();
    let has_complete_tail = raw_stream_chunk.ends_with('\n') || raw_stream_chunk.ends_with('\r');
    for (index, line) in lines.iter().enumerate() {
        let Some(payload) = normalize_stream_line(line)? else {
            continue;
        };
        if payload == OPENAI_COMPAT_STREAM_DONE_MARKER {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(payload) {
            Ok(value) => value,
            Err(_) if index + 1 == lines.len() && !has_complete_tail => continue,
            Err(_) => {
                return Err(AxonError::provider_invalid_response(
                    "provider",
                    "provider partial stream JSON 无效",
                ));
            }
        };
        provider_event_count = provider_event_count.saturating_add(1);
        append_event(
            &value,
            &mut text_deltas,
            &mut reasoning_deltas,
            &mut end_turn,
            &mut reasoning_end,
            &mut reasoning_event_key,
        )?;
    }

    if text_deltas.is_empty() && reasoning_deltas.is_empty() && end_turn.is_none() && !reasoning_end
    {
        return Err(AxonError::EmptyInput);
    }

    let deltas = stream_deltas(text_deltas, sequence_offset)?;
    let reasoning_deltas = stream_deltas(reasoning_deltas, sequence_offset)?;

    Ok(ProviderStreamPartialReport {
        deltas,
        reasoning_deltas,
        reasoning_event_key,
        end_turn,
        reasoning_end,
        provider_event_count,
        remote_raw_payload_allowed: false,
        reasons: vec![
            PROVIDER_STREAM_PARTIAL_REASON_NORMALIZED.to_owned(),
            PROVIDER_STREAM_PARTIAL_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
        ],
    })
}

fn stream_deltas(
    text_deltas: Vec<String>,
    sequence_offset: u64,
) -> AxonResult<Vec<AgentResponseStreamDelta>> {
    stream_deltas_with_budget(
        text_deltas,
        sequence_offset,
        ProjectionRuntimeBudget::current(),
    )
}

fn stream_deltas_with_budget(
    text_deltas: Vec<String>,
    sequence_offset: u64,
    budget: ProjectionRuntimeBudget,
) -> AxonResult<Vec<AgentResponseStreamDelta>> {
    let mut deltas = Vec::with_capacity(text_deltas.len());
    for content_delta in text_deltas {
        let mut remaining = content_delta.as_str();
        while !remaining.is_empty() {
            let (chunk, next) = budget.split_text_chunk(remaining);
            let sequence = sequence_offset
                .checked_add(u64::try_from(deltas.len()).map_err(|_| {
                    AxonError::provider_invalid_response(
                        "provider",
                        "provider partial stream delta 过多",
                    )
                })?)
                .ok_or_else(|| {
                    AxonError::provider_invalid_response(
                        "provider",
                        "provider partial stream sequence 溢出",
                    )
                })?;
            deltas.push(AgentResponseStreamDelta {
                sequence,
                content_delta: chunk.to_owned(),
            });
            remaining = next;
        }
    }
    Ok(deltas)
}

pub(super) fn append_openai_partial_event(
    value: &serde_json::Value,
    text_deltas: &mut Vec<String>,
    reasoning_deltas: &mut Vec<String>,
    end_turn: &mut Option<bool>,
    reasoning_end: &mut bool,
    reasoning_event_key: &mut Option<String>,
) -> AxonResult<()> {
    if is_openai_responses_event(ProviderStreamProtocol::OpenAiResponses, value) {
        return append_openai_responses_partial_event(
            value,
            text_deltas,
            reasoning_deltas,
            end_turn,
            reasoning_end,
            reasoning_event_key,
        );
    }
    reject_openai_error_event(value)?;
    let choices = value
        .get("choices")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                ProviderStreamProtocol::OpenAiCompatible.as_code(),
                "OpenAI-compatible partial stream 缺少 choices",
            )
        })?;
    for choice in choices {
        if let Some(content_delta) = choice
            .get("delta")
            .and_then(|delta| delta.get("content"))
            .and_then(serde_json::Value::as_str)
            .filter(|content| !content.is_empty())
        {
            text_deltas.push(content_delta.to_owned());
        }
        if let Some(reasoning_delta) = openai_compatible_reasoning_delta(choice) {
            reasoning_deltas.push(reasoning_delta.to_owned());
        }
        if let Some(reason) = choice
            .get("finish_reason")
            .and_then(serde_json::Value::as_str)
            .filter(|reason| !reason.trim().is_empty())
        {
            *end_turn = Some(reason == OPENAI_CHAT_FINISH_REASON_STOP);
            *reasoning_end = true;
        }
    }
    Ok(())
}

fn append_openai_responses_partial_event(
    value: &serde_json::Value,
    text_deltas: &mut Vec<String>,
    reasoning_deltas: &mut Vec<String>,
    end_turn: &mut Option<bool>,
    reasoning_end: &mut bool,
    reasoning_event_key: &mut Option<String>,
) -> AxonResult<()> {
    reject_openai_responses_error_event(value)?;
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("response.output_text.delta") => {
            if let Some(delta) = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
            {
                text_deltas.push(delta.to_owned());
            }
        }
        event_type if is_openai_responses_reasoning_delta_event(event_type) => {
            *reasoning_event_key = value
                .get("item_id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned);
            if let Some(delta) = value
                .get("delta")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
            {
                reasoning_deltas.push(delta.to_owned());
            }
        }
        Some(OPENAI_RESPONSES_COMPLETED_EVENT) => {
            *end_turn = value
                .pointer("/response/end_turn")
                .and_then(serde_json::Value::as_bool);
        }
        Some(OPENAI_RESPONSES_OUTPUT_ITEM_DONE_EVENT)
            if value
                .pointer("/item/type")
                .and_then(serde_json::Value::as_str)
                == Some(OPENAI_RESPONSES_REASONING_ITEM_TYPE) =>
        {
            *reasoning_event_key = value
                .pointer("/item/id")
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(str::to_owned);
            *reasoning_end = true;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn append_anthropic_partial_event(
    value: &serde_json::Value,
    text_deltas: &mut Vec<String>,
    reasoning_deltas: &mut Vec<String>,
    end_turn: &mut Option<bool>,
    reasoning_end: &mut bool,
    reasoning_event_key: &mut Option<String>,
) -> AxonResult<()> {
    let event_type = value.get("type").and_then(serde_json::Value::as_str);
    if event_type == Some("error") {
        return Err(provider_stream_error_event(
            ANTHROPIC_MESSAGES_STREAM_ERROR_EVENT,
            value.get("error").unwrap_or(value),
        ));
    }
    if event_type == Some(ANTHROPIC_MESSAGE_DELTA_EVENT) {
        if let Some(reason) = value
            .pointer("/delta/stop_reason")
            .and_then(serde_json::Value::as_str)
            .filter(|reason| !reason.trim().is_empty())
        {
            *end_turn = Some(reason == ANTHROPIC_STOP_REASON_END_TURN);
        }
        return Ok(());
    }
    if event_type == Some(ANTHROPIC_CONTENT_BLOCK_STOP_EVENT) {
        *reasoning_event_key = value
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map(|index| index.to_string());
        *reasoning_end = true;
        return Ok(());
    }
    if event_type != Some("content_block_delta") {
        return Ok(());
    }
    if value
        .get("delta")
        .and_then(|delta| delta.get("type"))
        .and_then(serde_json::Value::as_str)
        == Some("thinking_delta")
    {
        *reasoning_event_key = value
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .map(|index| index.to_string());
    }
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
        reasoning_deltas.push(thinking_delta.to_owned());
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
        text_deltas.push(text_delta.to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::ProjectionRuntimeBudget;

    use super::stream_deltas_with_budget;

    #[test]
    fn partial_stream_splits_large_delta_without_changing_content_or_sequence() {
        let budget = ProjectionRuntimeBudget::for_logical_cores(4);
        let content = format!("{}中", "x".repeat(budget.coalesced_text_bytes));
        let deltas =
            stream_deltas_with_budget(vec![content.clone()], 7, budget).expect("大 delta 应可拆分");

        assert!(deltas.len() > 1);
        assert_eq!(
            deltas
                .iter()
                .map(|delta| delta.content_delta.as_str())
                .collect::<String>(),
            content
        );
        assert!(
            deltas
                .iter()
                .enumerate()
                .all(|(index, delta)| delta.sequence == 7 + index as u64)
        );
    }
}
