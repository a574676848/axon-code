//! Exec cell 生命周期投射测试。

use super::runtime_flow_test_support::*;
use crate::app_shell::composer_dock::DockKind;
use crate::r#loop::agent_tui::run_output::{
    AgentTuiRunLine, AgentTuiRunOutput, AgentTuiToolCompletionView, AgentTuiToolExecutionView,
    AgentTuiToolKind, AgentTuiToolProgressView, AgentTuiToolReceipt, AgentTuiToolReceiptDetails,
};
use crate::surface::cell::{RenderCellData, RenderCellKind};
use axon_domain::ToolExecutionPermission;

#[test]
fn tool_receipt_merge_updates_typed_exec_status() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo check".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(tool_receipt(
        "call-1",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));
    state.finish_submit(Ok(output));

    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("完成"));
    assert!(typed_exec_first_duration_ms(&state).is_some());
}

#[test]
fn tool_execution_lifecycle_updates_single_active_exec_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo check".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            crate::r#loop::agent_tui::run_output::AgentTuiToolOutputChunk {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: "checking".to_owned(),
            },
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(tool_receipt(
        "call-1",
        "shell_command",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("完成"));
    assert_eq!(typed_exec_first_output(&state).as_deref(), Some("checking"));
    assert_eq!(typed_exec_first_summary(&state), None);
    assert!(typed_exec_first_duration_ms(&state).is_some());
}

#[test]
fn active_tool_cells_are_indexed_by_part_id_not_provider_call_id() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    for part_id in ["tool-part:test:first", "tool-part:test:second"] {
        state.apply_runtime_event(
            crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
                AgentTuiToolExecutionView {
                    part_id: part_id.to_owned(),
                    request_id: 1,
                    provider_call_id: "provider-call:shared".to_owned(),
                    tool_name: "shell_command".to_owned(),
                    kind: "shell".to_owned(),
                    semantic_kind: AgentTuiToolKind::Shell,
                    arguments_preview: part_id.to_owned(),
                    intent: None,
                    execution_permission: ToolExecutionPermission::Allowed,
                },
            ),
        );
    }
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            crate::r#loop::agent_tui::run_output::AgentTuiToolOutputChunk {
                part_id: "tool-part:test:first".to_owned(),
                provider_call_id: "provider-call:shared".to_owned(),
                tool_name: "shell_command".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: "first output".to_owned(),
            },
        ),
    );

    let exec_cells = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Exec(cell)) => {
                Some((cell.id.as_str(), cell.output_preview.as_deref()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        exec_cells,
        vec![
            ("tool-part:test:first", Some("first output")),
            ("tool-part:test:second", None),
        ]
    );
}

#[test]
fn tool_progress_updates_single_active_exec_cell_without_output_chunk_regression() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("搜索资料"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.search".to_owned(),
                kind: "search".to_owned(),
                semantic_kind: AgentTuiToolKind::Search,
                arguments_preview: "{\"query\":\"axon\"}".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_progress_event(
            AgentTuiToolProgressView {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.search".to_owned(),
                semantic_kind: AgentTuiToolKind::Search,
                phase: "searching".to_owned(),
                items_completed: Some(1),
                items_total: Some(2),
                current_item: Some("local".to_owned()),
                elapsed_ms: Some(1200),
                message: Some("返回 3 条结果".to_owned()),
            },
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("运行中"));
    assert_eq!(
        typed_exec_first_output(&state).as_deref(),
        Some("searching · 1/2 · 当前 local · 返回 3 条结果 · 1.2s")
    );
}

#[test]
fn opening_tool_output_dock_closes_other_docks() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo check".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            crate::r#loop::agent_tui::run_output::AgentTuiToolOutputChunk {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: "checking".to_owned(),
            },
        ),
    );
    state.toggle_shortcut_help_dock();

    assert!(state.is_shortcut_help_open());
    assert!(state.open_selected_tool_output_dock());
    assert_eq!(state.active_dock_kind(), Some(DockKind::ToolOutputViewer));
    assert!(!state.is_shortcut_help_open());
}

#[test]
fn dock_state_keeps_shortcut_and_tool_output_mutually_exclusive() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo check".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            crate::r#loop::agent_tui::run_output::AgentTuiToolOutputChunk {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: "checking".to_owned(),
            },
        ),
    );
    state.toggle_shortcut_help_dock();
    assert!(state.is_shortcut_help_open());
    assert_ne!(state.active_dock_kind(), Some(DockKind::ToolOutputViewer));

    state.toggle_shortcut_help_dock();
    assert!(!state.is_shortcut_help_open());
    assert_ne!(state.active_dock_kind(), Some(DockKind::ToolOutputViewer));

    assert!(state.open_selected_tool_output_dock());
    assert_eq!(state.active_dock_kind(), Some(DockKind::ToolOutputViewer));
    assert!(!state.is_shortcut_help_open());

    state.close_tool_output_viewer_dock();
    assert_ne!(state.active_dock_kind(), Some(DockKind::ToolOutputViewer));
    assert!(state.viewport.tool_output_dock.copy_text().is_none());
}

#[test]
fn runtime_tool_completed_event_settles_active_exec_cell_with_real_duration() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo check".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                status: "completed".to_owned(),
                summary: "ok".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
                duration_ms: Some(42),
            },
        ),
    );

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("完成"));
    assert_eq!(typed_exec_first_duration_ms(&state), Some(42));
}

#[test]
fn active_tool_cell_finalizes_in_transcript_only_after_completion() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:fixture".to_owned(),
                request_id: 1,
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo check".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("运行中"));
    assert!(typed_exec_first_output(&state).is_none());

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: "tool-part:test:fixture".to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                status: "completed".to_owned(),
                summary: "ok".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
                duration_ms: Some(42),
            },
        ),
    );

    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("完成"));
    assert_eq!(typed_exec_first_duration_ms(&state), Some(42));
}

#[test]
fn late_tool_progress_after_final_receipt_does_not_restart_elapsed() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("应用补丁"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:late-progress".to_owned(),
                request_id: 1,
                provider_call_id: "call-late-progress".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "docs/README.md".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: "tool-part:test:late-progress".to_owned(),
                provider_call_id: "call-late-progress".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                status: "completed".to_owned(),
                summary: "已完成".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: true,
                duration_ms: Some(42),
            },
        ),
    );

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_progress_event(
            AgentTuiToolProgressView {
                part_id: "tool-part:test:late-progress".to_owned(),
                provider_call_id: "call-late-progress".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                phase: "applying".to_owned(),
                items_completed: Some(1),
                items_total: Some(1),
                current_item: Some("docs/README.md".to_owned()),
                elapsed_ms: Some(5_000),
                message: Some("补丁文件已处理".to_owned()),
            },
        ),
    );

    assert_eq!(typed_exec_first_status(&state).as_deref(), Some("完成"));
    assert_eq!(typed_exec_first_duration_ms(&state), Some(42));
}

fn tool_receipt(
    provider_call_id: &str,
    tool_name: &str,
    details: crate::r#loop::agent_tui::run_output::AgentTuiToolReceiptDetails,
) -> AgentTuiToolReceipt {
    AgentTuiToolReceipt::with_id(
        "tool-part:test:fixture",
        provider_call_id,
        tool_name,
        details,
    )
}
