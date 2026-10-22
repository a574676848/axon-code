//! MCP / History cell 生命周期投射测试。

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::run_output::{
    AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput, AgentTuiRunLine,
    AgentTuiRunOutput, AgentTuiRuntimeEvent, AgentTuiToolKind, AgentTuiToolOutputChunk,
    AgentTuiToolReceipt,
};
use crate::r#loop::agent_tui::state::AgentTuiState;
use crate::surface::cell::{RenderCellData, RenderCellKind};

#[test]
fn mcp_output_chunk_updates_active_mcp_cell_without_exec_fallback() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("调用 MCP"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            AgentTuiToolOutputChunk {
                part_id: "tool-part:test:call-mcp".to_owned(),
                provider_call_id: "call-mcp".to_owned(),
                tool_name: "mcp__gitnexus__query".to_owned(),
                semantic_kind: AgentTuiToolKind::Mcp,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: "2 条结果".to_owned(),
            },
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    assert_eq!(cell_count(&state, RenderCellKind::MCP), 1);
    assert_eq!(typed_mcp_first_output(&state).as_deref(), Some("2 条结果"));
}

#[test]
fn mcp_receipt_settles_active_mcp_cell_and_keeps_output_preview() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("调用 MCP"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            AgentTuiToolOutputChunk {
                part_id: "tool-part:test:call-mcp".to_owned(),
                provider_call_id: "call-mcp".to_owned(),
                tool_name: "mcp__gitnexus__query".to_owned(),
                semantic_kind: AgentTuiToolKind::Mcp,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: "2 条结果".to_owned(),
            },
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(AgentTuiToolReceipt::with_id(
        "tool-part:test:call-mcp",
        "call-mcp",
        "mcp__gitnexus__query",
        crate::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Mcp,
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: Some(serde_json::json!({ "server": "gitnexus", "tool": "query" })),
            error_type: None,
            filesystem_write_performed: false,
        },
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    assert_eq!(cell_count(&state, RenderCellKind::MCP), 1);
    assert_eq!(typed_mcp_first_status(&state).as_deref(), Some("Completed"));
    assert_eq!(typed_mcp_first_output(&state).as_deref(), Some("2 条结果"));
}

#[test]
fn history_lifecycle_replaces_running_compaction_cell_on_completion() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("压缩上下文"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::with_id(AgentTuiLifecycleStatusViewInput {
                id: "history:lifecycle:test-compaction".to_owned(),
                title: "历史".to_owned(),
                kind: "CompactionStarted".to_owned(),
                summary: "压缩上下文".to_owned(),
                phase: "history".to_owned(),
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            }),
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::with_id(AgentTuiLifecycleStatusViewInput {
                id: "history:lifecycle:test-compaction".to_owned(),
                title: "历史".to_owned(),
                kind: "CompactionCompleted".to_owned(),
                summary: "保留关键事实".to_owned(),
                phase: "history".to_owned(),
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            }),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert_eq!(
        typed_history_first_name(&state).as_deref(),
        Some("保留关键事实")
    );
}

#[test]
fn history_lifecycle_keeps_compaction_failure_and_idle_schedule_visible() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("等待压缩"));

    state.apply_runtime_event(compaction_history_lifecycle_event(
        "history:lifecycle:compaction-failed",
        "CompactionFailed",
        "内联窗口轮转未完成：窗口轮转未通过门禁",
    ));
    state.apply_runtime_event(compaction_history_lifecycle_event(
        "history:lifecycle:compaction-planned",
        "CompactionPlanned",
        "空闲窗口轮转已排期",
    ));

    assert_eq!(
        typed_history_names(&state),
        vec![
            "内联窗口轮转未完成：窗口轮转未通过门禁".to_owned(),
            "空闲窗口轮转已排期".to_owned(),
        ],
        "压缩失败与空闲排期必须各自落到 history cell：phase=history 不会进入通用 lifecycle 通道，缺少映射会被整条丢弃"
    );
}

fn compaction_history_lifecycle_event(id: &str, kind: &str, summary: &str) -> AgentTuiRuntimeEvent {
    crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
        AgentTuiLifecycleStatusView::with_id(AgentTuiLifecycleStatusViewInput {
            id: id.to_owned(),
            title: "窗口轮转".to_owned(),
            kind: kind.to_owned(),
            summary: summary.to_owned(),
            phase: "history".to_owned(),
            tool_name: None,
            duration_ms: None,
            attempt_index: None,
        }),
    )
}

fn typed_history_names(state: &AgentTuiState) -> Vec<String> {
    state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::History(cell)) => cell.items.first().map(|item| item.name.clone()),
            _ => None,
        })
        .collect()
}
