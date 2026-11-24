//! Completed snapshot 与 transcript 对账测试。

use super::*;

#[test]
fn completed_snapshot_reconciles_live_facts_before_streamed_assistant() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行并解释"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::reasoning_start_event(
            "reasoning-part:live",
            "provider-reasoning:live",
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::reasoning_delta_event(
            "reasoning-part:live",
            "provider-reasoning:live",
            "实时思考",
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::reasoning_end_event(
            "reasoning-part:live",
            "provider-reasoning:live",
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                request_id: 1,
                part_id: "tool-part:live".to_owned(),
                provider_call_id: "provider-call:live".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "cargo test".to_owned(),
                intent: None,
                execution_permission: axon_domain::ToolExecutionPermission::Allowed,
            },
        ),
    );
    let skill = AgentTuiHookStatusView::new("Skill", "completed", Some("技能已完成".to_owned()));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(skill.clone()),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "最终回复".to_owned(),
        ),
    );

    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Reasoning(AgentTuiReasoningLine::new(
        "reasoning-part:live",
        "最终思考",
    )));
    output.push(AgentTuiRunLine::Reasoning(AgentTuiReasoningLine::new(
        "reasoning-part:recovered",
        "恢复的思考",
    )));
    output.push(AgentTuiRunLine::ToolLifecycle(
        AgentTuiToolLifecycleLine::with_id(AgentTuiToolLifecycleLineInput {
            id: "tool-lifecycle:live".to_owned(),
            tool_part_id: "tool-part:live".to_owned(),
            provider_call_id: "provider-call:live".to_owned(),
            tool_name: "shell_command".to_owned(),
            kind: AgentTuiToolKind::Shell,
            status: "running".to_owned(),
            summary: "运行中".to_owned(),
            arguments_preview: Some("cargo test".to_owned()),
            output_preview: None,
        }),
    ));
    output.push(AgentTuiRunLine::ToolReceipt(AgentTuiToolReceipt::with_id(
        "tool-part:live",
        "provider-call:live",
        "shell_command",
        crate::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "blocked".to_owned(),
            summary: "工具执行被权限策略禁止".to_owned(),
            metadata: None,
            error_type: Some("permission_denied".to_owned()),
            filesystem_write_performed: false,
        },
    )));
    output.push(AgentTuiRunLine::ToolLifecycle(
        AgentTuiToolLifecycleLine::with_id(AgentTuiToolLifecycleLineInput {
            id: "tool-lifecycle:recovered".to_owned(),
            tool_part_id: "tool-part:recovered".to_owned(),
            provider_call_id: "provider-call:recovered".to_owned(),
            tool_name: "builtin_read_file".to_owned(),
            kind: AgentTuiToolKind::Shell,
            status: "running".to_owned(),
            summary: "运行中".to_owned(),
            arguments_preview: Some("Cargo.toml".to_owned()),
            output_preview: None,
        }),
    ));
    output.push(AgentTuiRunLine::ToolReceipt(AgentTuiToolReceipt::with_id(
        "tool-part:recovered",
        "provider-call:recovered",
        "builtin_read_file",
        crate::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "读取完成".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));
    output.push(AgentTuiRunLine::Hook(skill));
    // durable 顺序：canonical assistant 是本轮最后的事实。
    output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
        "assistant:default",
        "最终回复",
    )));

    state.finish_submit(Ok(output));

    let assistant_index = state
        .cells
        .transcript
        .iter()
        .position(|line| matches!(line.data, Some(RenderCellData::Message(_))))
        .expect("assistant should be committed");
    for (index, line) in state.cells.transcript.iter().enumerate() {
        if matches!(
            line.data,
            Some(RenderCellData::Reasoning(_))
                | Some(RenderCellData::Exec(_))
                | Some(RenderCellData::History(_))
        ) {
            assert!(
                index < assistant_index,
                "process cell replayed after assistant: index={index}, assistant_index={assistant_index}, part_id={:?}, data={:?}",
                line.semantic_part_id(),
                line.data
            );
        }
    }

    let semantic_ids = state
        .cells
        .transcript
        .iter()
        .filter_map(crate::surface::cell::RenderCellRecord::semantic_part_id)
        .filter(|id| {
            id.starts_with("reasoning-part:")
                || id.starts_with("tool-part:")
                || id.starts_with("history:hook:")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        semantic_ids.len(),
        semantic_ids.iter().collect::<HashSet<_>>().len(),
        "completed snapshot should upsert semantic facts"
    );
    assert_eq!(cell_count(&state, RenderCellKind::REASONING), 2);
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 2);
    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::MESSAGE), 1);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(&line.data, Some(RenderCellData::Exec(cell))
            if cell.id == "tool-part:live" && cell.status.label() == "失败")
    }));
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(&line.data, Some(RenderCellData::Exec(cell))
            if cell.id == "tool-part:recovered" && cell.status.label() == "完成")
    }));
}

#[test]
fn terminal_replay_is_idempotent_by_part_identity() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("重复重放"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
            "assistant-part:1",
            "provider-message:1",
            "实时片段",
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_message_event_with_id(
            "assistant-part:1",
            "assistant-message-final",
            "最终回答",
            false,
        ),
    );

    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
        "assistant-part:1",
        "最终回答",
    )));

    state.apply_run_output(output.clone(), false, false, None);
    state.apply_run_output(output, false, false, None);

    let messages = state
        .cells
        .transcript
        .iter()
        .filter(|line| matches!(line.data, Some(RenderCellData::Message(_))))
        .collect::<Vec<_>>();
    assert_eq!(
        messages.len(),
        1,
        "终态重放必须按 part id 原位替换，而不是追加第二条正文"
    );
    assert_eq!(messages[0].text, "最终回答");
}

#[test]
fn completed_snapshot_reconciliation_is_scoped_to_current_turn() {
    let mut state = test_state();
    for (prompt, summary) in [("第一轮", "第一轮 Skill"), ("第二轮", "第二轮 Skill")] {
        state.start_runtime_submit(&test_submission(prompt));
        let mut output = AgentTuiRunOutput::empty();
        output.push(AgentTuiRunLine::Hook(AgentTuiHookStatusView::new(
            "Skill",
            "completed",
            Some(summary.to_owned()),
        )));
        output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
            "assistant:default",
            format!("{prompt}回复"),
        )));
        state.finish_submit(Ok(output));
    }

    let skill_summaries = state
        .cells
        .transcript
        .iter()
        .filter_map(|line| match &line.data {
            Some(RenderCellData::History(cell)) if cell.id == "history:hook:Skill" => {
                cell.items.first().map(|item| item.name.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(skill_summaries, vec!["第一轮 Skill", "第二轮 Skill"]);
}
