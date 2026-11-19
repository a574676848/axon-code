use super::{
    background_task_path, build_provider_turn_result, compact_structured_failure,
    provider_run_command, run_agent_identity,
};
use crate::run_commands::RunAgentIdentity;
use axon_protocol::wire::WireEventKind;
use axon_session_runtime::{ProviderTurnRequest, RuntimeProfile};
use serde_json::{Value, json};
use std::path::Path;

/// `work_items.kind` 默认值：后台 Goal Execution 的持久 Action 类型。
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
        runtime_profile: RuntimeProfile::Primary,
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

fn provider_turn_request_with_model(model: serde_json::Value) -> ProviderTurnRequest {
    let mut request = provider_turn_request(axon_daemon::PRIMARY_TURN_ACTION_KIND);
    request.transport_context = Some(json!({
        "channel": "tui",
        "destination": "terminal-test",
        "model": model,
    }));
    request
}

#[test]
fn explicit_turn_model_is_the_highest_priority_model_source() {
    let request = provider_turn_request_with_model(json!("balanced"));

    let command =
        provider_run_command(Path::new("missing-config.toml"), &request).expect("命令应可构造");

    assert_eq!(command.model.as_deref(), Some("balanced"));
    assert_eq!(command.session.as_deref(), Some("session-identity"));
    assert_eq!(command.run_session_id.as_deref(), Some("run-identity"));
}

/// 强度与模型同一条通道：随本轮下发的值优先于配置里的固定值。
#[test]
fn explicit_turn_reasoning_effort_is_taken_from_the_transport_context() {
    let mut request = provider_turn_request(axon_daemon::PRIMARY_TURN_ACTION_KIND);
    request.transport_context = Some(json!({
        "channel": "tui",
        "destination": "terminal-test",
        "reasoningEffort": "high",
    }));

    let command =
        provider_run_command(Path::new("missing-config.toml"), &request).expect("命令应可构造");

    assert_eq!(command.reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn absent_or_blank_explicit_reasoning_effort_leaves_effort_to_config() {
    for context in [
        json!({"channel": "tui", "destination": "terminal-test"}),
        json!({"channel": "tui", "destination": "terminal-test", "reasoningEffort": "   "}),
        json!({"channel": "tui", "destination": "terminal-test", "reasoningEffort": null}),
    ] {
        let mut request = provider_turn_request(axon_daemon::PRIMARY_TURN_ACTION_KIND);
        request.transport_context = Some(context);

        let command =
            provider_run_command(Path::new("missing-config.toml"), &request).expect("命令应可构造");

        assert_eq!(command.reasoning_effort, None);
    }
}

#[test]
fn absent_or_blank_explicit_model_leaves_model_to_session_and_config() {
    for context in [
        json!({"channel": "tui", "destination": "terminal-test"}),
        json!({"channel": "tui", "destination": "terminal-test", "model": "   "}),
        json!({"channel": "tui", "destination": "terminal-test", "model": null}),
    ] {
        let mut request = provider_turn_request(axon_daemon::PRIMARY_TURN_ACTION_KIND);
        request.transport_context = Some(context);

        let command =
            provider_run_command(Path::new("missing-config.toml"), &request).expect("命令应可构造");

        assert_eq!(command.model, None);
    }
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

/// 会话标识是 Crockford Base32 的大写 ULID；任务路径的合法段只接受小写。
#[test]
fn background_task_path_lowercases_run_state_ids() {
    let task_path = background_task_path("01M7RQ9NYW8S6SQ8GTNM730M96")
        .expect("大写会话标识必须归一成合法任务路径");
    assert_eq!(
        task_path.as_str().strip_prefix("/root/task-"),
        Some("01m7rq9nyw8s6sq8gtnm730m96")
    );
}

/// 该路径段是子会话身份的一半：非法字符必须整轮报错，不能折叠成 `-` 去撞同一条子会话。
#[test]
fn background_task_path_rejects_illegal_run_state_ids() {
    for illegal in ["goal:1/2", "task.1", "会话 1", ""] {
        let error = background_task_path(illegal).expect_err("非法会话标识必须直接报错");
        assert!(
            format!("{error:?}").contains(axon_agent_runtime::AGENT_CONTROL_REASON_INVALID_PATH),
            "失败必须保留 canonical 原因码：{illegal:?} -> {error:?}"
        );
    }
}
