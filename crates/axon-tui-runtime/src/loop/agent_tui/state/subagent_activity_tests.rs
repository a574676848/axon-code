//! 子 Agent 活动区状态投影测试。

use std::time::{Duration, Instant};

use axon_protocol::wire::{
    WireEventKind, WireLifecyclePhase, WireLifecycleStatus, WireTurnLifecycle,
};

use super::super::run_output::AgentTuiRuntimePayload;
use super::runtime_flow_test_support::{cell_count, test_state};
use crate::surface::cell::{RenderCellData, RenderCellKind};
use crate::surface::subagent_activity::SubagentActivityStatus;

#[test]
fn subagent_lifecycle_projects_activity_without_transcript_status_noise() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "started",
        WireLifecycleStatus::Running,
        "/root/review 已启动：审查当前改动",
        Some(1_700_000_000_000),
        None,
        None,
    ));

    let snapshot = state.subagent_activity_snapshot(Instant::now());
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].label, "审查当前改动");
    assert_eq!(snapshot.rows[0].activity, "启动中");
    assert_eq!(snapshot.rows[0].usage, None);
    assert_eq!(cell_count(&state, RenderCellKind::STATUS), 0);
}

#[test]
fn running_subagent_elapsed_keeps_advancing_after_lifecycle_update() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/review 正在生成回复",
        None,
        Some(1_000),
        None,
    ));

    let now = Instant::now();
    let initial = state.subagent_activity_snapshot(now).rows[0].elapsed;
    let advanced = state
        .subagent_activity_snapshot(now + Duration::from_secs(2))
        .rows[0]
        .elapsed;

    assert!(advanced >= initial + Duration::from_secs(2));
}

#[test]
fn subagent_progress_without_usage_keeps_latest_real_token_total() {
    let mut state = test_state();
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/review 正在生成回复",
        None,
        Some(1_000),
        Some(12_345),
    ));
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "tool_running",
        WireLifecycleStatus::Running,
        "/root/review 正在执行工具 builtin.search",
        None,
        Some(1_500),
        None,
    ));

    let row = state.subagent_activity_snapshot(Instant::now()).rows[0].clone();
    assert_eq!(row.activity, "执行工具");
    assert_eq!(row.usage, Some(12_345));
}

#[test]
fn parent_wait_projection_does_not_replace_provider_phase_or_tokens() {
    let mut state = test_state();
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_connecting",
        WireLifecycleStatus::Running,
        "/root/review 正在连接供应商",
        None,
        Some(1_000),
        Some(19_600),
    ));
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "waiting",
        WireLifecycleStatus::Waiting,
        "/root/review 等待子代理结果，timeout_ms=1000",
        None,
        None,
        None,
    ));

    let row = state.subagent_activity_snapshot(Instant::now()).rows[0].clone();
    assert_eq!(row.activity, "连接供应商");
    assert_eq!(row.status, SubagentActivityStatus::Running);
    assert_eq!(row.usage, Some(19_600));
}

#[test]
fn fast_delegate_label_includes_role_and_normal_subagent_keeps_task_summary() {
    let mut state = test_state();
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/delegate_fast_1",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/delegate_fast_1 已启动：检查编译结果",
        None,
        None,
        None,
    ));
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/analysis_2",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/analysis_2 已启动：分析根因",
        None,
        None,
        None,
    ));

    let rows = state.subagent_activity_snapshot(Instant::now()).rows;
    let fast = rows
        .iter()
        .find(|row| row.agent_id.contains("delegate_fast_1"))
        .expect("Fast 子代理应投影");
    let analysis = rows
        .iter()
        .find(|row| row.agent_id.contains("analysis_2"))
        .expect("普通子代理应投影");
    assert_eq!(fast.label, "Fast · 检查编译结果");
    assert_eq!(analysis.label, "分析根因");
}

#[test]
fn subagent_completed_is_removed_immediately() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/review 正在生成回复",
        Some(1_700_000_000_000),
        Some(64_000),
        None,
    ));
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "completed",
        WireLifecycleStatus::Completed,
        "/root/review 已完成：审查完成",
        Some(1_700_000_000_000),
        Some(65_000),
        Some(12_345),
    ));

    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );
}

#[test]
fn subagent_completed_record_uses_task_first_and_shared_display_units() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/review 正在生成回复",
        Some(1_700_000_000_000),
        Some(10_000),
        Some(1_000),
    ));
    state.apply_runtime_event(with_cache_hit_rate(
        subagent_lifecycle(
            "part:/root/review",
            "completed",
            WireLifecycleStatus::Completed,
            "/root/review 已完成：审查完成",
            Some(1_700_000_000_000),
            Some(12_300),
            Some(2_480),
        ),
        9_000,
    ));

    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );
    let result = state
        .cells
        .transcript
        .iter()
        .find_map(|record| match &record.data {
            Some(RenderCellData::Command(cell)) if cell.title == "代码审查子代理完成" => {
                Some(cell)
            }
            _ => None,
        })
        .expect("应追加子代理完成记录");
    assert_eq!(
        result.items.first().map(|item| item.summary.as_str()),
        Some("耗时：12.3s，Token 消耗：2.48K，缓存命中率：90.00%，摘要：审查完成")
    );
}

#[test]
fn subagent_completed_record_projects_summary_artifact_path_and_tokens() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "completed",
        WireLifecycleStatus::Completed,
        "/root/review 已完成：审查完成：发现 2 处潜在漏洞\n结果文件：E:\\axon\\data\\subagents\\results\\review.md",
        Some(1_700_000_000_000),
        Some(4_500),
        Some(3_250),
    ));

    let result = state
        .cells
        .transcript
        .iter()
        .find_map(|record| match &record.data {
            Some(RenderCellData::Command(cell)) if cell.title == "代码审查子代理完成" => {
                Some(cell)
            }
            _ => None,
        })
        .expect("应追加子代理完成记录");
    assert_eq!(
        result.items.first().map(|item| item.summary.as_str()),
        Some(
            "耗时：4.5s，Token 消耗：3.25K，摘要：审查完成：发现 2 处潜在漏洞\n结果文件：E:\\axon\\data\\subagents\\results\\review.md"
        )
    );
}

#[test]
fn subagent_completed_record_omits_zero_token_usage() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "completed",
        WireLifecycleStatus::Completed,
        "/root/review 已完成：审查完成",
        Some(1_700_000_000_000),
        Some(800),
        Some(0),
    ));

    let summary = state
        .cells
        .transcript
        .iter()
        .find_map(|record| match &record.data {
            Some(RenderCellData::Command(cell)) => cell.items.first(),
            _ => None,
        })
        .map(|item| item.summary.as_str());
    assert_eq!(summary, Some("耗时：800ms，摘要：审查完成"));
}

#[test]
fn subagent_waiting_keeps_child_running() {
    for phase in ["waiting_begin", "waiting"] {
        let mut state = test_state();

        state.apply_runtime_event(subagent_lifecycle(
            "part:/root/review",
            phase,
            WireLifecycleStatus::Waiting,
            "/root/review 等待父代理接收",
            Some(1_700_000_000_000),
            None,
            None,
        ));

        let row = state.subagent_activity_snapshot(Instant::now()).rows[0].clone();
        assert_eq!(row.status, SubagentActivityStatus::Running);
        assert_eq!(row.activity, "父代理等待结果");
    }
}

#[test]
fn subagent_wait_elapsed_remains_running() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "wait_elapsed",
        WireLifecycleStatus::Running,
        "/root/review 本次等待已结束，子代理仍在运行",
        Some(1_700_000_000_000),
        Some(30_000),
        None,
    ));

    let now = Instant::now();
    let snapshot = state.subagent_activity_snapshot(now);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].status, SubagentActivityStatus::Running);
    assert!(snapshot.rows[0].elapsed >= Duration::from_secs(30));
    assert_eq!(snapshot.rows[0].usage, None);

    let prune_at = now + Duration::from_millis(4_001);
    state.tick(prune_at);
    assert_eq!(
        state.subagent_activity_snapshot(prune_at).rows.len(),
        1,
        "运行中的子代理不会因 tick 被清理"
    );
}

#[test]
fn clear_hides_subagent_activity_and_ignores_late_events_until_next_submit() {
    let mut state = test_state();
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_generating",
        WireLifecycleStatus::Running,
        "/root/review 正在生成回复",
        None,
        None,
        None,
    ));
    assert_eq!(
        state.subagent_activity_snapshot(Instant::now()).rows.len(),
        1
    );

    state.clear();
    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "interrupted",
        WireLifecycleStatus::Cancelled,
        "/root/review 已中断",
        None,
        None,
        None,
    ));
    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );

    state.replace_draft("下一轮任务".to_owned());
    let submission = state.begin_submit().expect("下一轮任务应开始提交");
    drop(submission);
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/next",
        "started",
        WireLifecycleStatus::Running,
        "/root/next 已启动",
        None,
        None,
        None,
    ));
    assert_eq!(
        state.subagent_activity_snapshot(Instant::now()).rows.len(),
        1
    );
}

#[test]
fn interrupt_hides_running_subagent_activity_and_ignores_late_events() {
    let mut state = test_state();
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "provider_connecting",
        WireLifecycleStatus::Running,
        "/root/review 正在连接供应商",
        None,
        None,
        None,
    ));
    assert_eq!(
        state.subagent_activity_snapshot(Instant::now()).rows.len(),
        1
    );

    state.request_interrupt_current_task();
    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );

    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/review",
        "interrupted",
        WireLifecycleStatus::Cancelled,
        "/root/review 已中断",
        None,
        None,
        None,
    ));
    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );
}

#[test]
fn canonical_task_path_in_title_projects_semantic_label_without_summary() {
    let mut state = test_state();

    state.apply_runtime_event(subagent_lifecycle_with_title(SubagentLifecycleInput {
        part_id: "part:/root/review",
        ui_phase: "provider_generating",
        status: WireLifecycleStatus::Running,
        summary: None,
        started_at_ms: Some(1_700_000_000_000),
        duration_ms: None,
        total_tokens: None,
        title: "/root/review",
    }));

    let snapshot = state.subagent_activity_snapshot(Instant::now());
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].label, "代码审查");
}

#[test]
fn terminal_events_do_not_create_activity_rows() {
    let mut state = test_state();
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/delegate_fast_1",
        "completed",
        WireLifecycleStatus::Completed,
        "/root/delegate_fast_1 已完成",
        None,
        Some(2_000),
        None,
    ));
    state.apply_runtime_event(subagent_lifecycle(
        "part:/root/delegate_fast_2",
        "errored",
        WireLifecycleStatus::Failed,
        "/root/delegate_fast_2 执行失败",
        None,
        Some(5_000),
        None,
    ));

    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );
}

#[test]
fn new_execution_reopens_after_previous_completion() {
    let mut state = test_state();

    state.apply_runtime_event(with_run_id(
        subagent_lifecycle(
            "part:/root/review",
            "started",
            WireLifecycleStatus::Started,
            "/root/review 已启动：第一轮审查",
            None,
            None,
            None,
        ),
        "run-first",
    ));
    state.apply_runtime_event(with_run_id(
        subagent_lifecycle(
            "part:/root/review",
            "completed",
            WireLifecycleStatus::Completed,
            "/root/review 已完成",
            None,
            Some(5_000),
            Some(120),
        ),
        "run-first",
    ));
    assert!(
        state
            .subagent_activity_snapshot(Instant::now())
            .rows
            .is_empty()
    );

    state.apply_runtime_event(with_run_id(
        subagent_lifecycle(
            "part:/root/review",
            "started",
            WireLifecycleStatus::Started,
            "/root/review 已启动：第二轮审查",
            None,
            None,
            None,
        ),
        "run-second",
    ));

    let restarted = state.subagent_activity_snapshot(Instant::now()).rows[0].clone();
    assert_eq!(restarted.status, SubagentActivityStatus::Starting);
    assert_eq!(restarted.usage, None);
    assert_eq!(restarted.label, "第二轮审查");
}

fn subagent_lifecycle(
    part_id: &str,
    ui_phase: &str,
    status: WireLifecycleStatus,
    summary: &str,
    started_at_ms: Option<u64>,
    duration_ms: Option<u64>,
    total_tokens: Option<u64>,
) -> crate::AgentTuiRuntimeEvent {
    subagent_lifecycle_with_title(SubagentLifecycleInput {
        part_id,
        ui_phase,
        status,
        summary: Some(summary),
        started_at_ms,
        duration_ms,
        total_tokens,
        title: "子代理",
    })
}

struct SubagentLifecycleInput<'a> {
    part_id: &'a str,
    ui_phase: &'a str,
    status: WireLifecycleStatus,
    summary: Option<&'a str>,
    started_at_ms: Option<u64>,
    duration_ms: Option<u64>,
    total_tokens: Option<u64>,
    title: &'a str,
}

fn subagent_lifecycle_with_title(input: SubagentLifecycleInput<'_>) -> crate::AgentTuiRuntimeEvent {
    crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::TurnLifecycle(
        WireTurnLifecycle {
            part_id: input.part_id.to_owned(),
            turn_id: "subagent_activity".to_owned(),
            phase: WireLifecyclePhase::RegularTurn,
            status: input.status,
            title: input.title.to_owned(),
            kind: Some("subagent_activity".to_owned()),
            ui_phase: Some(input.ui_phase.to_owned()),
            tool_name: Some("gpt-test".to_owned()),
            attempt_index: None,
            summary: input.summary.map(str::to_owned),
            duration_ms: input.duration_ms,
            started_at_ms: input.started_at_ms,
            total_tokens: input.total_tokens,
            cache_hit_rate_basis_points: None,

            warnings: Vec::new(),
        },
    ))
}

fn with_run_id(
    mut event: crate::AgentTuiRuntimeEvent,
    run_id: &str,
) -> crate::AgentTuiRuntimeEvent {
    let AgentTuiRuntimePayload::WireEvent { event: wire, .. } = &mut event.payload else {
        panic!("测试事件必须是 wire event");
    };
    wire.session.run_id = Some(run_id.to_owned());
    event
}

fn with_cache_hit_rate(
    mut event: crate::AgentTuiRuntimeEvent,
    cache_hit_rate_basis_points: u16,
) -> crate::AgentTuiRuntimeEvent {
    let AgentTuiRuntimePayload::WireEvent { event: wire, .. } = &mut event.payload else {
        panic!("测试事件必须是 wire event");
    };
    let WireEventKind::TurnLifecycle(lifecycle) = &mut wire.kind else {
        panic!("测试事件必须是 lifecycle event");
    };
    lifecycle.cache_hit_rate_basis_points = Some(cache_hit_rate_basis_points);
    event
}
