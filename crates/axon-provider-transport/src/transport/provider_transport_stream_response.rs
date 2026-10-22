//! Provider HTTP SSE 响应读取与完成判定。

use std::{fmt, time::Duration};

use axon_agent_runtime::{ProviderStreamingTransportFailureKind, RunCancelToken};
use eventsource_stream::{EventStreamError, Eventsource as _};
use futures_util::StreamExt as _;
use reqwest::Response;
use tokio::time::Instant as TokioInstant;

use crate::{ProviderStreamingTransportError, ProviderStreamingTransportOutcome};

use super::{provider_response_failure::classify_terminal_event, transport_error};

const OPENAI_COMPAT_STREAM_DONE_MARKER: &str = "[DONE]";
const OPENAI_RESPONSES_COMPLETED_EVENT_TYPE: &str = "response.completed";
const ANTHROPIC_MESSAGE_STOP_EVENT_TYPE: &str = "message_stop";
/// 上游请求 ID 的响应头；边缘网关与中继常用这两个名字。
const UPSTREAM_REQUEST_ID_HEADERS: &[&str] = &["x-request-id", "request-id"];

#[derive(Debug)]
enum ProviderBodyError {
    Transport(reqwest::Error),
}

impl fmt::Display for ProviderBodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ProviderBodyError {}

pub(super) async fn response_to_raw_stream(
    response: Response,
    timeout_ms: u64,
    cancel_token: &RunCancelToken,
) -> ProviderStreamingTransportOutcome {
    response_to_raw_stream_with_callback(
        response,
        timeout_ms,
        &mut |_| {},
        &mut || {},
        cancel_token,
    )
    .await
}

pub(super) async fn response_to_text(
    response: Response,
    cancel_token: &RunCancelToken,
) -> Result<String, ProviderStreamingTransportOutcome> {
    response_to_text_with_progress(response, cancel_token, &mut || {}).await
}

pub(super) async fn response_to_text_with_progress<F>(
    response: Response,
    cancel_token: &RunCancelToken,
    on_progress: &mut F,
) -> Result<String, ProviderStreamingTransportOutcome>
where
    F: FnMut(),
{
    let status = response.status().as_u16();
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    let mut progress_sleep = Box::pin(tokio::time::sleep(Duration::from_millis(250)));
    loop {
        let next = tokio::select! {
            next = stream.next() => next,
            () = &mut progress_sleep => {
                on_progress();
                progress_sleep
                    .as_mut()
                    .reset(TokioInstant::now() + Duration::from_millis(250));
                continue;
            }
            () = cancel_token.cancelled() => {
                return Err(transport_error("provider streaming 已取消", None));
            }
        };
        let Some(next) = next else {
            break;
        };
        let bytes = next.map_err(|_| {
            ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new("provider 响应读取失败", Some(status))
                    .with_failure_kind(ProviderStreamingTransportFailureKind::Connection),
            )
        })?;
        body.extend_from_slice(&bytes);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

pub(super) async fn response_to_raw_stream_with_callback<F, G>(
    response: Response,
    timeout_ms: u64,
    on_sse_line: &mut F,
    on_progress: &mut G,
    cancel_token: &RunCancelToken,
) -> ProviderStreamingTransportOutcome
where
    F: FnMut(&str),
    G: FnMut(),
{
    let status = response.status().as_u16();
    let upstream_request_id = response_request_id(&response);
    let guarded_body = response.bytes_stream().map(move |result| match result {
        Ok(bytes) => Ok(bytes),
        Err(error) => Err(ProviderBodyError::Transport(error)),
    });
    let mut stream = guarded_body.eventsource();
    let mut completed = false;
    let idle_timeout = Duration::from_millis(timeout_ms);
    let mut idle_sleep = Box::pin(tokio::time::sleep(idle_timeout));
    let mut progress_sleep = Box::pin(tokio::time::sleep(Duration::from_millis(250)));
    loop {
        let event = tokio::select! {
            event = stream.next() => event,
            () = &mut idle_sleep => {
                return read_timeout(status);
            }
            () = &mut progress_sleep => {
                on_progress();
                progress_sleep
                    .as_mut()
                    .reset(TokioInstant::now() + Duration::from_millis(250));
                continue;
            }
            () = cancel_token.cancelled() => {
                return transport_error("provider streaming 已取消", None);
            }
        };
        let Some(event) = event else {
            break;
        };
        match event {
            Ok(event) => {
                idle_sleep
                    .as_mut()
                    .reset(TokioInstant::now() + idle_timeout);
                let data = normalized_sse_event_data(&event.event, &event.data);
                let data = data.trim();
                if !data.is_empty() && data != OPENAI_COMPAT_STREAM_DONE_MARKER {
                    on_sse_line(data);
                }
                if let Some(failure) = stream_terminal_failure(data) {
                    return ProviderStreamingTransportOutcome::Error(
                        ProviderStreamingTransportError::new(failure.message, Some(status))
                            .with_failure_kind(failure.kind)
                            .with_provider_event_type(Some(failure.event_type.to_owned()))
                            .with_request_id(upstream_request_id.clone()),
                    );
                }
                if stream_data_marks_completed(data) {
                    completed = true;
                    break;
                }
            }
            Err(error) => return stream_read_error(error, status),
        }
    }
    if !completed {
        return ProviderStreamingTransportOutcome::Error(
            ProviderStreamingTransportError::new("provider streaming 缺少完成标记", Some(status))
                .with_failure_kind(ProviderStreamingTransportFailureKind::Incomplete),
        );
    }
    ProviderStreamingTransportOutcome::StreamCompleted {
        http_status: status,
    }
}

fn read_timeout(status: u16) -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(
        ProviderStreamingTransportError::new("provider streaming 响应读取超时", Some(status))
            .with_failure_kind(ProviderStreamingTransportFailureKind::Timeout),
    )
}

fn stream_read_error(
    error: EventStreamError<ProviderBodyError>,
    status: u16,
) -> ProviderStreamingTransportOutcome {
    match error {
        EventStreamError::Transport(ProviderBodyError::Transport(error)) if error.is_timeout() => {
            read_timeout(status)
        }
        EventStreamError::Transport(ProviderBodyError::Transport(_)) => {
            ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new(
                    "provider streaming 响应读取失败",
                    Some(status),
                )
                .with_failure_kind(ProviderStreamingTransportFailureKind::Connection),
            )
        }
        EventStreamError::Utf8(_) | EventStreamError::Parser(_) => {
            ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new(
                    "provider streaming SSE 响应解析失败",
                    Some(status),
                )
                .with_failure_kind(ProviderStreamingTransportFailureKind::Protocol),
            )
        }
    }
}

pub(super) fn normalized_sse_event_data(event_name: &str, data: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.trim().to_owned();
    };
    let event_name = event_name.trim();
    if event_name.is_empty() || event_name == "message" {
        return serde_json::to_string(&value).unwrap_or_else(|_| data.trim().to_owned());
    }
    if let Some(object) = value.as_object_mut() {
        object
            .entry("type")
            .or_insert_with(|| serde_json::Value::String(event_name.to_owned()));
    }
    serde_json::to_string(&value).unwrap_or_else(|_| data.trim().to_owned())
}

pub(super) fn stream_data_marks_completed(data: &str) -> bool {
    if data == OPENAI_COMPAT_STREAM_DONE_MARKER {
        return true;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    matches!(
        value.get("type").and_then(serde_json::Value::as_str),
        Some(OPENAI_RESPONSES_COMPLETED_EVENT_TYPE | ANTHROPIC_MESSAGE_STOP_EVENT_TYPE)
    )
}

fn stream_terminal_failure(
    data: &str,
) -> Option<super::provider_response_failure::ProviderResponseFailure> {
    let value = serde_json::from_str::<serde_json::Value>(data).ok()?;
    classify_terminal_event(&value)
}

/// 取上游请求 ID；响应头缺失或不可读时返回 None。
fn response_request_id(response: &Response) -> Option<String> {
    UPSTREAM_REQUEST_ID_HEADERS.iter().find_map(|header| {
        let value = response.headers().get(*header)?.to_str().ok()?;
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}
