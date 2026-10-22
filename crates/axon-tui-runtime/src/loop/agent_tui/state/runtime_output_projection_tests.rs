//! 运行输出投射与收尾行为测试。

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::run_output::{
    AgentTuiHistoryItem, AgentTuiHistoryLine, AgentTuiLifecycleStatusView, AgentTuiRunLine,
    AgentTuiRunOutput, AgentTuiStatusItem, AgentTuiStatusLine, AgentTuiToolCompletionView,
    AgentTuiToolExecutionView, AgentTuiToolKind, AgentTuiToolReceipt,
};
use crate::surface::{
    cell::{RenderCellData, RenderCellKind},
    error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity},
    runtime_adapter::RuntimeAdapterSchemaVersion,
};
use axon_domain::ToolExecutionPermission;
use axon_protocol::wire::{WireEventKind, WirePlanStep, WirePlanUpdate};

#[test]
fn finish_submit_clears_all_active_lifecycle_cells() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("完成运行期状态"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new("设置", "Model", "gpt-5", "command", None, None, None),
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "运行",
                "CompactionStarted",
                "压缩上下文",
                "notice",
                None,
                None,
                None,
            ),
        ),
    );
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
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(cell_count(&state, RenderCellKind::COMMAND), 0);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::GENERATION_STATUS),
        0
    );
}

#[test]
fn finish_without_runtime_status_does_not_create_ready_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("无事件完成"));

    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(cell_count(&state, RenderCellKind::GENERATION_STATUS), 0);
}

#[test]
fn final_status_output_does_not_leave_duplicate_ready_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("完成运行状态"));
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
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Status(AgentTuiStatusLine::new(
        "就绪",
        vec![AgentTuiStatusItem::new(
            "Ready",
            "1s",
            None,
            Some("ready".to_owned()),
            None,
            None,
        )],
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
}

#[test]
fn failed_submit_persists_provider_retry_without_streaming_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("请求模型"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "StreamingStarted",
                "生成回复",
                "streaming",
                None,
                None,
                None,
            ),
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "生成",
                "ProviderRetry",
                "连接失败，正在切换渠道",
                "streaming",
                Some("gpt-5.4".to_owned()),
                None,
                Some(2),
            ),
        ),
    );
    state.finish_submit(Err(crate::RuntimeError::runtime("provider retry failed")));

    let generation = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::GenerationStatus(cell)) => Some(cell),
            _ => None,
        });
    let generation = generation.expect("retry generation status should persist");
    assert_eq!(generation.items.len(), 1);
    assert_eq!(generation.items[0].kind, "ProviderRetry");
    assert_eq!(generation.items[0].attempt_index, Some(2));
    assert_eq!(generation.items[0].tool_name.as_deref(), Some("gpt-5.4"));
}

#[test]
fn failed_submit_without_runtime_output_keeps_transcript_quiet() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("修复渲染区重复提示"));

    state.finish_submit(Err(crate::RuntimeError::runtime("provider failed")));

    assert_eq!(cell_count(&state, RenderCellKind::REVIEW), 0);
    assert!(
        !state
            .cells
            .transcript
            .iter()
            .any(|line| line.text.contains("上一轮任务失败"))
    );
}

#[test]
fn runtime_schema_mismatch_projects_inline_error_cell() {
    let mut state = test_state();
    state.replace_draft("不应提交到旧 adapter".to_owned());
    let error = crate::RuntimeError::schema_mismatch(
        RuntimeAdapterSchemaVersion::new(1, 0),
        RuntimeAdapterSchemaVersion::new(2, 0),
    );

    state.push_runtime_error(error);

    let error_cell = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Error(cell)) => Some(cell),
            _ => None,
        })
        .expect("schema mismatch 应投影为 inline error cell");
    assert_eq!(error_cell.title, "RuntimeAdapter 版本不匹配");
    assert_eq!(
        error_cell.message,
        "当前 runtime adapter 与 TUI 契约版本不一致。"
    );
    assert_eq!(
        error_cell.next_step.as_deref(),
        Some("升级 Axon runtime adapter 后重新启动。")
    );
    assert_eq!(error_cell.severity, TuiErrorSeverity::UserFacing);
    assert_eq!(error_cell.action, TuiErrorAction::Reconfigure);
    assert_eq!(error_cell.cell_kind, TuiErrorCellKind::Runtime);
    assert!(!state.should_exit());
    assert!(!state.is_working());
    assert!(state.begin_submit().is_none());
    assert_eq!(state.composer.draft.text(), "不应提交到旧 adapter");
}

#[test]
fn runtime_schema_minor_mismatch_is_not_treated_as_compatible() {
    let actual = crate::RuntimeAdapterContract::new(
        RuntimeAdapterSchemaVersion::new(1, 1),
        crate::RuntimeAdapterCapabilities::AXON_CLI,
    );

    let error = actual.validate().expect_err("minor 漂移也必须显式失败");
    let details = error.to_tui_error().details;

    assert_eq!(error.error_code(), Some("runtime_adapter_schema_mismatch"));
    assert_eq!(
        error.message(),
        "当前 runtime adapter 与 TUI 契约版本不一致。"
    );
    assert!(details.iter().any(|detail| detail == "期望 1.0"));
    assert!(details.iter().any(|detail| detail == "实际 1.1"));
}

#[test]
fn structured_output_suppresses_duplicate_auto_summary() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(tool_receipt(
        "call-1",
        "shell_command",
        crate::r#loop::agent_tui::run_output::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "ok".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::REVIEW), 0);
    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
}

#[test]
fn final_run_output_receipt_also_populates_tool_detail_buffer() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行最终回执"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(tool_receipt(
        "call-final-buffer",
        "shell_command",
        crate::r#loop::agent_tui::run_output::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "最终回执真实结果".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
        },
    )));

    state.finish_submit(Ok(output));

    let buffered = state
        .cells
        .tool_outputs_by_part_id
        .get("tool-part:test:runtime-output-receipt")
        .expect("最终 run output receipt 必须进入详情缓冲")
        .view()
        .lines
        .join("\n");
    assert!(buffered.contains("最终回执真实结果"));
}

#[test]
fn final_usage_updates_header_bar_state_without_transcript_cell_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("统计 token"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Usage(
        crate::r#loop::agent_tui::run_output::AgentTuiUsageLine::with_id(
            "usage:test:final",
            crate::r#loop::agent_tui::run_output::AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0),
        ),
    ));

    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::USAGE), 0);
    assert_eq!(state.last_turn_usage().total_tokens(), 5_113);
    assert_eq!(state.session_usage().total_tokens(), 5_113);
}

#[test]
fn runtime_usage_event_updates_header_bar_state_and_finish_does_not_double_count() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("统计 token"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::usage_update_event(
            crate::r#loop::agent_tui::run_output::AgentTuiTokenUsage::new(500, 9_500, 200, 200, 0)
                .with_current_context_tokens(10_200),
        ),
    );
    assert_eq!(state.last_turn_usage().current_context_tokens(), 10_200);
    assert_eq!(state.last_turn_usage().cache_read_input_tokens, 9_500);

    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Usage(
        crate::r#loop::agent_tui::run_output::AgentTuiUsageLine::with_id(
            "usage:test:runtime-event",
            crate::r#loop::agent_tui::run_output::AgentTuiTokenUsage::new(500, 9_500, 200, 200, 0)
                .with_current_context_tokens(10_200),
        ),
    ));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::USAGE), 0);
    assert_eq!(state.session_usage().total_tokens(), 10_400);
    assert_eq!(state.session_usage().current_context_tokens(), 10_200);
}
#[test]
fn missing_usage_does_not_clear_last_header_bar_usage() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("统计 token"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Usage(
        crate::r#loop::agent_tui::run_output::AgentTuiUsageLine::with_id(
            "usage:test:missing-next-turn",
            crate::r#loop::agent_tui::run_output::AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0),
        ),
    ));
    state.finish_submit(Ok(output));

    state.start_runtime_submit(&test_submission("无 usage 响应"));
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    assert_eq!(state.last_turn_usage().total_tokens(), 5_113);
    assert_eq!(state.session_usage().total_tokens(), 5_113);
}

#[test]
fn final_mcp_receipt_projects_typed_mcp_cell_without_exec_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("调用 MCP"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::ToolReceipt(tool_receipt(
        "call-mcp",
        "mcp__gitnexus__query",
        crate::r#loop::agent_tui::run_output::AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Mcp,
            status: "completed".to_owned(),
            summary: "query ok".to_owned(),
            metadata: Some(serde_json::json!({
                "server": "gitnexus",
                "tool": "query"
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 0);
    assert_eq!(cell_count(&state, RenderCellKind::MCP), 1);
    assert_eq!(
        typed_mcp_first_target(&state).as_deref(),
        Some("gitnexus/query")
    );
}

#[test]
fn wire_exec_receipt_transfers_real_result_into_detail_buffer_and_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    let part_id = "tool-part:test:wire-exec-detail";
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                request_id: 1,
                part_id: part_id.to_owned(),
                provider_call_id: "call-exec-detail".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "printf result".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: part_id.to_owned(),
                provider_call_id: "call-exec-detail".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                status: "completed".to_owned(),
                summary: "真实命令结果\n第二行结果".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
                duration_ms: None,
            },
        ),
    );

    let buffered = state
        .cells
        .tool_outputs_by_part_id
        .get(part_id)
        .expect("最终 Exec receipt 必须进入详情缓冲")
        .view()
        .lines
        .join("\n");
    assert!(buffered.contains("真实命令结果"));
    let index = state
        .cells
        .transcript
        .iter()
        .position(|line| line.semantic_part_id().as_deref() == Some(part_id))
        .expect("Exec cell 必须保留稳定 part_id");
    assert!(state.hydrate_tool_detail_at(index, part_id));
    assert!(matches!(
        state.cells.transcript[index].data.as_ref(),
        Some(RenderCellData::Exec(cell)) if cell
            .output_preview
            .as_deref()
            .is_some_and(|output| output.contains("真实命令结果"))
    ));
}

#[test]
fn wire_mcp_receipt_transfers_sanitized_output_preview_into_buffer_and_mcp_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("调用 MCP"));
    let part_id = "tool-part:test:wire-mcp-detail";
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                request_id: 1,
                part_id: part_id.to_owned(),
                provider_call_id: "call-mcp-detail".to_owned(),
                tool_name: "mcp__gitnexus__query".to_owned(),
                kind: "mcp".to_owned(),
                semantic_kind: AgentTuiToolKind::Mcp,
                arguments_preview: "{\"q\":\"auth\"}".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: part_id.to_owned(),
                provider_call_id: "call-mcp-detail".to_owned(),
                tool_name: "mcp__gitnexus__query".to_owned(),
                kind: "mcp".to_owned(),
                semantic_kind: AgentTuiToolKind::Mcp,
                status: "completed".to_owned(),
                summary: "MCP 已完成".to_owned(),
                metadata: Some(serde_json::json!({
                    "server": "gitnexus",
                    "tool": "query",
                    "outputPreview": "真实 MCP 结果"
                })),
                error_type: None,
                filesystem_write_performed: false,
                duration_ms: None,
            },
        ),
    );

    let buffered = state
        .cells
        .tool_outputs_by_part_id
        .get(part_id)
        .expect("最终 MCP receipt 必须进入详情缓冲")
        .view()
        .lines
        .join("\n");
    assert!(buffered.contains("真实 MCP 结果"));
    assert_eq!(
        typed_mcp_first_output(&state).as_deref(),
        Some("真实 MCP 结果")
    );
}

#[test]
fn tool_detail_buffer_is_reclaimed_when_transcript_tool_cell_is_removed() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("回收工具详情"));
    let part_id = "tool-part:test:reclaim-detail";
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                request_id: 1,
                part_id: part_id.to_owned(),
                provider_call_id: "call-reclaim-detail".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "printf detail".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: part_id.to_owned(),
                provider_call_id: "call-reclaim-detail".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                status: "completed".to_owned(),
                summary: "应被回收的结果".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
                duration_ms: None,
            },
        ),
    );
    assert!(state.cells.tool_outputs_by_part_id.contains_key(part_id));

    let index = state
        .cells
        .transcript
        .iter()
        .position(|line| line.semantic_part_id().as_deref() == Some(part_id))
        .expect("工具 cell 应存在");
    let _ = state.cells.transcript.remove(index);
    state
        .cells
        .active_runtime_cells
        .remove_transcript_index(index);

    assert_eq!(state.prune_tool_output_buffers(), 1);
    assert!(!state.cells.tool_outputs_by_part_id.contains_key(part_id));
}

#[test]
fn interrupt_settles_live_plan_cell_into_transcript() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("中断时保留计划"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::PlanUpdate(
            WirePlanUpdate {
                part_id: "plan-part:interrupt".to_owned(),
                title: "中断计划".to_owned(),
                explanation: Some("保留已展示的计划".to_owned()),
                steps: vec![WirePlanStep {
                    step: "完成收敛".to_owned(),
                    status: "running".to_owned(),
                    cycle: None,
                    diagnostic_count: 0,
                    failing_step_count: 0,
                }],
            },
        )),
    );

    assert_eq!(cell_count(&state, RenderCellKind::PLAN), 0);
    assert!(
        state
            .active_transcript_lines()
            .iter()
            .any(|line| { matches!(&line.data, Some(RenderCellData::Plan(_))) })
    );

    state.request_interrupt_current_task();

    let plan = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Plan(plan)) => Some(plan),
            _ => None,
        })
        .expect("中断后计划 cell 应保留在 transcript");
    assert_eq!(plan.id, "plan-part:interrupt");
    assert_eq!(
        plan.source_kind,
        crate::surface::cell_plan_data::AgentRenderPlanSourceKind::Markdown
    );
    assert!(plan.source.contains("完成收敛"));
}

#[test]
fn header_plan_progress_follows_plan_cell_lifecycle() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("跟踪计划生命周期"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::PlanUpdate(
            WirePlanUpdate {
                part_id: "plan-part:lifecycle".to_owned(),
                title: "生命周期计划".to_owned(),
                explanation: None,
                steps: vec![WirePlanStep {
                    step: "运行中".to_owned(),
                    status: "running".to_owned(),
                    cycle: None,
                    diagnostic_count: 0,
                    failing_step_count: 0,
                }],
            },
        )),
    );

    let progress = state
        .plan_progress()
        .expect("活跃 Plan cell 应驱动计划指标");
    assert_eq!(progress.completed, 0);
    assert_eq!(progress.total, 1);

    state.clear_active_transcript_lines();
    assert!(state.plan_progress().is_none());
}

fn tool_receipt(
    provider_call_id: &str,
    tool_name: &str,
    details: crate::r#loop::agent_tui::run_output::AgentTuiToolReceiptDetails,
) -> AgentTuiToolReceipt {
    AgentTuiToolReceipt::with_id(
        "tool-part:test:runtime-output-receipt",
        provider_call_id,
        tool_name,
        details,
    )
}

#[test]
fn final_history_output_projects_typed_history_cell_without_status_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("压缩上下文"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::History(AgentTuiHistoryLine::new(
        "历史",
        vec![AgentTuiHistoryItem::new(
            "保留关键事实",
            Some("上下文".to_owned()),
            Some("完成".to_owned()),
        )],
    )));
    state.finish_submit(Ok(output));

    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(&line.data, Some(RenderCellData::History(cell))
            if cell.items.first().is_some_and(|item|
                item.name == "保留关键事实"
                    && item.kind_label.as_deref() == Some("上下文")
                    && item.status_label.as_deref() == Some("完成")))
    }));
}

#[test]
fn session_run_line_stays_out_of_transcript_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("恢复会话"));
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Session(
        crate::AgentTuiRunSessionInfo::new("run-42", Some("run-41".to_owned())),
    ));

    state.finish_submit(Ok(output));

    assert!(
        !state
            .cells
            .transcript
            .iter()
            .any(|line| line.text.contains("Session") || line.text.contains("run-42"))
    );
}
