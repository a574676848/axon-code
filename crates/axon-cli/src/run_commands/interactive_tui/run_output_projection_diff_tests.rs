use axon_protocol::diff::FileChange;
use axon_tui_runtime::AgentTuiRunLine;
use serde_json::json;
use std::path::PathBuf;

use super::{CliRunResult, run_output_renderer::format_tui_run_output};

#[test]
fn format_tui_run_output_does_not_project_tool_result_metadata_as_diff() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": wire_tool_events_with_metadata(&json!({
                "kind": "edit",
                "files": [
                    {
                        "path": "src/lib.rs",
                        "operation": "update",
                        "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@\n-old\n+new"
                    }
                ]
            }))
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let diffs = diff_lines(&output);

    assert!(diffs.is_empty());
    assert!(
        output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::ToolReceipt(receipt)
                if receipt.id == "tool-part:test:wire-patch"
                    && receipt.summary == "patched files"))
    );
}

#[test]
fn format_tui_run_output_projects_explicit_wire_file_change_parts() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": wire_tool_events_with_file_changes(vec![
                wire_file_change("diff:tool-part:test:wire-patch:src/new.rs", "src/new.rs", &json!({
                    "type": "add",
                    "content": "new\n"
                }), 3),
                wire_file_change("diff:tool-part:test:wire-patch:src/lib.rs", "src/lib.rs", &json!({
                    "type": "update",
                    "unified_diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@\n-old\n+new",
                    "move_path": null
                }), 4),
                wire_file_change("diff:tool-part:test:wire-patch:src/old.rs", "src/old.rs", &json!({
                    "type": "delete",
                    "content": "old\n"
                }), 5),
                wire_file_change("diff:tool-part:test:wire-patch:src/name_old.rs", "src/name_old.rs", &json!({
                    "type": "update",
                    "unified_diff": "--- a/src/name_old.rs\n+++ b/src/name_new.rs\n@@\n-old\n+new",
                    "move_path": "src/name_new.rs"
                }), 6)
            ])
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let diffs = diff_lines(&output);

    assert_eq!(diffs.len(), 4);
    assert!(has_add(&diffs, "src/new.rs"));
    assert!(has_update(&diffs, "src/lib.rs"));
    assert!(has_delete(&diffs, "src/old.rs"));
    assert!(has_rename(&diffs, "src/name_old.rs", "src/name_new.rs"));
}

#[test]
fn format_tui_run_output_omits_edit_receipt_when_file_changes_are_projected() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": wire_tool_events_with_file_changes(vec![
                wire_file_change(
                    "diff:tool-part:test:wire-patch:src/new.rs",
                    "src/new.rs",
                    &json!({"type": "add", "content": "new\n"}),
                    3,
                ),
                wire_file_change(
                    "diff:tool-part:test:wire-patch:src/lib.rs",
                    "src/lib.rs",
                    &json!({"type": "update", "unified_diff": "-old\n+new"}),
                    4,
                ),
                wire_file_change(
                    "diff:tool-part:test:wire-patch:src/old.rs",
                    "src/old.rs",
                    &json!({"type": "delete", "content": "old\n"}),
                    5,
                ),
            ])
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let diffs = diff_lines(&output);

    assert_eq!(diffs.len(), 3);
    assert!(has_add(&diffs, "src/new.rs"));
    assert!(has_update(&diffs, "src/lib.rs"));
    assert!(has_delete(&diffs, "src/old.rs"));
    assert!(!output.lines().iter().any(|line| {
        matches!(line, AgentTuiRunLine::ToolReceipt(receipt)
            if receipt.kind == axon_tui_runtime::AgentTuiToolKind::Edit)
    }));
}

#[test]
fn format_tui_run_output_omits_mcp_receipt_when_file_changes_are_projected() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": [
                wire_event(
                    &json!({
                        "type": "toolCall",
                        "payload": {
                            "partId": "tool-part:test:morph-edit",
                            "toolCallId": "morph-edit-1",
                            "toolName": "mcp__morph_mcp__edit_file",
                            "toolKind": "mcp",
                            "summary": "Writed src/lib.rs"
                        }
                    }),
                    1,
                ),
                wire_event(
                    &json!({
                        "type": "toolResult",
                        "payload": {
                            "partId": "tool-part:test:morph-edit",
                            "toolCallId": "morph-edit-1",
                            "toolName": "mcp__morph_mcp__edit_file",
                            "toolKind": "mcp",
                            "status": "completed",
                            "summary": "Writed src/lib.rs",
                            "filesystemWritePerformed": true
                        }
                    }),
                    2,
                ),
                wire_event(
                    &json!({
                        "type": "fileChange",
                        "payload": {
                            "partId": "diff:tool-part:test:morph-edit:src/lib.rs",
                            "toolPartId": "tool-part:test:morph-edit",
                            "toolCallId": "morph-edit-1",
                            "path": "src/lib.rs",
                            "change": {
                                "type": "update",
                                "unified_diff": "-old\n+new"
                            }
                        }
                    }),
                    3,
                ),
                wire_event(
                    &json!({
                        "type": "toolCall",
                        "payload": {
                            "partId": "tool-part:test:unrelated-edit",
                            "toolCallId": "unrelated-edit-1",
                            "toolName": "builtin.edit",
                            "toolKind": "edit",
                            "summary": "未生成文件变更"
                        }
                    }),
                    4,
                ),
                wire_event(
                    &json!({
                        "type": "toolResult",
                        "payload": {
                            "partId": "tool-part:test:unrelated-edit",
                            "toolCallId": "unrelated-edit-1",
                            "toolName": "builtin.edit",
                            "toolKind": "edit",
                            "status": "completed",
                            "summary": "未生成文件变更",
                            "filesystemWritePerformed": false
                        }
                    }),
                    5,
                )
            ]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);

    assert_eq!(diff_lines(&output).len(), 1);
    assert!(!output.lines().iter().any(|line| {
        matches!(
            line,
            AgentTuiRunLine::ToolReceipt(receipt)
                if receipt.id == "tool-part:test:morph-edit"
        )
    }));
    assert!(output.lines().iter().any(|line| {
        matches!(
            line,
            AgentTuiRunLine::ToolReceipt(receipt)
                if receipt.id == "tool-part:test:unrelated-edit"
        )
    }));
}

#[test]
fn format_tui_run_output_coalesces_wire_snapshots_for_same_file() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": wire_tool_events_with_file_changes(vec![
                wire_file_change(
                    "diff:tool-part:test:wire-patch:src/error.rs",
                    "src/error.rs",
                    &json!({
                        "type": "add",
                        "content": "line 1\nline 2\n"
                    }),
                    4,
                ),
                wire_file_change(
                    "diff:tool-part:test:wire-patch:src/error.rs",
                    "src/error.rs",
                    &json!({
                        "type": "add",
                        "content": "line 1\n"
                    }),
                    3,
                ),
            ])
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    let diffs = diff_lines(&output);

    assert_eq!(diffs.len(), 1);
    assert!(matches!(
        diffs[0].changes.get(&PathBuf::from("src/error.rs")),
        Some(axon_protocol::diff::FileChange::Add { content })
            if content == "line 1\nline 2\n"
    ));
}

#[test]
fn format_tui_run_output_does_not_project_edit_cell_when_writed_diff_is_present() {
    let result = CliRunResult {
        stdout: json!({
            "wireEvents": [
                wire_file_change(
                    "diff:tool-part:test:update:src/lib.rs",
                    "src/lib.rs",
                    &json!({
                        "type": "update",
                        "unified_diff": "-old\n+new"
                    }),
                    1,
                ),
                wire_event(
                    &json!({
                        "type": "toolResult",
                        "payload": {
                            "partId": "tool-part:test:other-part-id",
                            "toolCallId": "call-1",
                            "toolName": "builtin.edit",
                            "toolKind": "edit",
                            "status": "completed",
                            "summary": "Edit completed",
                            "filesystemWritePerformed": true
                        }
                    }),
                    2,
                ),
            ]
        })
        .to_string(),
        stderr: String::new(),
        exit_code: 0,
    };

    let output = format_tui_run_output(&result);
    assert_eq!(diff_lines(&output).len(), 1);
    assert!(
        !output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::ToolReceipt(_)))
    );
    assert!(
        !output
            .lines()
            .iter()
            .any(|line| matches!(line, AgentTuiRunLine::ToolLifecycle(_)))
    );
}

fn diff_lines(
    output: &axon_tui_runtime::AgentTuiRunOutput,
) -> Vec<&axon_tui_runtime::AgentTuiDiffLine> {
    output
        .lines()
        .iter()
        .filter_map(|line| match line {
            AgentTuiRunLine::Diff(diff) => Some(diff),
            _ => None,
        })
        .collect()
}

fn has_add(diffs: &[&axon_tui_runtime::AgentTuiDiffLine], path: &str) -> bool {
    diffs.iter().any(|diff| {
        matches!(
            diff.changes.get(&PathBuf::from(path)),
            Some(FileChange::Add { .. })
        )
    })
}

fn has_delete(diffs: &[&axon_tui_runtime::AgentTuiDiffLine], path: &str) -> bool {
    diffs.iter().any(|diff| {
        matches!(
            diff.changes.get(&PathBuf::from(path)),
            Some(FileChange::Delete { .. })
        )
    })
}

fn has_update(diffs: &[&axon_tui_runtime::AgentTuiDiffLine], path: &str) -> bool {
    diffs.iter().any(|diff| {
        matches!(
            diff.changes.get(&PathBuf::from(path)),
            Some(FileChange::Update {
                move_path: None,
                ..
            })
        )
    })
}

fn has_rename(diffs: &[&axon_tui_runtime::AgentTuiDiffLine], source: &str, target: &str) -> bool {
    diffs.iter().any(|diff| {
        matches!(
            diff.changes.get(&PathBuf::from(source)),
            Some(FileChange::Update {
                move_path: Some(path),
                ..
            }) if path == &PathBuf::from(target)
        )
    })
}

fn wire_event(kind: &serde_json::Value, sequence: u64) -> serde_json::Value {
    json!({
        "schemaVersion": "axon.wire.v3",
        "eventId": format!("patch-event-{sequence}"),
        "correlationId": "patch-1",
        "session": { "sessionId": "run-1" },
        "sequence": sequence,
        "recordedAtMs": sequence,
        "kind": kind
    })
}

fn wire_tool_events_with_file_changes(
    file_changes: Vec<serde_json::Value>,
) -> Vec<serde_json::Value> {
    let mut events = vec![
        wire_event(
            &json!({
                "type": "toolCall",
                "payload": {
                    "partId": "tool-part:test:wire-patch",
                    "toolCallId": "patch-1",
                    "toolName": "builtin.edit",
                    "toolKind": "edit",
                    "summary": "patched files"
                }
            }),
            1,
        ),
        wire_event(
            &json!({
                "type": "toolResult",
                "payload": {
                    "partId": "tool-part:test:wire-patch",
                    "toolCallId": "patch-1",
                    "status": "completed",
                    "summary": "patched files",
                    "filesystemWritePerformed": true
                }
            }),
            2,
        ),
    ];
    events.extend(file_changes);
    events
}

fn wire_tool_events_with_metadata(metadata: &serde_json::Value) -> Vec<serde_json::Value> {
    vec![
        wire_event(
            &json!({
                "type": "toolCall",
                "payload": {
                    "partId": "tool-part:test:wire-patch",
                    "toolCallId": "patch-1",
                    "toolName": "builtin.edit",
                    "toolKind": "edit",
                    "summary": "patched files"
                }
            }),
            1,
        ),
        wire_event(
            &json!({
                "type": "toolResult",
                "payload": {
                    "partId": "tool-part:test:wire-patch",
                    "toolCallId": "patch-1",
                    "status": "completed",
                    "summary": "patched files",
                    "metadata": metadata,
                    "filesystemWritePerformed": true
                }
            }),
            2,
        ),
    ]
}

fn wire_file_change(
    part_id: &str,
    path: &str,
    change: &serde_json::Value,
    sequence: u64,
) -> serde_json::Value {
    wire_event(
        &json!({
            "type": "fileChange",
            "payload": {
                "partId": part_id,
                "toolPartId": "tool-part:test:wire-patch",
                "toolCallId": "patch-1",
                "path": path,
                "change": change
            }
        }),
        sequence,
    )
}
