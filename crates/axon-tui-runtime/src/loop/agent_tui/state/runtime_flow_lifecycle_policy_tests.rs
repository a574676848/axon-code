//! Runtime lifecycle 分类与噪声控制测试。

use super::*;

#[test]
fn unknown_lifecycle_status_does_not_create_status_noise() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("未知状态"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "状态",
                "UnknownRuntimeEvent",
                "machine detail",
                "unknown",
                None,
                None,
                None,
            ),
        ),
    );
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::STATUS),
        0
    );
}

#[test]
fn notice_lifecycle_status_keeps_explicit_status_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("明确提示"));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
            AgentTuiLifecycleStatusView::new(
                "状态",
                "Notice",
                "权限检查",
                "notice",
                None,
                None,
                None,
            ),
        ),
    );
    assert_eq!(
        active_lifecycle_cell_count(&state, RenderCellKind::STATUS),
        1
    );
}

#[test]
fn compaction_lifecycle_status_projects_dedicated_history_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("压缩上下文"));

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
    assert_eq!(cell_count(&state, RenderCellKind::HISTORY), 1);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(&line.data, Some(RenderCellData::History(cell))
            if cell.items.first().is_some_and(|item|
                item.name == "压缩上下文"
                    && item.kind_label.is_none()
                    && item.status_label.as_deref() == Some("进行中")))
    }));
}
