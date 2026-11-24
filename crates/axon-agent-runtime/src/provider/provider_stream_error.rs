//! Provider stream error event 结构化投影。
//!
//! 该模块集中维护 provider streaming 与 tool-call streaming 共享的错误事件语义，
//! 避免不同归一化路径输出结构漂移。

use axon_domain::AxonError;

/// Provider stream error event 的内部消息前缀。
pub const PROVIDER_STREAM_ERROR_EVENT_PREFIX: &str = "provider_stream_error_event:";

/// 将 provider stream 中的 error event 投影为可被 CLI 识别的结构化 transport error。
pub(crate) fn provider_stream_error_event(
    summary: &'static str,
    error: &serde_json::Value,
) -> AxonError {
    AxonError::provider_transport(provider_stream_error_json(summary, error), None)
}

fn provider_stream_error_json(summary: &'static str, error: &serde_json::Value) -> String {
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(summary);
    let value = serde_json::json!({
        "error": {
            "message": message,
            "type": error_string_field(error, "type"),
            "code": error_string_field(error, "code"),
            "param": error_string_field(error, "param"),
        }
    });
    format!("{PROVIDER_STREAM_ERROR_EVENT_PREFIX}{value}")
}

fn error_string_field<'a>(error: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    error.get(field).and_then(|value| match value {
        serde_json::Value::String(value) => Some(value.as_str()),
        _ => None,
    })
}
