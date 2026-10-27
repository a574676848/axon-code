use super::{build_provider_turn_result, compact_structured_failure, run_agent_identity};
use crate::run_commands::RunAgentIdentity;
use axon_protocol::wire::WireEventKind;
use axon_session_runtime::{JarvisRuntimeProfile, ProviderTurnRequest};
use serde_json::{Value, json};
use std::path::Path;

/// `jarvis_executions.kind` 默认值：后台 Goal Execution 的持久 Action 类型。
const GOAL_EXECUTION_ACTION_KIND: &str = "turn";

/// 真机样本：中继回 response.failed 不给 code，渠道链已尝试 2 次。
///
/// 结构化载荷重建后必须完整可解析（不受 600 字符全局截断影响），且新增的
/// 渠道归因字段不能在 CLI 到 Daemon 的接力中丢失。
#[test]
fn structured_failure_relay_keeps_provider_attribution_fields() {
    let detail = "Provider turn 执行失败，退出码 1：".to_owned()
        + r#"{"event":"run_failed","status":"failed","sessionId":"s-1","provider":"codex-onetoken","model":"gpt-6-astra","httpStatus":200,"content":null,"failure":{"category":"provider_transport","code":"runtime_failed","redactedMessage":"provider response failed：code=upstream_stream_aborted；Unable to forward upstream stream","httpStatus":200,"retryable":true,"replaySafe":false,"attemptCount":2,"providerEventType":"response.failed","upstreamRequestId":"req-9"},"providerError":null,"channelAttempts":[],"warnings":[]}"#;

    let compact = compact_structured_failure(&detail).expect("结构化载荷应重建为紧凑 JSON");
    // Daemon 侧同样按第一个 `{` 切分后解析：前缀是人话，载荷必须完整可解析。
    let payload = &compact[compact.find('{').expect("应包含 JSON 载荷")..];
    let value: serde_json::Value = serde_json::from_str(payload).expect("紧凑载荷必须完整可解析");
    assert_eq!(value["failure"]["attemptCount"], 2);
    assert_eq!(value["failure"]["providerEventType"], "response.failed");
    assert_eq!(value["failure"]["upstreamRequestId"], "req-9");
    assert_eq!(value["provider"], "codex-onetoken");
    assert_eq!(value["model"], "gpt-6-astra");
}

fn provider_turn_request(action_kind: &str) -> ProviderTurnRequest {
    ProviderTurnRequest {
        session_id: "session-identity".to_owned(),
        action_kind: action_kind.to_owned(),
        prompt: "执行已承诺的 Goal".to_owned(),
        quoted_message: None,
        cwd: ".".to_owned(),
        run_session_id: "run-identity".to_owned(),
        runtime_profile: JarvisRuntimeProfile::Jarvis,
        transport_context: None,
    }
}

fn provider_turn_request_with_audience(
    action_kind: &str,
    audience: serde_json::Value,
) -> ProviderTurnRequest {
    let mut request = provider_turn_request(action_kind);
    request.transport_context = Some(json!({ "audience": audience }));
    request
}

#[test]
fn provider_turn_identity_follows_action_kind() {
    assert_eq!(
        run_agent_identity(&provider_turn_request(
            axon_daemon::PRIMARY_TURN_ACTION_KIND
        )),
        RunAgentIdentity::PrimaryPrivate
    );
    assert_eq!(
        run_agent_identity(&provider_turn_request(GOAL_EXECUTION_ACTION_KIND)),
        RunAgentIdentity::GoalExecutionSubagent
    );
}

#[test]
fn provider_turn_identity_follows_audience_boundary() {
    assert_eq!(
        run_agent_identity(&provider_turn_request_with_audience(
            axon_daemon::PRIMARY_TURN_ACTION_KIND,
            json!({"kind": "private", "disclosure": "private_only"}),
        )),
        RunAgentIdentity::PrimaryPrivate
    );
    assert_eq!(
        run_agent_identity(&provider_turn_request_with_audience(
            axon_daemon::PRIMARY_TURN_ACTION_KIND,
            json!({"kind": "group", "disclosure": "workspace"}),
        )),
        RunAgentIdentity::PrimaryWorkspace
    );
    assert_eq!(
        run_agent_identity(&provider_turn_request_with_audience(
            axon_daemon::PRIMARY_TURN_ACTION_KIND,
            json!({"kind": "unknown"}),
        )),
        RunAgentIdentity::PrimaryWorkspace
    );
}

fn tool_result_event(
    event_id: &str,
    tool_name: &str,
    status: &str,
    metadata: Option<Value>,
) -> Value {
    json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": event_id,
        "correlationId": "turn-test",
        "session": { "sessionId": "session-test" },
        "sequence": 1,
        "recordedAtMs": 10,
        "kind": {
            "type": "toolResult",
            "payload": {
                "partId": format!("tool-part:{event_id}"),
                "toolCallId": format!("tool-call:{event_id}"),
                "toolName": tool_name,
                "toolKind": "builtin",
                "status": status,
                "summary": "结构化决定",
                "metadata": metadata,
                "filesystemWritePerformed": false
            }
        }
    })
}

fn tool_call_event(event_id: &str) -> Value {
    json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": event_id,
        "correlationId": "turn-test",
        "session": { "sessionId": "session-test" },
        "sequence": 1,
        "recordedAtMs": 10,
        "kind": {
            "type": "toolCall",
            "payload": {
                "partId": format!("tool-part:{event_id}"),
                "toolCallId": format!("tool-call:{event_id}"),
                "toolName": "builtin.read",
                "toolKind": "builtin",
                "summary": "读取文件"
            }
        }
    })
}

#[test]
fn authoritative_final_snapshot_keeps_call_and_result_facts() {
    let payload = json!({
        "content": "完成",
        "wireEvents": [
            tool_call_event("call"),
            tool_result_event("result", "builtin.read", "completed", None)
        ]
    });
    let run_result = crate::CliRunResult {
        exit_code: 0,
        stdout: payload.to_string(),
        stderr: String::new(),
    };

    let result = build_provider_turn_result(
        Path::new("does-not-exist.toml"),
        "run-session-authoritative",
        &run_result,
    )
    .expect("收口投影必须成功");

    let kinds = result
        .wire_events
        .iter()
        .map(|event| match event.kind {
            WireEventKind::ToolCall(_) => "call",
            WireEventKind::ToolResult(_) => "result",
            _ => "other",
        })
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["call", "result"]);
}
