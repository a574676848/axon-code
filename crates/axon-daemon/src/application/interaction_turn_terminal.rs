//! Primary Turn 失败时的统一 durable 收口事件。

use axon_protocol::wire::{
    WireAssistantMessage, WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase,
    WireLifecycleStatus, WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
};
use axon_session_runtime::{
    RuntimeEvent, model::TURN_COMPLETED_EVENT, wire_event_to_runtime_event,
};

const WIRE_EVENT_TYPE: &str = "wire_event";
const RUN_LIFECYCLE_KIND: &str = "RunLifecycle";
const RUN_TURN_ID: &str = "run";
const ASSISTANT_PART_ID: &str = "assistant:daemon-primary";
const ASSISTANT_MESSAGE_ID: &str = "assistant-message-final";
const ASSISTANT_CORRELATION_ID: &str = "assistant";
const FAILED_RESPONSE_PREFIX: &str = "本轮执行未完成：";
/// 用户主动中断时展示的正文；与失败区分开，避免把「我让它停的」说成「它没做成」。
const INTERRUPTED_RESPONSE: &str = "本轮已由用户中断。";
/// 结构化失败载荷的起始标记；之前的自然语言前缀保持原样展示。
const STRUCTURED_FAILURE_MARKER: &str = "{";

/// 一次终态收口的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnFailureKind {
    /// 执行失败。
    Failed,
    /// 用户主动中断。
    Interrupted,
}

impl TurnFailureKind {
    /// `turn_completed` 载荷里的状态值。
    const fn status(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

/// 一次失败的可展示字段；客户端据此画失败卡，不必再去解析正文里的载荷。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TurnFailureReport {
    pub(super) category: String,
    pub(super) code: String,
    pub(super) redacted_message: String,
    pub(super) http_status: Option<u64>,
    pub(super) retryable: bool,
    /// 渠道链已执行的尝试次数。
    pub(super) attempt_count: Option<u64>,
    /// 触发失败的 Provider 终态事件类型（如 `response.failed`）。
    pub(super) provider_event_type: Option<String>,
    /// Provider 或边缘网关返回的请求 ID。
    pub(super) upstream_request_id: Option<String>,
}

impl TurnFailureReport {
    /// 全部字段都空时视为没有可用结构。
    fn is_empty(&self) -> bool {
        self.category.is_empty()
            && self.code.is_empty()
            && self.redacted_message.is_empty()
            && self.http_status.is_none()
    }

    /// 投影成终态载荷里的 `failure` 对象。
    fn to_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "category": self.category,
            "code": self.code,
            "redactedMessage": self.redacted_message,
            "httpStatus": self.http_status,
            "retryable": self.retryable,
            "attemptCount": self.attempt_count,
            "providerEventType": self.provider_event_type,
            "upstreamRequestId": self.upstream_request_id,
        })
    }
}

/// 把 CLI 的失败回执整理成可读结论，并判定终态种类。
///
/// CLI 在失败时会输出一整段 `run_failed` JSON；直接塞进正文会让用户看到一屏原始载荷。
/// 这里优先取结构化字段里的脱敏结论，取不到才回落到原文；同时把结构化字段单独返回，
/// 客户端画失败卡时用得上（正文只留一句人话）。
pub(super) fn classify_turn_failure(error: &str) -> (TurnFailureKind, String, TurnFailureReport) {
    let trimmed = error.trim();
    // 用户中断必须先判，而且不能依赖整段 JSON 能解析：Provider 失败明细有长度上限
    // （CLI 的 `PROVIDER_FAILURE_DETAIL_MAX_CHARS`），结构化载荷可能在中途被截断，
    // 而「用户让它停」正是最常见的失败明细。只看完整解析会把中断误判成执行失败，
    // 进而把 Interaction 记成 rejected，用户看到的就是「本轮被后端拒绝」。
    if is_interrupted_payload(trimmed) {
        return (
            TurnFailureKind::Interrupted,
            INTERRUPTED_RESPONSE.to_owned(),
            TurnFailureReport::default(),
        );
    }
    let Some(start) = trimmed.find(STRUCTURED_FAILURE_MARKER) else {
        return (
            TurnFailureKind::Failed,
            format!("{FAILED_RESPONSE_PREFIX}{trimmed}"),
            TurnFailureReport::default(),
        );
    };
    let (prefix, payload) = trimmed.split_at(start);
    let facts = failure_facts(payload);
    if facts.category == axon_domain::RUNTIME_FAILURE_CATEGORY_INTERRUPTED
        || is_interrupted_payload(payload)
    {
        return (
            TurnFailureKind::Interrupted,
            INTERRUPTED_RESPONSE.to_owned(),
            TurnFailureReport::default(),
        );
    }
    let report = TurnFailureReport {
        category: facts.category.clone(),
        code: facts.code.clone().unwrap_or_default(),
        redacted_message: facts.detail.clone().unwrap_or_default(),
        http_status: facts.http_status,
        retryable: facts.retryable,
        attempt_count: facts.attempt_count,
        provider_event_type: facts.provider_event_type.clone(),
        upstream_request_id: facts.upstream_request_id.clone(),
    };
    let summary = match (facts.detail, facts.code) {
        (Some(detail), Some(code)) => format!("{detail}（{code}）"),
        (Some(detail), None) => detail,
        (None, Some(code)) => code,
        (None, None) => prefix.trim().trim_end_matches(['：', ':']).to_owned(),
    };
    (
        TurnFailureKind::Failed,
        format!("{FAILED_RESPONSE_PREFIX}{summary}"),
        report,
    )
}

/// 从结构化载荷里取出可诊断事实；载荷被截断时按字段名就地扫。
///
/// 两条路径给出同一组事实：完整时可走 JSON，截断后 JSON 整段不可解析，
/// 但字段本身往往完好，扫描能拿到同样的结论。
fn failure_facts(payload: &str) -> FailureFacts {
    let parsed = serde_json::from_str::<serde_json::Value>(payload).ok();
    let from_json = |field: &str| -> Option<serde_json::Value> {
        parsed
            .as_ref()
            .and_then(|value| value.get("failure"))
            .and_then(|failure| failure.get(field))
            .cloned()
    };
    let json_text = |field: &str| -> Option<String> {
        from_json(field)
            .and_then(|value| value.as_str().map(str::to_owned))
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    FailureFacts {
        category: json_text("category")
            .or_else(|| scan_json_string_field(payload, "category"))
            .unwrap_or_default(),
        detail: json_text("redactedMessage")
            .or_else(|| scan_json_string_field(payload, "redactedMessage")),
        code: json_text("code").or_else(|| scan_json_string_field(payload, "code")),
        http_status: from_json("httpStatus")
            .and_then(|value| value.as_u64())
            .or_else(|| {
                parsed
                    .as_ref()
                    .and_then(|value| value.get("httpStatus"))
                    .and_then(serde_json::Value::as_u64)
            })
            .or_else(|| scan_json_u64_field(payload, "httpStatus")),
        retryable: from_json("retryable")
            .and_then(|value| value.as_bool())
            .or_else(|| scan_json_bool_field(payload, "retryable"))
            .unwrap_or(false),
        attempt_count: from_json("attemptCount")
            .and_then(|value| value.as_u64())
            .or_else(|| scan_json_u64_field(payload, "attemptCount")),
        provider_event_type: json_text("providerEventType")
            .or_else(|| scan_json_string_field(payload, "providerEventType")),
        upstream_request_id: json_text("upstreamRequestId")
            .or_else(|| scan_json_string_field(payload, "upstreamRequestId")),
    }
}

/// 结构化载荷里的事实。
struct FailureFacts {
    category: String,
    detail: Option<String>,
    code: Option<String>,
    http_status: Option<u64>,
    retryable: bool,
    attempt_count: Option<u64>,
    provider_event_type: Option<String>,
    upstream_request_id: Option<String>,
}

/// 按字段名扫一个整数字段：取第一个真正带数字的出现位置。
///
/// 同名键可能出现两次（顶层 `httpStatus` 可能是 null，failure 里的才是真值），
/// 因此命不中数字就继续往后找，而不是拿第一个 null 当结论。
fn scan_json_u64_field(payload: &str, field: &str) -> Option<u64> {
    let needle = format!("\"{field}\":");
    let mut cursor = 0_usize;
    while let Some(offset) = payload[cursor..].find(&needle) {
        let start = cursor + offset + needle.len();
        let digits = payload[start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if let Ok(value) = digits.parse::<u64>() {
            return Some(value);
        }
        cursor = start;
    }
    None
}

/// 按字段名扫一个布尔字段：同样取第一个真正的布尔出现位置。
fn scan_json_bool_field(payload: &str, field: &str) -> Option<bool> {
    let needle = format!("\"{field}\":");
    let mut cursor = 0_usize;
    while let Some(offset) = payload[cursor..].find(&needle) {
        let start = cursor + offset + needle.len();
        let rest = &payload[start..];
        if rest.starts_with("true") {
            return Some(true);
        }
        if rest.starts_with("false") {
            return Some(false);
        }
        cursor = start;
    }
    None
}

/// 从可能被截断的 JSON 文本里捞一个字符串字段。
///
/// 只做一件事：找到 `"<field>":"` 之后读到下一个未转义的双引号，并还原最小转义集。
/// 不追求完整 JSON 语义：载荷被截断时它正是唯一还能取到结论的路径。
fn scan_json_string_field(payload: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\":\"");
    let start = payload.find(&needle)? + needle.len();
    let mut value = String::new();
    let mut escaped = false;
    for character in payload[start..].chars() {
        if escaped {
            match character {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                other => value.push(other),
            }
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => break,
            other => value.push(other),
        }
    }
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn is_interrupted_payload(payload: &str) -> bool {
    // 判据是「字段名 + 取值」而不是裸的 interrupted 词：Provider 的连接类错误里
    // 也可能出现这个词，不能把它当成人打断的结论。
    fn declares(payload: &str, field: &str, value: &str) -> bool {
        payload.contains(&format!("\"{field}\":\"{value}\""))
    }
    declares(
        payload,
        "category",
        axon_domain::RUNTIME_FAILURE_CATEGORY_INTERRUPTED,
    ) || declares(
        payload,
        "code",
        axon_domain::RUNTIME_FAILURE_CODE_INTERRUPTED,
    )
}

pub(super) struct FailedTurnTerminalInput<'a> {
    pub(super) action_id: &'a str,
    pub(super) action_kind: &'a str,
    pub(super) session_id: &'a str,
    pub(super) kind: TurnFailureKind,
    pub(super) response: &'a str,
    /// 结构化失败事实；客户端据此画失败卡。
    pub(super) failure: TurnFailureReport,
    pub(super) transport_context: serde_json::Value,
    pub(super) recorded_at_ms: u64,
    /// 本轮降级告警（含人格装配告警）；失败路径同样要透传。
    pub(super) warnings: &'a [String],
}

pub(super) fn failed_turn_terminal_events(
    input: FailedTurnTerminalInput<'_>,
) -> Result<Vec<RuntimeEvent>, String> {
    let response = input.response.to_owned();
    let recorded_at_ms = i64::try_from(input.recorded_at_ms).unwrap_or(i64::MAX);
    let session = WireSessionRef {
        session_id: input.session_id.to_owned(),
        run_id: None,
        parent_session_id: None,
    };
    let assistant = wire_event_to_runtime_event(&WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(format!("{}:{ASSISTANT_PART_ID}", input.session_id)),
        correlation_id: WireCorrelationId(ASSISTANT_CORRELATION_ID.to_owned()),
        session: session.clone(),
        sequence: 1,
        recorded_at_ms,
        kind: WireEventKind::AssistantMessage(WireAssistantMessage {
            part_id: ASSISTANT_PART_ID.to_owned(),
            message_id: ASSISTANT_MESSAGE_ID.to_owned(),
            content: response.clone(),
            delta: false,
        }),
    })?;
    let lifecycle = wire_event_to_runtime_event(&WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(format!("{}:run:failed", input.session_id)),
        correlation_id: WireCorrelationId(RUN_TURN_ID.to_owned()),
        session,
        sequence: 2,
        recorded_at_ms,
        kind: WireEventKind::TurnLifecycle(WireTurnLifecycle {
            part_id: format!("lifecycle:{RUN_TURN_ID}"),
            turn_id: RUN_TURN_ID.to_owned(),
            phase: WireLifecyclePhase::RegularTurn,
            status: WireLifecycleStatus::Failed,
            title: "Run failed".to_owned(),
            kind: Some(RUN_LIFECYCLE_KIND.to_owned()),
            ui_phase: None,
            tool_name: None,
            attempt_index: None,
            summary: Some(input.kind.status().to_owned()),
            duration_ms: None,
            started_at_ms: None,
            total_tokens: None,
            cache_hit_rate_basis_points: None,

            warnings: input.warnings.to_vec(),
        }),
    })?;
    let mut payload = serde_json::json!({
        "actionId": input.action_id,
        "kind": input.action_kind,
        "response": response,
        "status": input.kind.status(),
        "transportContext": input.transport_context,
    });
    // 结构化失败只在这里出现一次：正文留一句人话，字段留给界面画卡。
    if !input.failure.is_empty() {
        payload["failure"] = input.failure.to_payload();
    }
    let completed = RuntimeEvent {
        event_type: TURN_COMPLETED_EVENT.to_owned(),
        payload_json: payload.to_string(),
    };
    debug_assert_eq!(assistant.event_type, WIRE_EVENT_TYPE);
    Ok(vec![assistant, lifecycle, completed])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一条与生产同形的失败明细：自然语言前缀 + `run_failed` 载荷。
    fn provider_failure(failure_json: &str) -> String {
        let payload = format!(
            r#"{{"event":"run_failed","status":"failed","sessionId":"session-1","resumedFrom":"session-1","provider":"cpa","model":"gemini-3.8-flash-high","selectedSkillName":null,"selectedSkillSourceRef":null,"httpStatus":null,"content":null,"failure":{failure_json},"providerError":null,"channelAttempts":[],"warnings":[]}}"#
        );
        format!("Provider turn 执行失败，退出码 1：{payload}")
    }

    /// 按 CLI 的长度上限截断失败明细；`PROVIDER_FAILURE_DETAIL_MAX_CHARS` 就是 600。
    fn truncated(error: &str) -> String {
        let mut truncated = error.chars().take(600).collect::<String>();
        truncated.push_str("...");
        truncated
    }

    #[test]
    fn truncated_interrupted_payload_is_still_classified_as_interrupt() {
        // 回归：失败明细被截断后整段 JSON 无法解析，只看完整解析会把用户中断记成
        // 执行失败，Interaction 随即变成 rejected，界面显示「本轮被后端拒绝」。
        let error = truncated(&provider_failure(
            r#"{"category":"interrupted","code":"run_interrupted","redactedMessage":"任务已由用户中断"}"#,
        ));
        assert!(
            serde_json::from_str::<serde_json::Value>(&error[error.find('{').unwrap_or(0)..])
                .is_err(),
            "这条用例必须真的是截断载荷，否则它证明不了任何事"
        );

        let (kind, response, _) = classify_turn_failure(&error);
        assert_eq!(kind, TurnFailureKind::Interrupted);
        assert_eq!(response, INTERRUPTED_RESPONSE);
    }

    #[test]
    fn complete_payload_with_spaced_fields_keeps_its_conclusion() {
        // 带空格的 JSON 命不中紧凑标记，必须由解析路径给出同一个结论。
        let error = provider_failure(r#"{ "category": "interrupted", "code": "run_interrupted" }"#);
        let (kind, response, _) = classify_turn_failure(&error);
        assert_eq!(kind, TurnFailureKind::Interrupted);
        assert_eq!(response, INTERRUPTED_RESPONSE);
    }

    #[test]
    fn provider_text_mentioning_interrupted_is_not_a_user_interrupt() {
        // 连接类错误里也会出现 interrupted 字样；裸词判定会把「它没做成」说成「我让它停的」。
        let (kind, response, _) = classify_turn_failure(
            "Provider turn 执行失败，退出码 1：connection interrupted, retry later",
        );
        assert_eq!(kind, TurnFailureKind::Failed);
        assert!(response.starts_with(FAILED_RESPONSE_PREFIX));
    }

    #[test]
    fn structured_failure_keeps_readable_detail_and_code() {
        let error = provider_failure(
            r#"{"category":"provider_transport","code":"upstream_503","redactedMessage":"上游返回 503","httpStatus":503,"retryable":true}"#,
        );
        let (kind, response, report) = classify_turn_failure(&error);
        assert_eq!(kind, TurnFailureKind::Failed);
        assert_eq!(
            response,
            format!("{FAILED_RESPONSE_PREFIX}上游返回 503（upstream_503）")
        );
        // 结构化字段同时交给客户端：正文只留一句人话，画卡所需的事实不丢。
        assert_eq!(report.category, "provider_transport");
        assert_eq!(report.code, "upstream_503");
        assert_eq!(report.redacted_message, "上游返回 503");
        assert_eq!(report.http_status, Some(503));
        assert!(report.retryable);
    }

    #[test]
    fn truncated_payload_still_yields_a_readable_reason() {
        let error = truncated(&provider_failure(
            r#"{"category":"provider_transport","code":"invalid_parameter","redactedMessage":"Image too small (area=64px), minimum required area: >256px","httpStatus":400,"retryable":true}"#,
        ));
        assert!(
            serde_json::from_str::<serde_json::Value>(&error[error.find('{').unwrap_or(0)..])
                .is_err(),
            "这条用例必须真的是截断载荷"
        );

        let (kind, response, report) = classify_turn_failure(&error);

        assert_eq!(kind, TurnFailureKind::Failed);
        assert_eq!(
            response,
            format!(
                "{FAILED_RESPONSE_PREFIX}Image too small (area=64px), minimum required area: >256px（invalid_parameter）"
            ),
            "截断的载荷也要给出一句人话，而不是原始 JSON"
        );
        assert!(!response.contains("\"event\":\"run_failed\""));
        assert_eq!(report.code, "invalid_parameter");
        assert_eq!(report.http_status, Some(400));
    }

    #[test]
    fn terminal_payload_carries_the_structured_failure() {
        let error = provider_failure(
            r#"{"category":"provider_transport","code":"upstream_503","redactedMessage":"上游返回 503","httpStatus":503,"retryable":true}"#,
        );
        let (kind, response, report) = classify_turn_failure(&error);
        let events = failed_turn_terminal_events(FailedTurnTerminalInput {
            action_id: "action-1",
            action_kind: "user_input",
            session_id: "session-1",
            kind,
            response: &response,
            failure: report,
            transport_context: serde_json::json!({ "channel": "desktop" }),
            recorded_at_ms: 1_000,
            warnings: &[],
        })
        .expect("失败终态事件应可构造");

        let completed = events
            .iter()
            .find(|event| event.event_type == TURN_COMPLETED_EVENT)
            .expect("必须发布终态事件");
        let payload: serde_json::Value =
            serde_json::from_str(&completed.payload_json).expect("载荷是 JSON");
        assert_eq!(payload["status"], "failed");
        assert_eq!(payload["failure"]["code"], "upstream_503");
        assert_eq!(payload["failure"]["httpStatus"], 503);
        assert_eq!(payload["failure"]["retryable"], true);
        assert!(
            !payload["response"]
                .as_str()
                .unwrap_or_default()
                .contains("{"),
            "正文里不再出现原始载荷"
        );
    }

    #[test]
    fn terminal_payload_carries_persona_warnings() {
        let error = provider_failure(
            r#"{"category":"provider_transport","code":"upstream_503","redactedMessage":"上游返回 503"}"#,
        );
        let (kind, response, report) = classify_turn_failure(&error);
        let warnings = vec!["人格稳定段接近上限（5000 B）".to_owned()];
        let events = failed_turn_terminal_events(FailedTurnTerminalInput {
            action_id: "action-1",
            action_kind: "user_input",
            session_id: "session-1",
            kind,
            response: &response,
            failure: report,
            transport_context: serde_json::json!({ "channel": "desktop" }),
            recorded_at_ms: 1_000,
            warnings: &warnings,
        })
        .expect("失败终态事件应可构造");

        // 告警挂在终态 lifecycle wire 事件里：序列化后必须能看到原文。
        let emitted = events
            .iter()
            .map(|event| event.payload_json.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            emitted.contains("人格稳定段接近上限（5000 B）"),
            "失败终态事件必须携带人格告警：{emitted}"
        );
    }

    #[test]
    fn terminal_payload_carries_channel_attempts_and_provider_attribution() {
        // 真机样本：中继回 response.failed 且不给 code，渠道链已尝试 2 次。
        let error = provider_failure(
            r#"{"category":"provider_transport","code":"runtime_failed","redactedMessage":"provider response failed：code=upstream_stream_aborted；Unable to forward upstream stream","httpStatus":200,"retryable":true,"replaySafe":false,"attemptCount":2,"providerEventType":"response.failed","upstreamRequestId":"req-9"}"#,
        );
        let (kind, _, report) = classify_turn_failure(&error);
        assert_eq!(kind, TurnFailureKind::Failed);
        assert_eq!(report.attempt_count, Some(2));
        assert_eq!(
            report.provider_event_type.as_deref(),
            Some("response.failed")
        );
        assert_eq!(report.upstream_request_id.as_deref(), Some("req-9"));

        let events = failed_turn_terminal_events(FailedTurnTerminalInput {
            action_id: "action-1",
            action_kind: "user_input",
            session_id: "session-1",
            kind,
            response: "占位",
            failure: report,
            transport_context: serde_json::json!({ "channel": "desktop" }),
            recorded_at_ms: 1_000,
            warnings: &[],
        })
        .expect("失败终态事件应可构造");
        let payload: serde_json::Value = serde_json::from_str(
            &events
                .iter()
                .find(|event| event.event_type == TURN_COMPLETED_EVENT)
                .expect("必须发布终态事件")
                .payload_json,
        )
        .expect("载荷是 JSON");
        assert_eq!(payload["failure"]["attemptCount"], 2);
        assert_eq!(payload["failure"]["providerEventType"], "response.failed");
        assert_eq!(payload["failure"]["upstreamRequestId"], "req-9");
    }
}
