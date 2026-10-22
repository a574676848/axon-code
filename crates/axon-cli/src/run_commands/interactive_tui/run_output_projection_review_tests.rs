use axon_tui_runtime::{AgentTuiReviewStatus, AgentTuiRunLine};

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

fn review_events(entries: &[(&str, &str, &str)]) -> Vec<serde_json::Value> {
    entries
        .iter()
        .enumerate()
        .map(|(offset, (part_id, status, subject))| {
            serde_json::json!({
                "schemaVersion": "axon.wire.v3",
                "eventId": format!("review-event-{offset}"),
                "correlationId": "review-1",
                "session": { "sessionId": "session-1", "runId": "run-1" },
                "sequence": offset as u64 + 1,
                "recordedAtMs": offset as i64 + 1,
                "kind": {
                    "type": "reviewStatus",
                    "payload": {
                        "partId": part_id,
                        "subject": subject,
                        "status": status,
                        "details": [format!("{subject}明细")],
                    },
                },
            })
        })
        .collect()
}

fn stdout_with_wire_events(events: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "sessionId": "session-1",
        "wireEvents": events,
        "content": "",
    })
    .to_string()
}

fn review_lines(result: &CliRunResult) -> Vec<axon_tui_runtime::AgentTuiReviewLine> {
    format_tui_run_output(result)
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Review(review) => Some(review.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn format_tui_run_output_projects_wire_review_status_as_review_cell() {
    let result = CliRunResult {
        stdout: stdout_with_wire_events(review_events(&[("review:part:1", "blocked", "依赖检查")])),
        stderr: String::new(),
        exit_code: 0,
    };

    let reviews = review_lines(&result);

    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].id, "review:part:1");
    assert_eq!(reviews[0].status, AgentTuiReviewStatus::Blocked);
    assert_eq!(reviews[0].subject, "依赖检查");
    assert_eq!(reviews[0].details, ["依赖检查明细"]);
}

#[test]
fn format_tui_run_output_coalesces_review_snapshots_by_part_id() {
    let result = CliRunResult {
        stdout: stdout_with_wire_events(review_events(&[
            ("review:part:1", "running", "依赖检查"),
            ("review:part:1", "completed", "依赖检查"),
        ])),
        stderr: String::new(),
        exit_code: 0,
    };

    let reviews = review_lines(&result);

    assert_eq!(
        reviews.len(),
        1,
        "同一 review part 的多帧快照必须收敛为一个 cell"
    );
    assert_eq!(reviews[0].status, AgentTuiReviewStatus::Completed);
}

#[test]
fn format_tui_run_output_keeps_review_position_in_durable_order() {
    let mut events = review_events(&[("review:part:1", "completed", "依赖检查")]);
    events.push(serde_json::json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": "assistant-event-final",
        "correlationId": "review-1",
        "session": { "sessionId": "session-1", "runId": "run-1" },
        "sequence": 9,
        "recordedAtMs": 9,
        "kind": {
            "type": "assistantMessage",
            "payload": {
                "partId": "assistant-part:1",
                "messageId": "assistant-message-final",
                "content": "最终回答",
                "delta": false,
            },
        },
    }));
    let result = CliRunResult {
        stdout: stdout_with_wire_events(events),
        stderr: String::new(),
        exit_code: 0,
    };

    let kinds = format_tui_run_output(&result)
        .lines()
        .iter()
        .map(|line| match line {
            AgentTuiRunLine::Review(_) => "review",
            AgentTuiRunLine::Message(_) => "assistant",
            _ => "other",
        })
        .filter(|kind| *kind != "other")
        .collect::<Vec<_>>();

    assert_eq!(
        kinds,
        ["review", "assistant"],
        "review 终态行必须落在其 durable sequence 位置，而不是固定追加在末尾"
    );
}
