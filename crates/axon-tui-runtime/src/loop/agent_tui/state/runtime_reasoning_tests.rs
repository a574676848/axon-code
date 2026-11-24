//! Runtime reasoning transcript part 投射测试。

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::{
    run_output::{AgentTuiReasoningLine, AgentTuiRunLine, AgentTuiRunOutput},
    wire_event_test_support::{reasoning_delta_event, reasoning_end_event, reasoning_start_event},
};
use crate::surface::cell::RenderCellKind;

#[test]
fn reasoning_delta_projects_independent_cell_and_commits_on_finish() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("解释这段代码"));

    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
        "先读入口",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
        "，再查调用方",
    ));

    assert_eq!(cell_count(&state, RenderCellKind::REASONING), 1);
    assert_eq!(
        active_reasoning_text(&state).as_deref(),
        Some("先读入口，再查调用方")
    );

    state.apply_runtime_event(reasoning_end_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
    ));
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(
        typed_reasoning(&state),
        Some(("先读入口，再查调用方".to_owned(), true))
    );
    let elapsed_ms = state.cells.transcript.iter().find_map(|line| {
        let crate::surface::cell::RenderCellData::Reasoning(cell) = line.data.as_ref()? else {
            return None;
        };
        cell.elapsed_ms
    });
    assert!(elapsed_ms.is_some());
}

#[test]
fn reasoning_completion_clears_expanded_selection() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("验证自动折叠"));
    let part_id = "reasoning-part:collapse";
    state.apply_runtime_event(reasoning_start_event(
        part_id,
        "provider-reasoning:collapse",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        part_id,
        "provider-reasoning:collapse",
        "完成后应自动折叠",
    ));
    let index = state
        .cells
        .transcript
        .iter()
        .position(|line| {
            matches!(
                &line.data,
                Some(crate::surface::cell::RenderCellData::Reasoning(cell)) if cell.id == part_id
            )
        })
        .expect("reasoning cell should be projected");
    state
        .viewport
        .select_runtime_part(index, part_id.to_owned());

    state.apply_runtime_event(reasoning_end_event(part_id, "provider-reasoning:collapse"));

    assert_eq!(state.viewport.selected_runtime_part_id(), None);
}

#[test]
fn final_submit_clears_selection_for_unended_reasoning() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("验证最终收口折叠"));
    let part_id = "reasoning-part:final-collapse";
    state.apply_runtime_event(reasoning_start_event(
        part_id,
        "provider-reasoning:final-collapse",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        part_id,
        "provider-reasoning:final-collapse",
        "最终收口时应折叠",
    ));
    let index = state
        .cells
        .transcript
        .iter()
        .position(|line| {
            matches!(
                &line.data,
                Some(crate::surface::cell::RenderCellData::Reasoning(cell)) if cell.id == part_id
            )
        })
        .expect("reasoning cell should be projected");
    state
        .viewport
        .select_runtime_part(index, part_id.to_owned());

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(state.viewport.selected_runtime_part_id(), None);
}

#[test]
fn finish_submit_merges_completed_reasoning_with_streamed_part() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("解释这段代码"));

    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
        "流式思考",
    ));
    state.apply_runtime_event(reasoning_end_event(
        "reasoning-part:turn-1",
        "provider-reasoning:turn-1",
    ));

    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Reasoning(AgentTuiReasoningLine::new(
        "reasoning-part:turn-1",
        "最终思考摘要",
    )));
    state.finish_submit(Ok(output));

    assert_eq!(
        typed_reasoning_cells(&state),
        vec![(
            "reasoning-part:turn-1".to_owned(),
            "最终思考摘要".to_owned(),
            true,
        )]
    );
}

#[test]
fn reasoning_delta_keeps_interleaved_ids_as_independent_parts() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("比较两个方案"));

    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:a",
        "provider-reasoning:a",
    ));
    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:b",
        "provider-reasoning:b",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:a",
        "provider-reasoning:a",
        "方案A",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:b",
        "provider-reasoning:b",
        "方案B",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:a",
        "provider-reasoning:a",
        "继续分析",
    ));

    assert_eq!(cell_count(&state, RenderCellKind::REASONING), 2);
    assert_eq!(
        typed_reasoning_cells(&state),
        vec![
            (
                "reasoning-part:a".to_owned(),
                "方案A继续分析".to_owned(),
                false,
            ),
            ("reasoning-part:b".to_owned(), "方案B".to_owned(), false),
        ]
    );

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(
        typed_reasoning_cells(&state),
        vec![
            (
                "reasoning-part:a".to_owned(),
                "方案A继续分析".to_owned(),
                true,
            ),
            ("reasoning-part:b".to_owned(), "方案B".to_owned(), true),
        ]
    );
}

#[test]
fn interrupt_commits_visible_reasoning_parts() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("分析失败路径"));

    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:interrupt",
        "provider-reasoning:interrupt",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:interrupt",
        "provider-reasoning:interrupt",
        "已展示的 reasoning",
    ));
    state.request_interrupt_current_task();

    assert_eq!(
        typed_reasoning_cells(&state),
        vec![(
            "reasoning-part:interrupt".to_owned(),
            "已展示的 reasoning".to_owned(),
            true,
        )]
    );
}

#[test]
fn durable_orphan_reasoning_delta_recreates_part() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("忽略孤儿 delta"));

    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:orphan",
        "provider-reasoning:orphan",
        "历史思考",
    ));

    assert_eq!(cell_count(&state, RenderCellKind::REASONING), 1);
    assert_eq!(active_reasoning_text(&state).as_deref(), Some("历史思考"));

    state.apply_runtime_event(reasoning_end_event(
        "reasoning-part:orphan",
        "provider-reasoning:orphan",
    ));
    assert!(
        typed_reasoning(&state).is_some_and(|(_, done)| done),
        "缺失 Start 的 durable delta 也应能被 End 正常收口"
    );
}

#[test]
fn reasoning_parts_collapse_and_reopen_independently() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("验证 reasoning 分段折叠"));

    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:first",
        "provider-reasoning:first",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:first",
        "provider-reasoning:first",
        "第一段思考",
    ));
    state.apply_runtime_event(reasoning_end_event(
        "reasoning-part:first",
        "provider-reasoning:first",
    ));

    assert_eq!(
        typed_reasoning_cells(&state),
        vec![(
            "reasoning-part:first".to_owned(),
            "第一段思考".to_owned(),
            true,
        )]
    );

    state.apply_runtime_event(reasoning_start_event(
        "reasoning-part:second",
        "provider-reasoning:second",
    ));
    state.apply_runtime_event(reasoning_delta_event(
        "reasoning-part:second",
        "provider-reasoning:second",
        "第二段思考",
    ));

    assert_eq!(
        typed_reasoning_cells(&state),
        vec![
            (
                "reasoning-part:first".to_owned(),
                "第一段思考".to_owned(),
                true,
            ),
            (
                "reasoning-part:second".to_owned(),
                "第二段思考".to_owned(),
                false,
            ),
        ]
    );

    state.apply_runtime_event(reasoning_end_event(
        "reasoning-part:second",
        "provider-reasoning:second",
    ));

    assert!(
        typed_reasoning_cells(&state)
            .into_iter()
            .all(|(_, _, done)| done),
        "每个 reasoning part 都应在自身结束事件到达后独立收口"
    );
}
