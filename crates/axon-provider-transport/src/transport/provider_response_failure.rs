//! Provider 终止失败事件的统一分类。

use axon_agent_runtime::ProviderStreamingTransportFailureKind;
use serde_json::Value;

const RESPONSE_FAILED_EVENT_TYPE: &str = "response.failed";
const RESPONSE_INCOMPLETE_EVENT_TYPE: &str = "response.incomplete";
const ERROR_EVENT_TYPE: &str = "error";

const CONTEXT_LENGTH_EXCEEDED_CODE: &str = "context_length_exceeded";
const RATE_LIMIT_EXCEEDED_CODE: &str = "rate_limit_exceeded";
const SERVER_OVERLOAD_CODES: &[&str] = &[
    "server_error",
    "internal_error",
    "server_is_overloaded",
    "slow_down",
];
const ACCOUNT_ERROR_CODES: &[&str] = &[
    "authentication_error",
    "invalid_api_key",
    "insufficient_quota",
    "usage_not_included",
];
const REQUEST_ERROR_CODES: &[&str] = &[
    "invalid_prompt",
    "bio_policy",
    "cyber_policy",
    "content_policy_violation",
];

/// 上游链路中断的稳定错误码。
///
/// 中继/网关宣告「无法继续转发上游流」时通常不回传 code；这里给一个稳定值，
/// 让熔断、文案与统计按语义分流，而不是落进无法归因的 `unknown`。
const UPSTREAM_STREAM_ABORTED_CODE: &str = "upstream_stream_aborted";
/// Provider 终态事件既没给 code 也无法按消息归因时的稳定兜底码。
const PROVIDER_EVENT_CODE_UNKNOWN: &str = "provider_event_unknown";
/// 上游流转发中断的消息标记（小写匹配）。
const UPSTREAM_STREAM_ABORTED_MARKERS: &[&str] = &[
    "forward upstream",
    "upstream stream",
    "upstream connection",
    "stream aborted",
    "无法转发上游",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProviderResponseFailure {
    pub(super) message: String,
    pub(super) kind: ProviderStreamingTransportFailureKind,
    /// 触发失败的 Provider 终态事件类型。
    pub(super) event_type: &'static str,
}

pub(super) fn classify_terminal_event(event: &Value) -> Option<ProviderResponseFailure> {
    match event.get("type").and_then(Value::as_str) {
        Some(RESPONSE_INCOMPLETE_EVENT_TYPE) => Some(classify_incomplete(event)),
        Some(RESPONSE_FAILED_EVENT_TYPE) => Some(classify_failed(event)),
        Some(ERROR_EVENT_TYPE) => Some(classify_error_event(event)),
        _ => classify_bare_error(event),
    }
}

/// 没有 `type` 的 in-band 错误对象。
///
/// OpenAI 兼容的中继（含 Gemini 经中继）常在 200 的 SSE 里直接下发
/// `{"error":{"code":503,"message":"...","status":"UNAVAILABLE"}}`，不带 `type`。
/// 漏掉这一形状，传输层只会看到「流结束但没有完成标记」，把一次明确的上游 503 误报成
/// 「provider streaming 缺少完成标记」，读者既看不出是谁失败、也不知道该不该重试。
///
/// 只认**错误对象本身**：`{"error":null}` 与正常 delta 分片都不算终止事件。
fn classify_bare_error(event: &Value) -> Option<ProviderResponseFailure> {
    let error = event.get("error")?;
    if !error.is_object() {
        return None;
    }
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    // 状态码与错误码在线上有三种写法：数字 `code`、数字 `status`、字符串取值（`UNAVAILABLE`）。
    let status = loose_status(error.get("status")).or_else(|| loose_status(error.get("code")));
    let code = loose_code(error.get("code"))
        .or_else(|| loose_code(error.get("status")))
        .unwrap_or_default();
    let code = stable_failure_code(&code, message);
    Some(ProviderResponseFailure {
        message: format!("provider 请求失败：code={code}；{message}"),
        kind: failure_kind_for_status(status)
            .or_else(|| failure_kind_for_code(&code))
            .or_else(|| failure_kind_for_bare_status(error))
            .unwrap_or_else(|| failure_kind_for_message(message)),
        event_type: ERROR_EVENT_TYPE,
    })
}

/// 数值状态码：数字直接用，字符串按数字解析；认不出时为 None。
fn loose_status(value: Option<&Value>) -> Option<u16> {
    match value? {
        Value::Number(number) => number.as_u64().and_then(|raw| u16::try_from(raw).ok()),
        Value::String(text) => text.trim().parse::<u16>().ok(),
        _ => None,
    }
}

/// 错误码：数字转成十进制串，字符串去空白；空串视为没给。
fn loose_code(value: Option<&Value>) -> Option<String> {
    let code = match value? {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.trim().to_owned(),
        _ => return None,
    };
    (!code.is_empty()).then_some(code)
}

/// 只有字符串状态（如 Gemini 的 `UNAVAILABLE`）时的归因。
///
/// 这些取值都是「服务端此刻不可用」，与 HTTP 5xx 同一档：可重试，不是请求本身有问题。
fn failure_kind_for_bare_status(error: &Value) -> Option<ProviderStreamingTransportFailureKind> {
    let status = error
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    matches!(
        status.as_str(),
        "UNAVAILABLE" | "OVERLOADED" | "INTERNAL" | "DEADLINE_EXCEEDED" | "ABORTED"
    )
    .then_some(ProviderStreamingTransportFailureKind::Upstream)
}

fn classify_incomplete(event: &Value) -> ProviderResponseFailure {
    let reason = event
        .pointer("/response/incomplete_details/reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    ProviderResponseFailure {
        message: format!("provider response incomplete：reason={reason}"),
        kind: ProviderStreamingTransportFailureKind::Incomplete,
        event_type: RESPONSE_INCOMPLETE_EVENT_TYPE,
    }
}

fn classify_failed(event: &Value) -> ProviderResponseFailure {
    let code = event
        .pointer("/response/error/code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let message = event
        .pointer("/response/error/message")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let code = stable_failure_code(code, message);
    ProviderResponseFailure {
        message: format!("provider response failed：code={code}；{message}"),
        kind: failure_kind_for_code(&code).unwrap_or_else(|| failure_kind_for_message(message)),
        event_type: RESPONSE_FAILED_EVENT_TYPE,
    }
}

fn classify_error_event(event: &Value) -> ProviderResponseFailure {
    let status = event
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|status| u16::try_from(status).ok());
    let code = event
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let message = event
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let code = stable_failure_code(code, message);
    ProviderResponseFailure {
        message: format!("provider websocket error：code={code}；{message}"),
        kind: failure_kind_for_status(status)
            .or_else(|| failure_kind_for_code(&code))
            .unwrap_or_else(|| failure_kind_for_message(message)),
        event_type: ERROR_EVENT_TYPE,
    }
}

fn failure_kind_for_status(status: Option<u16>) -> Option<ProviderStreamingTransportFailureKind> {
    match status {
        Some(408) => Some(ProviderStreamingTransportFailureKind::Timeout),
        Some(429) => Some(ProviderStreamingTransportFailureKind::RateLimited),
        Some(401 | 403) => Some(ProviderStreamingTransportFailureKind::Authentication),
        Some(status) if status >= 500 => Some(ProviderStreamingTransportFailureKind::Upstream),
        Some(status) if (400..500).contains(&status) => {
            Some(ProviderStreamingTransportFailureKind::Request)
        }
        _ => None,
    }
}

fn failure_kind_for_code(code: &str) -> Option<ProviderStreamingTransportFailureKind> {
    if code == CONTEXT_LENGTH_EXCEEDED_CODE {
        Some(ProviderStreamingTransportFailureKind::ContextOverflow)
    } else if REQUEST_ERROR_CODES.contains(&code) {
        Some(ProviderStreamingTransportFailureKind::Request)
    } else if code == RATE_LIMIT_EXCEEDED_CODE {
        Some(ProviderStreamingTransportFailureKind::RateLimited)
    } else if SERVER_OVERLOAD_CODES.contains(&code) {
        Some(ProviderStreamingTransportFailureKind::Upstream)
    } else if ACCOUNT_ERROR_CODES.contains(&code) {
        Some(ProviderStreamingTransportFailureKind::Authentication)
    } else if code == UPSTREAM_STREAM_ABORTED_CODE {
        Some(ProviderStreamingTransportFailureKind::Upstream)
    } else {
        None
    }
}

/// 把 Provider 给不出 code 的终态事件归因到稳定错误码。
///
/// Provider 只回一句「无法转发上游流」时，旧行为会把 `unknown` 当成错误码写进失败记录，
/// 渠道熔断与界面文案因此都无法分流；这里按消息归因，归不出才用兜底码。
fn stable_failure_code(code: &str, message: &str) -> String {
    let trimmed = code.trim();
    if !trimmed.is_empty() && trimmed != "unknown" {
        return trimmed.to_owned();
    }
    if is_upstream_stream_aborted(message) {
        return UPSTREAM_STREAM_ABORTED_CODE.to_owned();
    }
    PROVIDER_EVENT_CODE_UNKNOWN.to_owned()
}

fn is_upstream_stream_aborted(message: &str) -> bool {
    let normalized = message.to_lowercase();
    UPSTREAM_STREAM_ABORTED_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

/// 无法按 code 归因时按消息归因；仍认不出才算 Unknown。
fn failure_kind_for_message(message: &str) -> ProviderStreamingTransportFailureKind {
    if is_upstream_stream_aborted(message) {
        ProviderStreamingTransportFailureKind::Upstream
    } else {
        ProviderStreamingTransportFailureKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_events_have_stable_retry_classification() {
        let cases = [
            (
                serde_json::json!({
                    "type": "response.incomplete",
                    "response": {"incomplete_details": {"reason": "max_output_tokens"}}
                }),
                ProviderStreamingTransportFailureKind::Incomplete,
            ),
            (
                failed_event("rate_limit_exceeded"),
                ProviderStreamingTransportFailureKind::RateLimited,
            ),
            (
                failed_event("server_is_overloaded"),
                ProviderStreamingTransportFailureKind::Upstream,
            ),
            (
                failed_event("invalid_prompt"),
                ProviderStreamingTransportFailureKind::Request,
            ),
            (
                failed_event("insufficient_quota"),
                ProviderStreamingTransportFailureKind::Authentication,
            ),
        ];

        for (event, expected) in cases {
            assert_eq!(classify_terminal_event(&event).unwrap().kind, expected);
        }
    }

    #[test]
    fn context_overflow_keeps_code_in_message_for_shared_recovery() {
        let failure = classify_terminal_event(&failed_event(CONTEXT_LENGTH_EXCEEDED_CODE)).unwrap();

        assert_eq!(
            failure.kind,
            ProviderStreamingTransportFailureKind::ContextOverflow
        );
        assert!(failure.message.contains(CONTEXT_LENGTH_EXCEEDED_CODE));
    }

    #[test]
    fn code_less_upstream_stream_abort_is_attributed_not_unknown() {
        // 真机样本：中继回了 response.failed 但不给 code，只说无法继续转发上游流。
        let failure = classify_terminal_event(&serde_json::json!({
            "type": "response.failed",
            "response": {"error": {"message": "Unable to forward upstream stream"}}
        }))
        .unwrap();

        assert_eq!(
            failure.kind,
            ProviderStreamingTransportFailureKind::Upstream
        );
        assert_eq!(failure.event_type, "response.failed");
        assert!(
            failure.message.contains("code=upstream_stream_aborted"),
            "归因必须落成稳定错误码：{}",
            failure.message
        );
    }

    #[test]
    fn unrecognized_code_less_failure_keeps_stable_fallback_code() {
        let failure = classify_terminal_event(&serde_json::json!({
            "type": "response.failed",
            "response": {"error": {"message": "something we have never seen"}}
        }))
        .unwrap();

        assert_eq!(failure.kind, ProviderStreamingTransportFailureKind::Unknown);
        assert!(failure.message.contains("code=provider_event_unknown"));
        assert!(
            !failure.message.contains("code=unknown；"),
            "不得再落进无法归因的 unknown：{}",
            failure.message
        );
    }

    fn failed_event(code: &str) -> Value {
        serde_json::json!({
            "type": "response.failed",
            "response": {"error": {"code": code, "message": "failed"}}
        })
    }

    #[test]
    fn bare_error_object_is_attributed_to_upstream_overload() {
        // 真机样本：中继在 200 的 SSE 里直接下发错误对象，不带 type。
        let failure = classify_terminal_event(&serde_json::json!({
            "error": {
                "code": 503,
                "message": "The service is currently unavailable. Please try again later.",
                "status": "UNAVAILABLE"
            }
        }))
        .expect("这形状必须被认成终止事件");

        assert_eq!(
            failure.kind,
            ProviderStreamingTransportFailureKind::Upstream
        );
        assert_eq!(failure.event_type, ERROR_EVENT_TYPE);
        assert!(
            failure.message.contains("code=503"),
            "数字错误码要原样带出来：{}",
            failure.message
        );
        assert!(
            failure.message.contains("currently unavailable"),
            "上游原话要留住：{}",
            failure.message
        );
    }

    #[test]
    fn bare_error_with_only_a_status_word_still_reads_as_upstream() {
        let failure = classify_terminal_event(&serde_json::json!({
            "error": {"message": "overloaded", "status": "UNAVAILABLE"}
        }))
        .expect("只有字符串状态也要认");

        assert_eq!(
            failure.kind,
            ProviderStreamingTransportFailureKind::Upstream
        );
    }

    #[test]
    fn normal_chunks_and_null_error_are_not_terminal_events() {
        // `{"error":null}` 与正常 delta 分片都不是终止事件，不许被误判成失败。
        assert!(
            classify_terminal_event(&serde_json::json!({"error": null, "choices": []})).is_none()
        );
        assert!(
            classify_terminal_event(&serde_json::json!({
                "choices": [{"delta": {"content": "继续"}, "finish_reason": null}],
                "id": "chunk-1"
            }))
            .is_none()
        );
    }
}
