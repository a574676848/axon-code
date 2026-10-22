//! Exec cell 与 Diff cell 生命周期投射测试。

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::run_output::{
    AgentTuiRunLine, AgentTuiRunOutput, AgentTuiToolCompletionView, AgentTuiToolDiffSnapshotView,
    AgentTuiToolExecutionView, AgentTuiToolKind, AgentTuiToolReceipt, AgentTuiToolReceiptDetails,
};
use crate::r#loop::agent_tui::run_output_diff::AgentTuiDiffLine;
use crate::surface::cell::{RenderCellData, RenderCellKind, RenderCellMode};
use axon_domain::ToolExecutionPermission;
use axon_protocol::{
    diff::FileChange,
    wire::{WireEventKind, WireFileChangeEvent},
};
use std::{collections::BTreeMap, path::PathBuf};

fn tool_receipt(
    provider_call_id: &str,
    tool_name: &str,
    details: AgentTuiToolReceiptDetails,
) -> AgentTuiToolReceipt {
    AgentTuiToolReceipt::with_id(
        "tool-part:test:fixture",
        provider_call_id,
        tool_name,
        details,
    )
}

#[test]
fn edit_begin_snapshot_settles_as_diff_when_completion_has_no_metadata() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "src/lib.rs".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    let mut changes = BTreeMap::new();
    changes.insert(
        PathBuf::from("src/lib.rs"),
        FileChange::Update {
            unified_diff: "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new".to_owned(),
            move_path: None,
        },
    );

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_diff_snapshot_event(
            AgentTuiToolDiffSnapshotView {
                id: "diff:tool:call-1".to_owned(),
                tool_part_id: Some("tool-part:test:fixture".to_owned()),
                provider_call_id: "call-1".to_owned(),
                title: "应用补丁".to_owned(),
                changes,
                text_writes_applied: 0,
                renames_applied: 0,
            },
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 1);

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                status: "completed".to_owned(),
                summary: "ok".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: true,
                duration_ms: Some(42),
            },
        ),
    );

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 1);
    assert!(
        !state
            .cells
            .tool_outputs_by_part_id
            .contains_key("tool-part:test:fixture")
    );
    assert!(
        state
            .cells
            .transcript
            .iter()
            .any(|line| { matches!(line.mode, RenderCellMode::Cell(RenderCellKind::DIFF)) })
    );
}

#[test]
fn noop_write_snapshot_does_not_replace_active_tool_with_diff() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:noop-write".to_owned(),
                request_id: 1,
                provider_call_id: "call-noop-write".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "src/lib.rs".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::FileChange(
            WireFileChangeEvent {
                part_id: "diff:tool-part:test:noop-write:src/lib.rs".to_owned(),
                tool_part_id: Some("tool-part:test:noop-write".to_owned()),
                tool_call_id: Some("call-noop-write".to_owned()),
                path: "src/lib.rs".to_owned(),
                change: FileChange::Update {
                    unified_diff: String::new(),
                    move_path: None,
                },
            },
        )),
    );

    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 0);
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
}

#[test]
fn repeated_file_change_snapshots_replace_one_diff_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "src/error.rs".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );

    for content in ["line 1\n", "line 1\nline 2\n"] {
        state.apply_runtime_event(
            crate::r#loop::agent_tui::wire_event_test_support::wire_event(
                WireEventKind::FileChange(WireFileChangeEvent {
                    part_id: "diff:tool-part:test:fixture:src/error.rs".to_owned(),
                    tool_part_id: Some("tool-part:test:fixture".to_owned()),
                    tool_call_id: Some("call-1".to_owned()),
                    path: "src/error.rs".to_owned(),
                    change: FileChange::Add {
                        content: content.to_owned(),
                    },
                }),
            ),
        );
    }

    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 1);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Diff(cell))
                if matches!(
                    cell.changes.get(&PathBuf::from("src/error.rs")),
                    Some(FileChange::Add { content }) if content == "line 1\nline 2\n"
                )
        )
    }));
}

#[test]
fn file_change_snapshots_for_one_tool_merge_into_one_diff_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:merge-files".to_owned(),
                request_id: 1,
                provider_call_id: "call-merge-files".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "docs".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );

    for (path, content) in [("docs/first.md", "first\n"), ("docs/second.md", "second\n")] {
        state.apply_runtime_event(
            crate::r#loop::agent_tui::wire_event_test_support::wire_event(
                WireEventKind::FileChange(WireFileChangeEvent {
                    part_id: format!("diff:tool-part:test:merge-files:{path}"),
                    tool_part_id: Some("tool-part:test:merge-files".to_owned()),
                    tool_call_id: Some("call-merge-files".to_owned()),
                    path: path.to_owned(),
                    change: FileChange::Add {
                        content: content.to_owned(),
                    },
                }),
            ),
        );
    }

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    let diff = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Diff(cell)) => Some(cell),
            _ => None,
        })
        .expect("one merged diff cell");
    assert_eq!(diff.changes.len(), 2);
}

#[test]
fn final_diff_replaces_completed_tool_cell_without_duplicate_projection() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:final-diff".to_owned(),
                request_id: 1,
                provider_call_id: "call-final-diff".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "docs/README.md".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    let mut receipt = tool_receipt(
        "call-final-diff",
        "builtin.edit",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Edit,
            status: "completed".to_owned(),
            summary: "已完成".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: true,
        },
    );
    receipt.id = "tool-part:test:final-diff".to_owned();
    output.push(AgentTuiRunLine::ToolReceipt(receipt));
    let mut changes = BTreeMap::new();
    changes.insert(
        PathBuf::from("docs/README.md"),
        FileChange::Add {
            content: "updated\n".to_owned(),
        },
    );
    output.push(AgentTuiRunLine::Diff(AgentTuiDiffLine::with_id(
        "diff:tool-part:test:final-diff:docs/README.md",
        "文件变更",
        changes,
        0,
        0,
    )));

    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 1);
}

#[test]
fn final_multiple_diffs_for_one_tool_merge_after_receipt() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:final-multi-diff".to_owned(),
                request_id: 1,
                provider_call_id: "call-final-multi-diff".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "docs".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    let mut receipt = tool_receipt(
        "call-final-multi-diff",
        "builtin.edit",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Edit,
            status: "completed".to_owned(),
            summary: "已完成".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: true,
        },
    );
    receipt.id = "tool-part:test:final-multi-diff".to_owned();
    output.push(AgentTuiRunLine::ToolReceipt(receipt));
    for (path, content) in [("docs/one.md", "one\n"), ("docs/two.md", "two\n")] {
        let mut changes = BTreeMap::new();
        changes.insert(
            PathBuf::from(path),
            FileChange::Add {
                content: content.to_owned(),
            },
        );
        output.push(AgentTuiRunLine::Diff(AgentTuiDiffLine::with_id(
            format!("diff:tool-part:test:final-multi-diff:{path}"),
            "文件变更",
            changes,
            0,
            0,
        )));
    }

    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    let diff_count = state
        .cells
        .transcript
        .iter()
        .filter(|line| matches!(line.data, Some(RenderCellData::Diff(_))))
        .count();
    assert_eq!(diff_count, 1);
}

#[test]
fn tool_result_after_file_change_does_not_project_duplicate_exec_cell_when_write_performed() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("修改文件"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:live-edit".to_owned(),
                request_id: 1,
                provider_call_id: "call-live-edit".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "Cargo.toml".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::FileChange(
            WireFileChangeEvent {
                part_id: "diff:tool-part:test:live-edit:Cargo.toml".to_owned(),
                tool_part_id: Some("tool-part:test:live-edit".to_owned()),
                tool_call_id: Some("call-live-edit".to_owned()),
                path: "Cargo.toml".to_owned(),
                change: FileChange::Update {
                    unified_diff: "-old\n+new".to_owned(),
                    move_path: None,
                },
            },
        )),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: "tool-part:test:live-edit".to_owned(),
                provider_call_id: "call-live-edit".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                status: "completed".to_owned(),
                summary: "Edit completed".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: true,
                duration_ms: Some(100),
            },
        ),
    );

    assert_eq!(cell_count(&state, RenderCellKind::DIFF), 1);
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
}
