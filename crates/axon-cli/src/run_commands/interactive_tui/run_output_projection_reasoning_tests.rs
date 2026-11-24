use axon_tui_runtime::{AgentTuiMessagePartRole, AgentTuiRunLine};

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

#[test]
fn format_tui_run_output_projects_wire_reasoning_before_assistant_part() {
    let result = CliRunResult {
        stdout: r#"{
            "content": "旧顶层回答",
            "wireEvents": [{
                "schemaVersion": "1.0",
                "eventId": "evt-1",
                "correlationId": "provider-reasoning:1",
                "session": { "sessionId": "s1" },
                "sequence": 1,
                "recordedAtMs": 1,
                "kind": {
                    "type": "reasoningStart",
                    "payload": {
                        "partId": "reasoning:part-1",
                        "reasoningId": "provider-reasoning:1"
                    }
                }
            }, {
                "schemaVersion": "1.0",
                "eventId": "evt-2",
                "correlationId": "provider-reasoning:1",
                "session": { "sessionId": "s1" },
                "sequence": 2,
                "recordedAtMs": 1,
                "kind": {
                    "type": "reasoningDelta",
                    "payload": {
                        "partId": "reasoning:part-1",
                        "reasoningId": "provider-reasoning:1",
                        "delta": "先分析"
                    }
                }
            }, {
                "schemaVersion": "1.0",
                "eventId": "evt-3",
                "correlationId": "provider-reasoning:1",
                "session": { "sessionId": "s1" },
                "sequence": 3,
                "recordedAtMs": 1,
                "kind": {
                    "type": "reasoningDelta",
                    "payload": {
                        "partId": "reasoning:part-1",
                        "reasoningId": "provider-reasoning:1",
                        "delta": "再回答"
                    }
                }
            }, {
                "schemaVersion": "1.0",
                "eventId": "evt-4",
                "correlationId": "provider-reasoning:1",
                "session": { "sessionId": "s1" },
                "sequence": 4,
                "recordedAtMs": 1,
                "kind": {
                    "type": "reasoningEnd",
                    "payload": {
                        "partId": "reasoning:part-1",
                        "reasoningId": "provider-reasoning:1"
                    }
                }
            }, {
                "schemaVersion": "1.0",
                "eventId": "evt-5",
                "correlationId": "assistant",
                "session": { "sessionId": "s1" },
                "sequence": 5,
                "recordedAtMs": 1,
                "kind": {
                    "type": "assistantMessage",
                    "payload": {
                        "partId": "assistant:part-final",
                        "messageId": "provider-assistant-final",
                        "content": "最终回答",
                        "delta": false
                    }
                }
            }]
        }"#
        .to_owned(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let lines = output.lines();

    assert!(matches!(
        lines.first(),
        Some(AgentTuiRunLine::Reasoning(reasoning))
            if reasoning.id == "reasoning:part-1" && reasoning.text == "先分析再回答"
    ));
    assert!(matches!(
        lines.get(1),
        Some(AgentTuiRunLine::Message(message))
            if message.role == AgentTuiMessagePartRole::Assistant
                && message.id == "assistant:part-final"
                && message.text == "最终回答"
    ));
}

#[test]
fn format_tui_run_output_preserves_prior_turn_when_retry_has_no_visible_output() {
    let events = vec![
        streaming_started_event(1, 1),
        assistant_delta_event(2, "assistant-part:1", "第一轮成功"),
        reasoning_delta_event(3, "reasoning-part:1", "第一轮分析"),
        reasoning_end_event(4, "reasoning-part:1"),
        streaming_started_event(5, 1),
        streaming_started_event(6, 2),
        provider_retry_event(7, 2),
        assistant_delta_event(8, "assistant-part:2", "第二轮成功"),
        reasoning_delta_event(9, "reasoning-part:2", "第二轮分析"),
        reasoning_end_event(10, "reasoning-part:2"),
    ];
    let result = CliRunResult {
        stdout: serde_json::json!({ "wireEvents": events }).to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let assistants = output
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Message(message) => Some(message.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let reasoning = output
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Reasoning(reasoning) => Some(reasoning.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(assistants, vec!["第一轮成功", "第二轮成功"]);
    assert_eq!(reasoning, vec!["第一轮分析", "第二轮分析"]);
}

fn streaming_started_event(sequence: u64, attempt_index: u64) -> serde_json::Value {
    wire_event(
        sequence,
        &serde_json::json!({
            "type": "turnLifecycle",
            "payload": {
                "partId": format!("generation:stream:{attempt_index}"),
                "turnId": "provider-stream",
                "phase": "regularTurn",
                "status": "started",
                "title": "生成",
                "kind": "StreamingStarted",
                "attemptIndex": attempt_index
            }
        }),
    )
}

fn provider_retry_event(sequence: u64, attempt_index: u64) -> serde_json::Value {
    wire_event(
        sequence,
        &serde_json::json!({
            "type": "turnLifecycle",
            "payload": {
                "partId": format!("generation:retry:{attempt_index}"),
                "turnId": "ProviderRetry",
                "phase": "regularTurn",
                "status": "running",
                "title": "运行",
                "kind": "ProviderRetry",
                "attemptIndex": attempt_index
            }
        }),
    )
}

fn assistant_delta_event(sequence: u64, part_id: &str, content: &str) -> serde_json::Value {
    wire_event(
        sequence,
        &serde_json::json!({
            "type": "assistantMessage",
            "payload": {
                "partId": part_id,
                "messageId": part_id,
                "content": content,
                "delta": true
            }
        }),
    )
}

fn reasoning_delta_event(sequence: u64, part_id: &str, delta: &str) -> serde_json::Value {
    wire_event(
        sequence,
        &serde_json::json!({
            "type": "reasoningDelta",
            "payload": {
                "partId": part_id,
                "reasoningId": part_id,
                "delta": delta
            }
        }),
    )
}

fn reasoning_end_event(sequence: u64, part_id: &str) -> serde_json::Value {
    wire_event(
        sequence,
        &serde_json::json!({
            "type": "reasoningEnd",
            "payload": {
                "partId": part_id,
                "reasoningId": part_id
            }
        }),
    )
}

fn wire_event(sequence: u64, kind: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": format!("event-{sequence}"),
        "correlationId": format!("correlation-{sequence}"),
        "session": { "sessionId": "retry-projection-test" },
        "sequence": sequence,
        "recordedAtMs": sequence,
        "kind": kind
    })
}

#[test]
fn format_tui_run_output_preserves_spaces_between_wire_reasoning_deltas() {
    let result = CliRunResult {
        stdout: serde_json::json!({
            "wireEvents": [
                reasoning_delta_event(1, "reasoning:space", "first "),
                reasoning_delta_event(2, "reasoning:space", "second")
            ]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let reasoning = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::Reasoning(reasoning) => Some(reasoning.text.as_str()),
            _ => None,
        })
        .expect("reasoning cell");

    assert_eq!(reasoning, "first second");
}
