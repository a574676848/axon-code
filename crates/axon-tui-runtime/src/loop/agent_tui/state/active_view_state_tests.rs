//! Active view tail projection tests。

use crate::{
    r#loop::agent_tui::state::runtime_flow_test_support::{test_state, test_submission},
    surface::{
        cell::{RenderCellData, RenderCellMode, RenderCellRecord, RenderMessageSourceKind},
        cell_store::CellStoreError,
    },
};

#[test]
fn active_view_projects_tail_only_for_first_cold_bottom_mount_frame() {
    let mut state = test_state();
    state.cells.transcript.clear();
    for index in 0..16 {
        state
            .cells
            .transcript
            .push(RenderCellRecord::system(format!("运行行 {index}")));
        state.cells.active_runtime_cells.set_tool_cell(
            format!("call-{index}"),
            index,
            std::time::Instant::now(),
        );
    }

    assert!(state.prepare_cold_bottom_mount());
    let first_frame = state
        .active_view_lines()
        .into_iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>();

    assert_eq!(first_frame.len(), 10);
    assert_eq!(first_frame.first().map(String::as_str), Some("运行行 6"));
    assert_eq!(first_frame.last().map(String::as_str), Some("运行行 15"));

    assert!(state.complete_cold_bottom_mount_frame());
    assert_eq!(state.active_view_lines().len(), 16);
}

#[test]
fn startup_lines_do_not_enter_cold_bottom_mount_window() {
    let mut state = test_state();
    state
        .cells
        .transcript
        .replace_with_records(vec![RenderCellRecord::system("启动行")]);
    state.cells.transcript[0].mode = RenderCellMode::Startup;

    assert!(!state.prepare_cold_bottom_mount());

    let lines = state.active_view_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "启动行");
}

#[test]
fn assistant_stream_tail_commits_after_consolidation() {
    let mut state = test_state();
    state.cells.transcript.clear();
    state.upsert_active_assistant_cell(RenderCellRecord::streaming_agent_message(
        "assistant:test:stream-tail",
        "第一段",
    ));
    let active_index = state
        .bridge
        .assistant_streams
        .anchor("assistant:test:stream-tail")
        .map(crate::r#loop::agent_tui::runtime_bridge::RuntimePartAnchor::cell_index)
        .expect("streaming tail should be active");

    let original_render_id = state
        .cells
        .transcript
        .get(active_index)
        .map(RenderCellRecord::render_id)
        .expect("active assistant cell");

    state.upsert_active_assistant_cell(RenderCellRecord::streaming_agent_message(
        "assistant:test:stream-tail",
        "第一段\n第二段",
    ));

    let replacement = state
        .cells
        .transcript
        .get(active_index)
        .expect("replaced active assistant cell");
    assert_eq!(replacement.render_id(), original_render_id);
    assert_eq!(replacement.render_revision(), 1);
    assert_eq!(replacement.text, "第一段\n第二段");
    assert!(matches!(
        &replacement.data,
        Some(RenderCellData::Message(cell))
            if cell.source_kind == RenderMessageSourceKind::Stream
    ));

    state.apply_tui_event(
        crate::r#loop::agent_tui::tui_event::AgentTuiEvent::ConsolidateAgentMessage {
            cell: RenderCellRecord::agent_message("assistant:test:stream-tail", "第一段\n第二段"),
        },
    );

    assert!(
        state
            .bridge
            .assistant_streams
            .anchor("assistant:test:stream-tail")
            .is_none()
    );
    assert!(matches!(
        state
            .cells
            .transcript
            .mutate_cell(active_index, |cell| cell.text = "不应修改".to_owned()),
        Err(CellStoreError::Inactive { .. })
    ));
    assert_eq!(
        state
            .cells
            .transcript
            .get(active_index)
            .map(|cell| cell.text.as_str()),
        Some("第一段\n第二段")
    );
}

#[test]
fn assistant_final_cell_replaces_stream_tail_when_anchor_is_unavailable() {
    let mut state = test_state();
    state.cells.transcript.clear();
    state
        .cells
        .transcript
        .push(RenderCellRecord::streaming_agent_message(
            "assistant:test:missing-anchor",
            "第一段",
        ));
    state
        .cells
        .transcript
        .push(RenderCellRecord::streaming_agent_message(
            "assistant:test:missing-anchor",
            "第一段\n第二段",
        ));

    state.apply_tui_event(
        crate::r#loop::agent_tui::tui_event::AgentTuiEvent::ConsolidateAgentMessage {
            cell: RenderCellRecord::agent_message(
                "assistant:test:missing-anchor",
                "第一段\n第二段",
            ),
        },
    );

    assert_eq!(state.cells.transcript.len(), 1);
    assert!(matches!(
        state.cells.transcript.get(0).map(|cell| &cell.data),
        Some(Some(RenderCellData::Message(message)))
            if message.source_kind == RenderMessageSourceKind::Markdown
    ));
}

#[test]
fn live_transcript_tail_filters_generation_status() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("等待模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::streaming_started_event(
            "cpa", "gpt-5.4", 1,
        ),
    );

    assert!(state.active_transcript_lines().iter().any(|line| {
        matches!(line.data, Some(RenderCellData::GenerationStatus(_))) && line.text.contains("生成")
    }));
    assert!(state.live_transcript_tail_lines().is_none());
}
