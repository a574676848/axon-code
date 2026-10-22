use axon_tui_runtime::AgentTuiRunLine;
use serde_json::json;

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

#[test]
fn format_tui_run_output_projects_memory_recall_top_three_as_history_cell() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": [memory_recall_wire_event(&json!({
                "status": "hybrid",
                "hitCount": 4,
                "trace": [{
                    "contentRef": "memory://same-repo/first-important-decision",
                    "scoreBasisPoints": 9300,
                    "scopeMatch": "same_repo",
                    "promptInjected": true
                }, {
                    "contentRef": "memory://workspace/second-useful-preference",
                    "scoreBasisPoints": 8700,
                    "scopeMatch": "same_workspace",
                    "promptInjected": true
                }, {
                    "contentRef": "memory://workspace/third-review-rule",
                    "scoreBasisPoints": 8100,
                    "scopeMatch": "same_workspace",
                    "promptInjected": true
                }, {
                    "contentRef": "memory://workspace/fourth-hidden-by-preview-limit",
                    "scoreBasisPoints": 7600,
                    "scopeMatch": "same_workspace",
                    "promptInjected": true
                }]
            }))]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert!(output.lines().iter().any(|line| {
        matches!(line, AgentTuiRunLine::History(history)
            if history.title == "记忆"
                && history.items.len() == 3
                && history.items[0].name == "memory://same-repo/first-important-decision"
                && history.items[0].kind_label.as_deref() == Some("记忆")
                && history.items[0].status_label.as_deref() == Some("93% · same_repo")
                && history.items[2].status_label.as_deref() == Some("81% · same_workspace"))
    }));
}

#[test]
fn format_tui_run_output_drops_empty_or_skipped_memory_recall_without_status_noise() {
    for metadata in [
        json!({"status":"skipped","hitCount":0,"trace":[]}),
        json!({"status":"hybrid","hitCount":0,"trace":[]}),
        json!({"status":"hybrid","hitCount":1,"trace":[{"contentRef":"memory://not-used","scoreBasisPoints":9000,"scopeMatch":"same_repo","promptInjected":false}]}),
    ] {
        let result = CliRunResult {
            stdout: json!({
                "wireEvents": [memory_recall_wire_event(&metadata)]
            })
            .to_string(),
            stderr: String::new(),
            exit_code: 0,
        };

        let output = format_tui_run_output(&result);

        assert!(
            output.lines().iter().all(|line| !matches!(
                line,
                AgentTuiRunLine::History(history) if history.title == "记忆"
            )),
            "{metadata} 不应生成记忆历史行",
        );
        assert!(
            output
                .lines()
                .iter()
                .all(|line| !matches!(line, AgentTuiRunLine::Status(_))),
            "{metadata} 不应生成状态噪声",
        );
    }
}

#[test]
fn format_tui_run_output_omits_provider_visible_guidance_memory_refs() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": [memory_recall_wire_event(&json!({
                "status": "hybrid",
                "hitCount": 3,
                "trace": [{
                    "contentRef": "memory-ref:tool-guidance:use builtin.glob",
                    "scoreBasisPoints": 9300,
                    "scopeMatch": "same_repo",
                    "promptInjected": true
                }, {
                    "contentRef": "memory_tool_guidance=memory-guidance: run fmt",
                    "scoreBasisPoints": 9000,
                    "scopeMatch": "same_repo",
                    "promptInjected": true
                }, {
                    "contentRef": "memory://workspace/visible-preference",
                    "scoreBasisPoints": 8700,
                    "scopeMatch": "same_workspace",
                    "promptInjected": true
                }]
            }))]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let history = output
        .lines()
        .iter()
        .find_map(|line| match line {
            AgentTuiRunLine::History(history) if history.title == "记忆" => Some(history),
            _ => None,
        })
        .expect("应保留用户可见记忆");

    assert_eq!(history.items.len(), 1);
    assert_eq!(
        history.items[0].name,
        "memory://workspace/visible-preference"
    );
}

fn memory_recall_wire_event(metadata: &serde_json::Value) -> serde_json::Value {
    json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": "event-memory-recall",
        "correlationId": "session-1",
        "session": {
            "sessionId": "session-1"
        },
        "sequence": 1,
        "recordedAtMs": 42,
        "kind": {
            "type": "memoryStatus",
            "payload": {
                "subject": "recall",
                "status": "completed",
                "outcome": metadata["status"].as_str().unwrap_or("unknown"),
                "summary": "Memory recall",
                "refs": [],
                "metadata": metadata
            }
        }
    })
}
