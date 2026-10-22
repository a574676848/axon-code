//! Assistant stream part projection tests。

use std::time::{Duration, Instant};

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::{
    AgentTuiMessagePart, AgentTuiSubmitFailure,
    run_output::{AgentTuiRunLine, AgentTuiRunOutput},
};
use crate::surface::cell::{RenderCellData, RenderMessageSourceKind};

#[test]
fn assistant_stream_commits_update_one_render_cell_revision() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("连续流式提交"));
    let now = Instant::now();

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "第一行\n".to_owned(),
        ),
    );
    assert!(state.tick(now + Duration::from_millis(90)));
    let first = state.cells.transcript.last().expect("first assistant cell");
    let render_id = first.render_id();
    let first_revision = first.render_revision();
    assert_eq!(first.text, "第一行\n");

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "第二行\n".to_owned(),
        ),
    );
    assert!(state.tick(now + Duration::from_millis(180)));
    let updated = state
        .cells
        .transcript
        .last()
        .expect("updated assistant cell");

    assert_eq!(updated.render_id(), render_id);
    assert!(updated.render_revision() > first_revision);
    assert_eq!(updated.text, "第一行\n第二行\n");
    let Some(RenderCellData::Message(updated_cell)) = &updated.data else {
        panic!("updated assistant cell should keep message data");
    };
    assert_eq!(updated_cell.id, "assistant:default");
    assert_eq!(updated_cell.source_kind, RenderMessageSourceKind::Stream);
    assert_eq!(
        state
            .cells
            .transcript
            .iter()
            .filter(|line| matches!(line.data, Some(RenderCellData::Message(_))))
            .count(),
        1
    );

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));
    let finalized = state
        .cells
        .transcript
        .last()
        .expect("finalized assistant cell");
    assert_eq!(finalized.text, "第一行\n第二行\n");
    assert!(matches!(
        finalized.data,
        Some(RenderCellData::Message(ref cell))
            if cell.id == "assistant:default"
                && cell.source_kind == RenderMessageSourceKind::Markdown
    ));
}

#[test]
fn assistant_stream_part_id_switch_commits_independent_parts() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("多 assistant part"));
    let now = Instant::now();

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            "assistant:a",
            "provider-message:a",
            "第一段",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(90)));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            "assistant:b",
            "provider-message:b",
            "第二段",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(180)));
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    let assistant_lines = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Message(cell))
                if cell.source_kind == RenderMessageSourceKind::Markdown =>
            {
                Some((cell.id.as_str(), line.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        assistant_lines,
        vec![("assistant:a", "第一段"), ("assistant:b", "第二段")]
    );
}

#[test]
fn interleaved_child_streams_keep_two_active_cells_and_update_by_part() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("并行子代理节点"));
    let now = Instant::now();
    let first_part = "child:run-1:node:inspect-api:attempt:1:assistant";
    let second_part = "child:run-1:node:inspect-ui:attempt:1:assistant";

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            first_part,
            "provider-message:api",
            "API-1\n",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(90)));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            second_part,
            "provider-message:ui",
            "UI-1\n",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(180)));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            first_part,
            "provider-message:api",
            "API-2\n",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(270)));

    let active_messages = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Message(cell))
                if cell.source_kind == RenderMessageSourceKind::Stream =>
            {
                Some((cell.id.as_str(), line.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        active_messages,
        vec![(first_part, "API-1\nAPI-2\n"), (second_part, "UI-1\n")]
    );
    assert_eq!(state.bridge.assistant_streams.stream_count(), 2);
    assert!(state.bridge.assistant_streams.anchor(first_part).is_some());
    assert!(state.bridge.assistant_streams.anchor(second_part).is_some());
}

#[test]
fn assistant_full_message_replaces_delta_part_without_duplication() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("final assistant part"));
    let now = Instant::now();

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            "assistant:final",
            "provider-message:final",
            "局部",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(90)));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_message_event_with_id(
            "assistant:final",
            "provider-message:final",
            "完整回答",
            false,
        ),
    );
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    let assistant_lines = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Message(cell))
                if cell.source_kind == RenderMessageSourceKind::Markdown =>
            {
                Some((cell.id.as_str(), line.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(assistant_lines, vec![("assistant:final", "完整回答")]);
}

/// F-6 回归：邮箱淘汰 delta 后，失败终态必须用宿主交出的 durable 重放补齐正文。
///
/// 实时邮箱只淘汰可重建的展示增量；被淘汰的中段只能由终态按 durable 顺序完整重放
/// 补齐。失败若只上报错误文本，正文会永久留下无法察觉的缺口。
#[test]
fn failed_submit_replays_durable_text_over_shed_delta_part() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("过载失败收口"));
    let now = Instant::now();

    // 中段增量已被邮箱淘汰：实时流只剩尾部。
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            "assistant:shed",
            "provider-message:shed",
            "尾部",
        ),
    );
    assert!(state.tick(now + Duration::from_millis(90)));

    let mut terminal = AgentTuiRunOutput::empty();
    terminal.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
        "assistant:shed",
        "开头中段尾部",
    )));
    state.finish_submit_with_cause(
        Err(AgentTuiSubmitFailure::with_output(
            crate::RuntimeError::runtime("provider failed"),
            terminal,
        )),
        None,
    );

    assert_eq!(
        state.session_summary().failed_turns,
        1,
        "携带 durable 重放的失败终态仍必须按失败收口"
    );
    let assistant_lines = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Message(cell))
                if cell.source_kind == RenderMessageSourceKind::Markdown =>
            {
                Some((cell.id.as_str(), line.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        assistant_lines,
        vec![("assistant:shed", "开头中段尾部")],
        "被淘汰的中段必须由宿主交出的 durable 重放补齐"
    );
}

#[test]
fn assistant_duplicate_full_snapshot_replaces_existing_part_without_duplication() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("重复 assistant snapshot"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_message_event_with_id(
            "assistant:repeat",
            "provider-message:repeat-1",
            "相同回复",
            false,
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_message_event_with_id(
            "assistant:repeat",
            "provider-message:repeat-2",
            "相同回复",
            false,
        ),
    );
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    let assistant_lines = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::Message(cell))
                if cell.source_kind == RenderMessageSourceKind::Markdown =>
            {
                Some((cell.id.as_str(), line.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(assistant_lines, vec![("assistant:repeat", "相同回复")]);
}

#[test]
fn assistant_stream_table_deltas_keep_one_markdown_context_in_active_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("三帧表格"));
    let now = Instant::now();
    let deltas = [
        "| 现状 | 状态 |\n",
        "| --- | --- |\n",
        "| 流式展示 | 正常 |\n",
    ];

    for (index, delta) in deltas.into_iter().enumerate() {
        state.apply_runtime_event(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
                delta.to_owned(),
            ),
        );
        assert!(state.tick(now + Duration::from_secs(1)));

        let source = state
            .cells
            .transcript
            .iter()
            .find_map(|line| match &line.data {
                Some(RenderCellData::Message(cell))
                    if cell.source_kind == RenderMessageSourceKind::Stream =>
                {
                    Some(line.text.clone())
                }
                _ => None,
            })
            .expect("active assistant stream cell");

        let rendered = crate::widgets::render_streaming_markdown_lines(&source, 80)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        match index {
            0 => assert!(rendered.contains("现状") && !rendered.contains('┌')),
            1 => assert!(rendered.contains('┌') && rendered.contains('┘')),
            2 => assert!(rendered.contains('┌') && rendered.contains("流式展示")),
            _ => unreachable!("固定三帧表格测试"),
        }
    }
}

#[test]
fn assistant_stream_preserves_whitespace_only_delta() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("空白增量"));
    let now = Instant::now();

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "第一行\n".to_owned(),
        ),
    );
    assert!(state.tick(now + Duration::from_secs(1)));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event("\n".to_owned()),
    );
    assert!(state.tick(now + Duration::from_secs(1)));

    assert_eq!(
        state.cells.transcript.last().map(|cell| cell.text.as_str()),
        Some("第一行\n\n")
    );
}
