use axon_tui_runtime::{AgentTuiMessagePartRole, AgentTuiRunLine};
use serde_json::json;

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

#[test]
fn format_tui_run_output_projects_json_content_and_usage() {
    let result = CliRunResult {
        stdout: r#"{
            "sessionId": "run-1",
            "content": "完成",
            "wireEvents": []
        }"#
        .replace(
            "\"wireEvents\": []",
            &format!(
                "\"wireEvents\": [{}]",
                [
                    wire_event(
                        &json!({
                            "type": "assistantMessage",
                            "payload": {
                                "partId": "assistant-part:run-1",
                                "messageId": "provider-message:run-1",
                                "content": "完成",
                                "delta": false
                            }
                        }),
                        1,
                    ),
                    wire_event(
                        &json!({
                            "type": "usageUpdate",
                            "payload": {
                                "inputTokens": 10,
                                "cachedInputTokens": 2,
                                "outputTokens": 3,
                                "reasoningOutputTokens": 1,
                                "currentContextTokens": 20
                            }
                        }),
                        2,
                    ),
                ]
                .into_iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
                .join(",")
            ),
        ),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let lines = output.lines();

    assert!(matches!(lines.first(), Some(AgentTuiRunLine::Session(_))));
    assert!(matches!(
        lines.get(1),
        Some(AgentTuiRunLine::Message(message))
            if message.role == AgentTuiMessagePartRole::Assistant
                && message.id == "assistant-part:run-1"
                && message.text == "完成"
    ));
    assert_eq!(output.token_usage_total().total_tokens(), 16);
    assert_eq!(output.token_usage_total().current_context_tokens(), 20);
}

#[test]
fn format_tui_run_output_projects_session_title() {
    let result = CliRunResult {
        stdout: json!({
            "sessionId": "run-1",
            "sessionTitle": "修复终端标题",
            "wireEvents": []
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert!(matches!(
        output.lines().first(),
        Some(AgentTuiRunLine::Session(session))
            if session.session_id == "run-1"
                && session.title.as_deref() == Some("修复终端标题")
    ));
}

#[test]
fn format_tui_run_output_requires_structured_assistant_parts() {
    let result = CliRunResult {
        stdout: json!({
            "content": "无 session 回答",
            "wireEvents": []
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    assert!(!output.lines().iter().any(|line| matches!(
        line,
        AgentTuiRunLine::Message(message)
            if message.role == AgentTuiMessagePartRole::Assistant
    )));
}

#[test]
fn format_tui_run_output_prefers_wire_assistant_without_duplicate_content() {
    let result = CliRunResult {
        stdout: json!({
            "content": "旧顶层回答",
            "wireEvents": [wire_event(
                &json!({
                    "type": "assistantMessage",
                    "payload": {
                        "partId": "assistant:wire-final",
                        "messageId": "provider-assistant-final",
                        "content": "wire 回答",
                        "delta": false
                    }
                }),
                1,
            )]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let assistants = output
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Message(message)
                if message.role == AgentTuiMessagePartRole::Assistant =>
            {
                Some((message.id.as_str(), message.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(assistants, vec![("assistant:wire-final", "wire 回答")]);
}

#[test]
fn format_tui_run_output_projects_snake_case_usage_context_and_cache() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": [wire_event(
                &json!({
                    "type": "usageUpdate",
                    "payload": {
                        "inputTokens": 500,
                        "cachedInputTokens": 9500,
                        "cacheWriteInputTokens": 200,
                        "outputTokens": 200,
                        "reasoningOutputTokens": 10,
                        "currentContextTokens": 10410
                    }
                }),
                1,
            )]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let usage = output.token_usage_total();

    assert_eq!(usage.input_tokens, 500);
    assert_eq!(usage.cache_read_input_tokens, 9_500);
    assert_eq!(usage.cache_write_input_tokens, 200);
    assert_eq!(usage.output_tokens, 200);
    assert_eq!(usage.reasoning_output_tokens, 10);
    assert_eq!(usage.current_context_tokens(), 10_410);
}

#[test]
fn format_tui_run_output_uses_structured_shell_exec_metadata_without_summary_parsing() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": wire_tool_events(
                "call-1",
                "builtin.shell_command",
                "exec",
                "completed",
                "shell command=echo axon duration_ms=65000 exit_code=0 timed_out=false stdout=machine text",
                &json!({
                    "kind": "exec",
                    "toolName": "shell_command",
                    "status": "completed",
                    "summary": "echo axon · exit 0",
                    "argumentsPreview": "echo axon",
                    "outputPreview": "出 axon"
                }),
                false,
            )
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let receipt = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::ToolReceipt(receipt) => Some(receipt),
            _ => None,
        })
        .expect("shell receipt");

    assert_eq!(
        receipt.metadata.as_ref().expect("metadata")["argumentsPreview"],
        "echo axon"
    );
    assert_eq!(
        receipt.summary,
        "shell command=echo axon duration_ms=1m 05s exit_code=0 timed_out=false stdout=machine text"
    );
}

#[test]
fn format_tui_run_output_does_not_infer_shell_exec_metadata_from_summary() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": wire_tool_events(
                "call-1",
                "builtin.shell_command",
                "shell",
                "completed",
                "shell command=echo axon elapsed_ms=1250 exit_code=0 timed_out=false stdout=axon stderr=",
                &serde_json::Value::Null,
                false,
            )
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let receipt = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::ToolReceipt(receipt) => Some(receipt),
            _ => None,
        })
        .expect("shell receipt");

    assert!(receipt.metadata.is_none());
    assert_eq!(
        receipt.summary,
        "shell command=echo axon elapsed_ms=1.25s exit_code=0 timed_out=false stdout=axon stderr="
    );
}

#[test]
fn format_tui_run_output_surfaces_stderr_as_error() {
    let result = CliRunResult {
        stdout: String::new(),
        stderr: "provider failed".to_owned(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);

    assert!(output.lines().iter().any(|line| {
        matches!(line, AgentTuiRunLine::Error(error) if error.message.contains("provider failed"))
    }));
}

#[test]
fn format_tui_run_output_structures_stderr_runtime_reason() {
    let result = CliRunResult {
        stdout: String::new(),
        stderr: "error: rollout 会话 JSON 无效".to_owned(),
        exit_code: 2,
    };

    let output = format_tui_run_output(&result);
    let error = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::Error(error) => Some(error),
            _ => None,
        })
        .expect("stderr 应投影为结构化运行时错误");

    assert_eq!(error.error_type.as_deref(), Some("runtime"));
    assert_eq!(error.error_code.as_deref(), Some("runtime_stderr"));
    assert!(
        error
            .details
            .iter()
            .any(|detail| { detail == "原因 error: rollout 会话 JSON 无效" })
    );
}

#[test]
fn format_tui_run_output_does_not_compensate_provider_status_as_error_cell() {
    let result = CliRunResult {
        stdout: json!({
            "event": "run_failed",
            "sessionId": "run-1",
            "wireEvents": [wire_event(
                &json!({
                    "type": "providerStatus",
                    "payload": {
                        "provider": "anthropic",
                        "model": "claude",
                        "phase": "failed",
                        "transport": "http",
                        "attemptIndex": 1,
                        "errorMessage": "配置不可用"
                    }
                }),
                1,
            )]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);
    assert!(
        !output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::Error(_)))
    );
}

#[test]
fn format_tui_run_output_does_not_project_recovered_provider_failure_as_error() {
    let result = CliRunResult {
        stdout: json!({
            "event": "run_completed",
            "sessionId": "run-1",
            "wireEvents": [
                wire_event(
                    &json!({
                        "type": "providerStatus",
                        "payload": {
                            "provider": "anthropic-a",
                            "model": "claude",
                            "phase": "failed",
                            "transport": "http",
                            "attemptIndex": 1,
                            "errorMessage": "上游暂时不可用"
                        }
                    }),
                    1,
                ),
                wire_event(
                    &json!({
                        "type": "assistantMessage",
                        "payload": {
                            "partId": "assistant:recovered",
                            "messageId": "provider-assistant:recovered",
                            "content": "已通过后续渠道完成",
                            "delta": false
                        }
                    }),
                    2,
                )
            ]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert!(
        !output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::Error(_)))
    );
}

#[test]
fn format_tui_run_output_keeps_failed_tool_as_exec_without_summary_review() {
    let result = CliRunResult {
        stdout: json!({
            "content": "工具失败",
            "wireEvents": wire_tool_events(
                "call-1",
                "shell",
                "shell",
                "failed",
                "shell command=cargo check exit_code=1 timed_out=false output_handler=compact full_output_path=none stdout= stderr=error",
                &serde_json::Value::Null,
                false,
            )
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 1,
    };

    let output = format_tui_run_output(&result);

    assert!(output.lines().iter().any(|line| {
        matches!(line, AgentTuiRunLine::ToolReceipt(receipt)
            if receipt.status == "failed" && receipt.tool_name == "shell")
    }));
    assert!(
        !output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::Review(_)))
    );
}

#[test]
fn format_tui_run_output_does_not_project_stderr_as_error_for_success() {
    let result = CliRunResult {
        stdout: String::new(),
        stderr: "Compiling axon-cli v0.1.0".to_owned(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert!(
        !output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::Error(_)))
    );
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

fn wire_tool_events(
    tool_call_id: &str,
    tool_name: &str,
    tool_kind: &str,
    status: &str,
    summary: &str,
    metadata: &serde_json::Value,
    filesystem_write_performed: bool,
) -> Vec<serde_json::Value> {
    vec![
        wire_event(
            &json!({
                "type": "toolCall",
                "payload": {
                    "partId": "tool-part:test:projection-fixture",
                    "toolCallId": tool_call_id,
                    "toolName": tool_name,
                    "toolKind": tool_kind,
                    "summary": summary
                }
            }),
            1,
        ),
        wire_event(
            &json!({
                "type": "toolResult",
                "payload": {
                    "partId": "tool-part:test:projection-fixture",
                    "toolCallId": tool_call_id,
                    "status": status,
                    "summary": summary,
                    "metadata": metadata,
                    "filesystemWritePerformed": filesystem_write_performed
                }
            }),
            2,
        ),
    ]
}
