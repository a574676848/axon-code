//! Daemon 回合失败报告的终态投影测试。
//!
//! 失败报告必须与成功报告共用同一条 durable 重放路径：TUI 实时邮箱只淘汰可重建的
//! 展示增量，被淘汰的增量只能由终态按 durable 顺序完整重放补齐。

use axon_tui_runtime::AgentTuiRunLine;
use serde_json::json;

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

/// F-6 回归：失败报告必须与成功报告共用同一条 durable 重放路径。
///
/// TUI 实时邮箱只淘汰可重建的展示增量；被淘汰的中段只能由终态按 durable 顺序
/// 完整重放补齐。失败报告若丢弃 `wireEvents`，正文会永久留下缺口。
#[test]
fn format_tui_run_output_replays_deltas_from_daemon_run_failed_report() {
    let result = CliRunResult {
        stdout: json!({
            "event": "run_failed",
            "error": "Daemon 事件订阅重试耗尽",
            "wireEvents": [
                wire_event(
                    &json!({
                        "type": "assistantMessage",
                        "payload": {
                            "partId": "assistant-part:1",
                            "messageId": "assistant-message-final",
                            "content": "开头",
                            "delta": true
                        }
                    }),
                    1,
                ),
                wire_event(
                    &json!({
                        "type": "assistantMessage",
                        "payload": {
                            "partId": "assistant-part:1",
                            "messageId": "assistant-message-final",
                            "content": "中段尾部",
                            "delta": true
                        }
                    }),
                    2,
                )
            ]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);
    let assistant = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::Message(message) => Some(message.clone()),
            _ => None,
        })
        .expect("失败报告携带的 durable 重放必须投影为 assistant 正文");

    assert_eq!(assistant.text, "开头中段尾部");
    let errors = output
        .lines()
        .iter()
        .filter(|line| matches!(line, AgentTuiRunLine::Error(_)))
        .count();
    assert_eq!(errors, 1, "重放之后只应收口一条结构化错误行");
}

#[test]
fn format_tui_run_output_surfaces_daemon_run_failed_report() {
    let result = CliRunResult {
        stdout: json!({
            "event": "run_failed",
            "error": "连接 Daemon IPC 失败：系统找不到指定的文件。 (os error 2)",
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);
    let error = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::Error(error) => Some(error),
            _ => None,
        })
        .expect("Daemon run_failed 报告应投影为结构化运行时错误");

    assert_eq!(error.error_type.as_deref(), Some("runtime"));
    assert_eq!(error.error_code.as_deref(), Some("daemon_run_failed"));
    assert!(error.message.contains("连接 Daemon IPC 失败"));
    assert_eq!(
        error.details,
        vec!["原因 连接 Daemon IPC 失败：系统找不到指定的文件。 (os error 2)"]
    );
    assert!(
        error
            .next_step
            .as_deref()
            .is_some_and(|next_step| next_step.contains("axon daemon status"))
    );
}

#[test]
fn format_tui_run_output_prefers_wire_runtime_error_over_top_level_duplicate() {
    let result = CliRunResult {
        stdout: json!({
            "event": "run_failed",
            "sessionId": "run-1",
            "failure": {
                "category": "runtime",
                "code": "runtime_failed",
                "redactedMessage": "顶层重复错误",
                "visibleOutputEmitted": false,
                "retryable": false,
                "replaySafe": true,
                "attemptCount": 1,
                "partialOutputPresent": false,
                "timestamp": 6
            },
            "wireEvents": [wire_event(
                &json!({
                    "type": "runtimeError",
                    "payload": {
                        "category": "rollout_store",
                        "code": "rollout_json_invalid",
                        "redactedMessage": "rollout 会话 JSON 无效",
                        "visibleOutputEmitted": false,
                        "retryable": false,
                        "replaySafe": true,
                        "attemptCount": 1,
                        "partialOutputPresent": false,
                        "timestamp": 7
                    }
                }),
                7,
            )]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);
    let errors = output
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Error(error) => Some(error),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].error_type.as_deref(), Some("rollout_store"));
    assert_eq!(
        errors[0].error_code.as_deref(),
        Some("rollout_json_invalid")
    );
    assert_eq!(
        errors[0].caused_by_runtime_event.as_deref(),
        Some("event-7")
    );
    assert_eq!(errors[0].message, "rollout 会话 JSON 无效");
}

#[test]
fn format_tui_run_output_projects_channel_attempts_to_error() {
    let result = CliRunResult {
        stdout: json!({
            "event": "run_failed",
            "sessionId": "run-1",
            "failure": {
                "category": "provider_transport",
                "code": "provider_transport_failed",
                "redactedMessage": "全部 2 个渠道均失败，最后错误：bad gateway",
                "httpStatus": 502,
                "visibleOutputEmitted": false,
                "retryable": true,
                "replaySafe": true,
                "attemptCount": 2,
                "partialOutputPresent": false,
                "timestamp": 8
            },
            "channelAttempts": [
                {
                    "provider": "openai-a",
                    "model": "gpt-5",
                    "httpStatus": 429,
                    "outcome": "transient_failure"
                },
                {
                    "provider": "openai-b",
                    "model": "gpt-5",
                    "httpStatus": 502,
                    "outcome": "transient_failure"
                }
            ]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);
    let error = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::Error(error) => Some(error),
            _ => None,
        })
        .expect("应投影错误行");

    assert_eq!(error.http_status, Some(502));
    assert_eq!(error.channel_attempts.len(), 2);
    assert_eq!(error.channel_attempts[0].provider, "openai-a");
    assert_eq!(error.channel_attempts[0].http_status, Some(429));
    assert_eq!(error.channel_attempts[1].provider, "openai-b");
    assert_eq!(error.channel_attempts[1].http_status, Some(502));
}

fn wire_event(kind: &serde_json::Value, sequence: u64) -> serde_json::Value {
    json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": format!("event-{sequence}"),
        "correlationId": "run-1",
        "session": { "sessionId": "run-1" },
        "sequence": sequence,
        "recordedAtMs": sequence,
        "kind": kind
    })
}
