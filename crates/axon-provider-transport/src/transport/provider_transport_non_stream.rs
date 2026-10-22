//! OpenAI Responses 非流式 fallback。

use std::fmt::Write as _;

use axon_domain::{AxonError, AxonResult};

use crate::{
    ProviderStreamingHttpRequestPlan, ProviderStreamingTransportError,
    ProviderStreamingTransportOutcome,
};

const OPENAI_COMPAT_STREAM_DONE_MARKER: &str = "[DONE]";
const OPENAI_RESPONSES_COMPLETED_EVENT_TYPE: &str = "response.completed";
const OPENAI_RESPONSES_FAILED_EVENT_TYPE: &str = "response.failed";
const OPENAI_RESPONSES_OUTPUT_TEXT_TYPE: &str = "output_text";
const OPENAI_RESPONSES_FUNCTION_CALL_TYPE: &str = "function_call";

pub(super) fn non_stream_responses_body(
    request_plan: &ProviderStreamingHttpRequestPlan,
) -> Option<String> {
    if !request_plan
        .url
        .trim_end_matches('/')
        .ends_with("/responses")
    {
        return None;
    }
    let mut body = serde_json::from_str::<serde_json::Value>(&request_plan.body_text).ok()?;
    if body.get("stream").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    body["stream"] = serde_json::json!(false);
    serde_json::to_string(&body).ok()
}

pub(super) fn callback_non_stream_raw_lines<F>(body: &str, on_sse_line: &mut F)
where
    F: FnMut(&str),
{
    for line in body.lines() {
        if let Some(data) = line.trim().strip_prefix("data:") {
            let data = data.trim();
            if !data.is_empty() && data != OPENAI_COMPAT_STREAM_DONE_MARKER {
                on_sse_line(data);
            }
        }
    }
}

pub(super) fn openai_responses_non_stream_to_raw_stream(body: &str) -> AxonResult<String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| AxonError::provider_failure("OpenAI Responses 非流式 JSON 无效"))?;
    if let Some(error) = value.get("error") {
        let failed = serde_json::json!({
            "type": OPENAI_RESPONSES_FAILED_EVENT_TYPE,
            "response": { "error": error }
        });
        return Ok(format!("data: {}\n", failed));
    }
    let mut events = Vec::new();
    for item in value
        .get("output")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        append_openai_responses_non_stream_item_events(item, &mut events);
    }
    if let Some(text) = value
        .get("output_text")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
    {
        events.push(serde_json::json!({
            "type": "response.output_text.delta",
            "delta": text
        }));
    }
    events.push(serde_json::json!({
        "type": OPENAI_RESPONSES_COMPLETED_EVENT_TYPE,
        "response": value
    }));
    let mut raw = String::new();
    for event in events {
        let _ = writeln!(&mut raw, "data: {event}");
    }
    Ok(raw)
}

pub(super) fn non_stream_conversion_error(
    error: AxonError,
    status: u16,
) -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
        format!("{error:?}"),
        Some(status),
    ))
}

pub(super) fn non_stream_fallback_failed(
    original: ProviderStreamingTransportOutcome,
    fallback: ProviderStreamingTransportOutcome,
) -> ProviderStreamingTransportOutcome {
    let ProviderStreamingTransportOutcome::Error(original_error) = original else {
        return fallback;
    };
    let ProviderStreamingTransportOutcome::Error(fallback_error) = fallback else {
        return fallback;
    };
    let http_status = fallback_error.http_status.or(original_error.http_status);
    ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError {
        message: format!(
            "provider non-stream fallback failed；stream_error={}；fallback_error={}",
            original_error.message, fallback_error.message
        ),
        http_status,
        ..fallback_error
    })
}

fn append_openai_responses_non_stream_item_events(
    item: &serde_json::Value,
    events: &mut Vec<serde_json::Value>,
) {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("message") => append_message_text_events(item, events),
        Some(OPENAI_RESPONSES_FUNCTION_CALL_TYPE) => {
            events.push(serde_json::json!({
                "type": "response.output_item.done",
                "item": item
            }));
        }
        _ => {}
    }
}

fn append_message_text_events(item: &serde_json::Value, events: &mut Vec<serde_json::Value>) {
    for content in item
        .get("content")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if content.get("type").and_then(serde_json::Value::as_str)
            != Some(OPENAI_RESPONSES_OUTPUT_TEXT_TYPE)
        {
            continue;
        }
        if let Some(text) = content
            .get("text")
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
        {
            events.push(serde_json::json!({
                "type": "response.output_text.delta",
                "delta": text
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_stream_response_maps_text_to_responses_events() {
        let body = r#"{"id":"resp_1","output":[{"type":"message","content":[{"type":"output_text","text":"hello"}]}]}"#;
        let raw = openai_responses_non_stream_to_raw_stream(body).unwrap();

        assert!(raw.contains(r#""type":"response.output_text.delta""#));
        assert!(raw.contains(r#""delta":"hello""#));
        assert!(raw.contains(r#""type":"response.completed""#));
    }

    #[test]
    fn non_stream_response_maps_function_call_to_done_item() {
        let body = r#"{"id":"resp_1","output":[{"type":"function_call","call_id":"call_1","name":"builtin_search","arguments":"{\"query\":\"panic\"}"}]}"#;
        let raw = openai_responses_non_stream_to_raw_stream(body).unwrap();

        assert!(raw.contains(r#""type":"response.output_item.done""#));
        assert!(raw.contains(r#""call_id":"call_1""#));
    }
}
