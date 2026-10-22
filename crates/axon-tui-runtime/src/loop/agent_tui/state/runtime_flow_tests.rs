//! 运行流 transcript 投射测试。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axon_protocol::wire::{WireEventKind, WireToolCall};

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiLaunchRequest, AgentTuiModelCatalogItem,
    AgentTuiRunOverrides,
    run_output::{
        AgentTuiHookStatusView, AgentTuiLifecycleLine, AgentTuiLifecycleStatusView,
        AgentTuiMessagePart, AgentTuiReasoningLine, AgentTuiRunLine, AgentTuiRunOutput,
        AgentTuiStatusItem, AgentTuiStatusLine, AgentTuiToolExecutionView, AgentTuiToolKind,
        AgentTuiToolLifecycleLine, AgentTuiToolLifecycleLineInput, AgentTuiToolReceipt,
    },
};
use crate::surface::cell::{RenderCellData, RenderCellKind};

#[test]
fn runtime_status_cell_updates_in_place_and_clears_on_finish() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("实现渲染区"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::streaming_started_event(
            "openai", "gpt-5", 1,
        ),
    );

    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        1
    );
    assert!(active_generation_summary_contains(
        &state,
        "openai/gpt-5 生成回复中"
    ));
    assert!(state.is_working());

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "处理中".to_owned(),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
    assert!(!active_generation_summary_contains(
        &state,
        "openai/gpt-5 生成回复中"
    ));
    assert!(state.tick(Instant::now() + Duration::from_secs(1)));
    assert!(!active_lifecycle_has_indicator(&state));

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
}

#[test]
fn submit_projects_provider_connecting_before_daemon_status_arrives() {
    let mut state = test_state();

    state.start_runtime_submit(&test_submission("等待模型"));

    let activity = state
        .bridge
        .provider_activity
        .current()
        .expect("提交后应有本地连接态");
    assert_eq!(activity.label, "fixture/model 正在连接");
    assert!(activity.animated);
}

#[test]
fn historical_completed_turn_restores_reply_usage_divider_and_session_summary() {
    let mut state = test_state();
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Session(
        crate::r#loop::agent_tui::run_output::AgentTuiRunSessionInfo::new("history-session", None),
    ));
    output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
        "assistant:history",
        "历史最终回复",
    )));
    output.push(AgentTuiRunLine::Usage(
        crate::r#loop::agent_tui::run_output::AgentTuiUsageLine::with_id(
            "usage:history",
            crate::r#loop::agent_tui::run_output::AgentTuiTokenUsage::new(120, 30, 0, 50, 10),
        ),
    ));

    state.restore_historical_turn(output);

    assert!(state.cells.transcript.iter().any(|line| {
        matches!(&line.data, Some(RenderCellData::Message(_))) && line.text.contains("历史最终回复")
    }));
    let summary = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::TurnSummary(summary)) => Some(summary),
            _ => None,
        })
        .expect("历史回合应生成 usage 分割线");
    assert_eq!(summary.duration_ms, None);
    assert_eq!(summary.total_tokens, Some(210));
    assert_eq!(
        state.header.overrides.session.as_deref(),
        Some("history-session")
    );
    assert_eq!(state.header.session_summary.turns, 1);
    assert_eq!(state.header.session_usage.total_tokens(), 210);
    assert_eq!(state.header.hint, crate::r#loop::agent_tui::TUI_HINT_DONE);
}

#[test]
fn runtime_status_enum_tracks_ready_and_working_states() {
    let mut state = test_state();

    assert!(!state.is_working());

    state.start_runtime_submit(&test_submission("检查运行态"));
    assert!(state.is_working());

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));
    assert!(!state.is_working());
}

#[test]
fn wire_runtime_error_is_structured_without_stopping_live_agent() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("继续执行"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::RuntimeError(
            axon_domain::RuntimeFailure::new(
                "mcp_unavailable",
                "mcp_call_failed",
                "gitnexus.query connection refused",
                axon_domain::RUNTIME_FAILURE_TIMESTAMP_UNKNOWN,
            ),
        )),
    );

    assert!(state.is_working());
    assert_eq!(cell_count(&state, RenderCellKind::ERROR), 1);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Error(cell))
                if cell.details.iter().any(|detail| detail.contains("gitnexus.query connection refused"))
                    && cell.details.iter().any(|detail| detail == "代码 mcp_call_failed")
        )
    }));
}

#[test]
fn next_submit_accepts_restarted_wire_sequence_and_projects_tool_immediately() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("第一轮"));
    state
        .bridge
        .turn_ledger
        .seed_sequence("wire:test-session:test-run", u64::MAX);
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    state.start_runtime_submit(&test_submission("第二轮"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::ToolCall(
            WireToolCall {
                part_id: "tool-part:next-turn".to_owned(),
                tool_call_id: "call:next-turn".to_owned(),
                request_id: None,
                tool_name: "builtin.read_file".to_owned(),
                tool_kind: "read_file".to_owned(),
                summary: "读取文件".to_owned(),
                arguments_summary: Some("Cargo.toml".to_owned()),
                intent: None,
                execution_permission: None,
            },
        )),
    );

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
}

#[test]
fn pure_text_turn_does_not_emit_turn_summary_separator() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("解释一下"));

    state.finish_submit(Ok(AgentTuiRunOutput::assistant("纯文本回答")));

    assert_eq!(cell_count(&state, RenderCellKind::TURN_SUMMARY), 0);
}

#[test]
fn worked_turn_emits_single_turn_summary_separator() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(AgentTuiToolReceipt::with_id(
        "tool-part:test:summary",
        "provider-call:test:summary",
        "shell_command",
        crate::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "命令完成".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));

    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::TURN_SUMMARY), 1);
    assert!(matches!(
        state
            .cells
            .transcript
            .last()
            .and_then(|line| line.data.as_ref()),
        Some(RenderCellData::TurnSummary(_))
    ));
}

#[test]
fn worked_turn_summary_keeps_generation_performance() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.bridge.generation_performance.last = Some(
        crate::r#loop::agent_tui::runtime_bridge::GenerationPerformanceSnapshot {
            ttft_total_ms: 8_000,
            ttft_samples: 1,
            decode_total_ms: 1_000,
            generation_tokens: 40,
        },
    );
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(AgentTuiToolReceipt::with_id(
        "tool-part:test:performance",
        "provider-call:test:performance",
        "shell_command",
        crate::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "命令完成".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));

    state.finish_submit(Ok(output));

    let Some(RenderCellData::TurnSummary(summary)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("完成回合应生成 usage 分割线");
    };
    assert_eq!(summary.first_token_avg_ms, Some(8_000));
    assert_eq!(summary.generation_tps_x10, Some(400));
}

#[test]
fn finish_submit_updates_live_tool_cell_without_duplicate_receipt_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("读取文件"));
    let receipt = AgentTuiToolReceipt::with_id(
        "tool-part:test:dedupe",
        "provider-call:test:dedupe",
        "builtin_read_file",
        crate::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "读取完成".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    );
    state.apply_tool_execution_started(AgentTuiToolExecutionView {
        request_id: 1,
        part_id: receipt.id.clone(),
        provider_call_id: receipt.provider_call_id.clone(),
        tool_name: receipt.tool_name.clone(),
        kind: "read".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        arguments_preview: "Cargo.toml".to_owned(),
        intent: None,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    });
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(receipt));

    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    assert_eq!(cell_count(&state, RenderCellKind::TURN_SUMMARY), 1);
}

#[test]
fn finish_submit_clears_active_runtime_status() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("检查运行状态"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            crate::r#loop::agent_tui::run_output::AgentTuiHookStatusView {
                run_id: "hook-1".to_owned(),
                name: "PreToolUse".to_owned(),
                status: crate::r#loop::agent_tui::run_output::AgentTuiHookStatus::Running,
                status_message: Some("检查权限".to_owned()),
                entries: Vec::new(),
            },
        ),
    );
    assert!(state.is_working());

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert!(!state.is_working());
    assert_eq!(active_hook_item_count(&state), 0);
}

#[test]
fn finish_after_streaming_reply_drops_provider_model_lifecycle_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("你是谁？"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "StreamingStarted",
                "cpa/gpt-5.4",
                "streaming",
                Some("cpa/gpt-5.4".to_owned()),
                None,
                Some(1),
            ),
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "我是 Axon。".to_owned(),
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::GenerationStatus(
        AgentTuiLifecycleLine::new(
            "生成",
            vec![AgentTuiLifecycleStatusView::new(
                "生成",
                "StreamingStarted",
                "cpa/gpt-5.4",
                "streaming",
                Some("cpa/gpt-5.4".to_owned()),
                None,
                Some(1),
            )],
        ),
    ));
    state.finish_submit(Ok(output));

    assert!(!state.is_working());
    assert_eq!(typed_message_source(&state).as_deref(), Some("我是 Axon。"));
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
    assert!(
        !state
            .cells.transcript
            .iter()
            .any(|line| line.text.contains("cpa/gpt-5.4") || line.text.contains("StreamingStarted"))
    );
}

#[test]
fn finish_after_skill_hook_keeps_single_streaming_reply_and_history_status() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("分析图片"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "这张图显示的是表格。".to_owned(),
        ),
    );
    assert!(state.tick(Instant::now() + Duration::from_secs(1)));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::hook_status_event(
            AgentTuiHookStatusView::with_run_id(
                "skill-run",
                "Skill",
                "completed",
                Some("已增强 draft-image-1-axon-clipboard-image.png".to_owned()),
            ),
        ),
    );
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Message(AgentTuiMessagePart::assistant(
        "assistant:default",
        "这张图显示的是表格。",
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::MESSAGE), 1);
    assert_eq!(
        typed_message_source(&state).as_deref(),
        Some("这张图显示的是表格。")
    );
    assert!(state.cells.transcript.iter().any(|line| matches!(
        &line.data,
        Some(RenderCellData::History(cell))
            if cell.items.iter().any(|item|
                item.name == "已增强 draft-image-1-axon-clipboard-image.png"
                    && item.kind_label.as_deref() == Some("Skill")
            )
    )));
}

#[test]
fn interrupt_preserves_streamed_agent_output_and_clears_runtime_state() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("中断保留输出"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "已经输出的回复".to_owned(),
        ),
    );
    state.request_interrupt_current_task();

    assert_eq!(
        typed_message_source(&state).as_deref(),
        Some("已经输出的回复")
    );
    assert!(active_message_source(&state).is_none());
    assert!(!state.is_working());
}

#[test]
fn failed_submit_clears_active_status_cell_without_ready_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("验证失败状态"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "StreamingStarted",
                "gpt-5",
                "streaming",
                Some("gpt-5".to_owned()),
                None,
                Some(1),
            ),
        ),
    );

    state.finish_submit(Err(crate::RuntimeError::runtime("provider failed")));

    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
}

#[test]
fn final_lifecycle_output_does_not_leave_live_view_after_finish() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("完成生命周期状态"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::GenerationStatus(
        AgentTuiLifecycleLine::new(
            "生成",
            vec![AgentTuiLifecycleStatusView::new(
                "生成",
                "Generation",
                "生成回复",
                "generation",
                None,
                None,
                None,
            )],
        ),
    ));
    output.push(AgentTuiRunLine::Status(AgentTuiStatusLine::new(
        "运行",
        vec![AgentTuiStatusItem::new(
            "LifecycleStatus",
            "收尾",
            None,
            Some("notice".to_owned()),
            None,
            None,
        )],
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::STATUS),
        0
    );
}

#[test]
fn lifecycle_status_event_projects_dedicated_generation_status_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("等待模型"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "Generation",
                "生成回复",
                "generation",
                None,
                None,
                None,
            ),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        1
    );
    assert!(lifecycle_cell_has_phase(
        &state,
        RenderCellKind::GENERATION_STATUS,
        "generation"
    ));
}

#[path = "runtime_flow_generation_status_tests.rs"]
mod generation_status_tests;
#[path = "runtime_flow_lifecycle_policy_tests.rs"]
mod lifecycle_policy_tests;
#[path = "runtime_flow_snapshot_reconciliation_tests.rs"]
mod snapshot_reconciliation_tests;
