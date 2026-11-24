//! Exec cell 运行期 tick 行为测试。

use axon_domain::ToolExecutionPermission;
use std::time::{Duration, Instant};

use super::runtime_flow_test_support::{
    cell_count, test_state, test_submission, typed_exec_first_duration_ms,
};
use crate::r#loop::agent_tui::run_output::{AgentTuiToolExecutionView, AgentTuiToolKind};
use crate::surface::cell::RenderCellKind;

#[test]
fn active_exec_cell_duration_updates_on_runtime_tick() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行命令"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:call-1".to_owned(),
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
    assert_eq!(typed_exec_first_duration_ms(&state), None);

    assert!(state.tick(Instant::now() + Duration::from_secs(1)));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    assert!(typed_exec_first_duration_ms(&state).is_some());
}

#[test]
fn completed_exec_cell_does_not_update_duration_on_runtime_tick() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("编辑文件"));
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: "tool-part:test:call-edit".to_owned(),
                request_id: 1,
                provider_call_id: "call-edit".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                arguments_preview: "scan_cfg.py".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );

    // 第一次 tick，开始计时
    state.tick(Instant::now() + Duration::from_secs(1));
    assert!(typed_exec_first_duration_ms(&state).is_some());

    // 工具完成并结算（打勾）
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            crate::r#loop::agent_tui::run_output::AgentTuiToolCompletionView {
                part_id: "tool-part:test:call-edit".to_owned(),
                provider_call_id: "call-edit".to_owned(),
                tool_name: "builtin.edit".to_owned(),
                kind: "edit".to_owned(),
                semantic_kind: AgentTuiToolKind::Edit,
                status: "completed".to_owned(),
                summary: "edit ok".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: true,
                duration_ms: Some(500),
            },
        ),
    );

    assert_eq!(typed_exec_first_duration_ms(&state), Some(500));

    // 随后时间流逝多次 tick（模拟过了 2 分钟）
    state.tick(Instant::now() + Duration::from_secs(120));

    // 验证：耗时必须固定在 500ms，绝对不能跟随系统时间继续跑动！
    assert_eq!(typed_exec_first_duration_ms(&state), Some(500));
}
