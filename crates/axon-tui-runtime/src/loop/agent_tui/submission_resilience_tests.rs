use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use axon_agent_runtime::ProjectionRuntimeBudget;
use axon_protocol::wire::{
    WireCorrelationId, WireEventKind, WireLifecyclePhase, WireLifecycleStatus, WireTurnLifecycle,
};

use super::submission_worker_mailbox::WorkerMailbox;
use super::test_support::*;
use super::*;
use crate::r#loop::agent_tui::run_output::{AgentTuiToolCompletionView, AgentTuiToolKind};
use crate::surface::mailbox_telemetry::{MailboxTelemetry, MailboxTelemetrySnapshot};

const MAILBOX_PERFORMANCE_SAMPLE_COUNT: usize = 1_000;
const MAILBOX_P95_LIMIT_US: u64 = 25_000;
const MAILBOX_P99_LIMIT_US: u64 = 50_000;
const MAILBOX_WAKE_PRODUCER_DELAY: Duration = Duration::from_millis(20);
const MAILBOX_WAKE_TIMEOUT: Duration = Duration::from_secs(1);
const MAILBOX_WAKE_LATENCY_LIMIT: Duration = Duration::from_millis(500);
/// 消费者像主循环一样周期轮询邮箱，不依赖单次 park 的唤醒时机。
const MAILBOX_WAKE_POLL_INTERVAL: Duration = Duration::from_millis(5);

fn subagent_activity_event(status: WireLifecycleStatus, phase: &str) -> AgentTuiRuntimeEvent {
    crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::TurnLifecycle(
        WireTurnLifecycle {
            part_id: "subagent:/root/delegate_fast_1".to_owned(),
            turn_id: "subagent_activity".to_owned(),
            phase: WireLifecyclePhase::RegularTurn,
            status,
            title: "子代理".to_owned(),
            kind: Some("subagent_activity".to_owned()),
            ui_phase: Some(phase.to_owned()),
            tool_name: Some("claude-haiku-4-5".to_owned()),
            attempt_index: None,
            summary: Some(
                "path=/root/delegate_fast_1; task=快速委派 1; model=claude-haiku-4-5".to_owned(),
            ),
            duration_ms: Some(1_000),
            started_at_ms: Some(1),
            total_tokens: Some(128),
            cache_hit_rate_basis_points: None,

            warnings: Vec::new(),
        },
    ))
}

#[test]
fn worker_mailbox_push_wakes_owner_thread() {
    let mailbox = WorkerMailbox::new();
    let producer_mailbox = mailbox.clone();
    let producer = thread::spawn(move || {
        thread::sleep(MAILBOX_WAKE_PRODUCER_DELAY);
        producer_mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event("wake"),
        );
    });
    let started_at = Instant::now();
    while mailbox.try_take().is_none() && started_at.elapsed() < MAILBOX_WAKE_TIMEOUT {
        thread::park_timeout(
            MAILBOX_WAKE_TIMEOUT
                .saturating_sub(started_at.elapsed())
                .min(MAILBOX_WAKE_POLL_INTERVAL),
        );
    }
    producer.join().expect("worker mailbox producer 不应 panic");

    assert!(started_at.elapsed() < MAILBOX_WAKE_LATENCY_LIMIT);
}

#[test]
fn worker_mailbox_never_blocks_critical_tool_projection() {
    let mailbox = WorkerMailbox::new();
    for index in 0..ProjectionRuntimeBudget::current().semantic_mailbox_slots {
        mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(format!(
                "{index},"
            )),
        );
    }
    let tool_completion = crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
        AgentTuiToolCompletionView {
            part_id: "tool-part:test:tool-2".to_owned(),
            provider_call_id: "tool-2".to_owned(),
            tool_name: "builtin.shell_command".to_owned(),
            kind: "shell".to_owned(),
            semantic_kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "done".to_owned(),
            metadata: None,
            error_type: None,
            filesystem_write_performed: false,
            duration_ms: Some(1),
        },
    );
    let expected_event_id = tool_completion.event_id;
    mailbox.push_progress(tool_completion);

    assert!(std::iter::from_fn(|| mailbox.try_take()).any(|message| {
        matches!(message, SubmissionWorkerMessage::Progress(event) if event.event_id == expected_event_id)
    }));
}

#[test]
fn worker_mailbox_sheds_display_frames_without_blocking_producer() {
    let telemetry = MailboxTelemetry::new(true);
    let mailbox = WorkerMailbox::with_telemetry(telemetry);
    let budget = ProjectionRuntimeBudget::current();
    let progress_slots = budget
        .semantic_mailbox_slots
        .saturating_sub(budget.critical_mailbox_reserve);
    for index in 0..budget.semantic_mailbox_slots {
        mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
                format!("assistant:backpressure:{index}"),
                format!("provider-message:backpressure:{index}"),
                "delta",
            ),
        );
    }

    let producer_mailbox = mailbox.clone();
    let (producer_done_tx, producer_done_rx) = mpsc::channel();
    let producer = thread::spawn(move || {
        producer_mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
                "assistant:backpressure:overflow",
                "provider-message:backpressure:overflow",
                "delta",
            ),
        );
        producer_done_tx.send(()).expect("完成信号应可发送");
    });
    producer_done_rx
        .recv_timeout(MAILBOX_WAKE_TIMEOUT)
        .expect("满队列不得阻塞生产者");
    producer.join().expect("背压 producer 不应 panic");

    let snapshot = mailbox.telemetry_snapshot().worker;
    assert_eq!(
        snapshot.backlog_high_water, progress_slots as u64,
        "展示增量帧必须收敛到预留语义槽位后的容量"
    );
    assert!(
        snapshot.shed_count >= 1,
        "背压淘汰必须计入背压指标：{snapshot:?}"
    );
    assert!(snapshot.enqueued_count >= progress_slots as u64);
}

#[test]
fn worker_mailbox_preserves_semantic_boundary_under_stream_burst() {
    let mailbox = WorkerMailbox::new();
    let budget = ProjectionRuntimeBudget::current();
    for index in 0..budget.semantic_mailbox_slots * 2 {
        mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
                format!("assistant:burst:{index}"),
                format!("provider-message:burst:{index}"),
                "delta",
            ),
        );
    }
    let terminal = subagent_activity_event(WireLifecycleStatus::Completed, "completed");
    let terminal_event_id = terminal.event_id;
    mailbox.push_progress(terminal);

    assert!(
        drain_progress(&mailbox)
            .into_iter()
            .any(|event| event.event_id == terminal_event_id),
        "durable 语义边界帧不得被背压淘汰"
    );
}

#[test]
fn disabled_mailbox_telemetry_preserves_queue_behavior_without_samples() {
    let telemetry = MailboxTelemetry::new(false);
    let mailbox = WorkerMailbox::with_telemetry(telemetry);
    mailbox.push_progress(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event("delta"),
    );

    assert!(mailbox.try_take().is_some());
    assert_eq!(
        mailbox.telemetry_snapshot(),
        MailboxTelemetrySnapshot::default()
    );
}

#[test]
fn worker_mailbox_normal_load_stays_within_projection_budget() {
    let telemetry = MailboxTelemetry::new(true);
    let mailbox = WorkerMailbox::with_telemetry(telemetry);
    let mut latencies_us = Vec::with_capacity(MAILBOX_PERFORMANCE_SAMPLE_COUNT);

    for index in 0..MAILBOX_PERFORMANCE_SAMPLE_COUNT {
        let started_at = Instant::now();
        mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event_with_id(
                format!("assistant:mailbox-perf:{index}"),
                format!("provider-message:mailbox-perf:{index}"),
                "delta",
            ),
        );
        assert!(mailbox.try_take().is_some());
        latencies_us.push(u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX));
    }

    latencies_us.sort_unstable();
    let p95 = latency_percentile(&latencies_us, 95);
    let p99 = latency_percentile(&latencies_us, 99);
    println!("worker mailbox p95={p95}us p99={p99}us");
    assert!(p95 < MAILBOX_P95_LIMIT_US, "mailbox p95={p95}us");
    assert!(p99 < MAILBOX_P99_LIMIT_US, "mailbox p99={p99}us");

    let snapshot = mailbox.telemetry_snapshot().worker;
    assert_eq!(
        snapshot.enqueued_count,
        MAILBOX_PERFORMANCE_SAMPLE_COUNT as u64
    );
    assert_eq!(
        snapshot.drained_count,
        MAILBOX_PERFORMANCE_SAMPLE_COUNT as u64
    );
}

fn latency_percentile(sorted: &[u64], percentile: usize) -> u64 {
    let index = sorted.len().saturating_mul(percentile) / 100;
    sorted[index.min(sorted.len().saturating_sub(1))]
}

#[test]
fn worker_mailbox_projects_wire_sequence_monotonically_without_reorder_delay() {
    let telemetry = MailboxTelemetry::new(true);
    let mailbox = WorkerMailbox::with_telemetry(telemetry);
    let second = wire_sequence(
        subagent_activity_event(WireLifecycleStatus::Completed, "completed"),
        2,
        "event:2",
    );
    let first = wire_sequence(
        subagent_activity_event(WireLifecycleStatus::Running, "provider_generating"),
        1,
        "event:1",
    );

    mailbox.push_progress(second);
    mailbox.push_progress(first);

    // 高序号帧不被任何重排窗口推迟；迟到低序号帧被水位拒绝而不是回退展示层。
    let event_ids = drain_progress(&mailbox)
        .into_iter()
        .map(|event| wire_event_id(&event))
        .collect::<Vec<_>>();
    assert_eq!(event_ids, vec!["event:2"]);
    assert_eq!(mailbox.telemetry_snapshot().worker.stale_count, 1);
}

#[test]
fn worker_mailbox_rejects_duplicate_wire_identity_before_projection() {
    let mailbox = WorkerMailbox::with_telemetry(MailboxTelemetry::new(true));
    let event = wire_sequence(
        subagent_activity_event(WireLifecycleStatus::Running, "provider_generating"),
        7,
        "event:dedupe",
    );

    mailbox.push_progress(event);
    mailbox.push_progress(wire_sequence(
        subagent_activity_event(WireLifecycleStatus::Running, "provider_generating"),
        7,
        "event:dedupe",
    ));

    assert_eq!(drain_progress(&mailbox).len(), 1);
    assert_eq!(mailbox.telemetry_snapshot().worker.coalesced_count, 1);
}

#[test]
fn worker_mailbox_retains_semantic_event_after_stream_burst() {
    let mailbox = WorkerMailbox::new();
    for index in 0..ProjectionRuntimeBudget::current().semantic_mailbox_slots * 2 {
        mailbox.push_progress(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(format!(
                "{index},"
            )),
        );
    }
    let terminal = subagent_activity_event(WireLifecycleStatus::Completed, "completed");
    let terminal_event_id = terminal.event_id;
    mailbox.push_progress(terminal);

    assert!(
        drain_progress(&mailbox)
            .into_iter()
            .any(|event| event.event_id == terminal_event_id)
    );
}

#[test]
fn invalidated_worker_mailbox_drops_queued_and_late_events() {
    let mailbox = WorkerMailbox::new();
    mailbox.push_progress(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event("old"),
    );
    mailbox.invalidate();
    mailbox.push_progress(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event("late"),
    );

    assert!(mailbox.try_take().is_none());
}

fn wire_sequence(
    mut event: AgentTuiRuntimeEvent,
    sequence: u64,
    event_id: &str,
) -> AgentTuiRuntimeEvent {
    let AgentTuiRuntimePayload::WireEvent { event: wire, .. } = &mut event.payload else {
        panic!("测试事件必须是 wire payload");
    };
    wire.sequence = sequence;
    wire.event_id = WireCorrelationId(event_id.to_owned());
    event
}

fn drain_progress(mailbox: &WorkerMailbox) -> Vec<AgentTuiRuntimeEvent> {
    std::iter::from_fn(|| mailbox.try_take())
        .filter_map(|message| match message {
            SubmissionWorkerMessage::Progress(event) => Some(*event),
            SubmissionWorkerMessage::ApplySteer(_) => None,
            #[cfg(test)]
            SubmissionWorkerMessage::Finished(_) => None,
        })
        .collect()
}

fn wire_event_id(event: &AgentTuiRuntimeEvent) -> String {
    let AgentTuiRuntimePayload::WireEvent { event, .. } = &event.payload else {
        panic!("测试事件必须是 wire payload");
    };
    event.event_id.0.clone()
}

#[test]
fn runtime_idle_watchdog_projects_notice_without_cancelling_submission() {
    let mut state = test_state();
    let submission = test_submission("永不返回");
    state.start_runtime_submit(&submission);
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });

    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");
    driver.set_last_activity_for_test(
        Instant::now()
            .checked_sub(RUNTIME_IDLE_NOTICE_AFTER)
            .expect("测试 idle 窗口小于 Instant 可表示范围"),
    );
    assert!(driver.tick(&mut state).expect("idle notice tick 应成功"));

    assert!(driver.is_active());
    assert_eq!(cancel_token.lifecycle(), "created");
    let notice = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(crate::surface::cell::RenderCellData::Status(cell)) => Some(cell),
            _ => None,
        })
        .expect("idle watchdog 应投影为 notice cell");
    assert_eq!(notice.title, "运行中");
    assert_eq!(notice.items[0].kind, "等待");
    assert!(notice.items[0].summary.contains("任务仍在运行"));
    gate.release();
}

#[test]
fn worker_terminal_result_is_available_without_queue_capacity() {
    let mailbox = WorkerMailbox::new();
    mailbox.finish(Err(AgentTuiSubmitFailure::without_output(
        RuntimeError::runtime("provider failed"),
    )));

    let _error = mailbox
        .take_finished_when_drained()
        .expect("终态槽应独立于进度容量")
        .expect_err("应保留 worker 失败结果");
}

#[test]
fn cancel_token_lifecycle_moves_from_requested_to_settled_after_interrupt_tick() {
    let mut state = test_state();
    let submission = test_submission("可中断任务");
    state.start_runtime_submit(&submission);
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });

    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");
    cancel_token.cancel();
    state.request_interrupt_current_task();

    assert!(driver.tick(&mut state).expect("cancel tick 应成功"));

    assert!(!driver.is_active());
    assert_eq!(cancel_token.lifecycle(), "settled");
    gate.release();
}

#[test]
fn cancel_token_lifecycle_exposes_created_requested_observed_and_settled_states() {
    let cancel_token = AgentTuiCancelToken::default();

    assert_eq!(cancel_token.lifecycle(), "created");
    cancel_token.cancel();
    assert_eq!(cancel_token.lifecycle(), "requested");
    cancel_token.mark_cancel_observed();
    assert_eq!(cancel_token.lifecycle(), "observed");
    cancel_token.settle();
    assert_eq!(cancel_token.lifecycle(), "settled");
}

#[test]
fn cancel_token_cancels_the_runtime_execution_token() {
    let cancel_token = AgentTuiCancelToken::default();
    let run_cancel_token = cancel_token.run_cancel_token();

    cancel_token.cancel();

    assert!(run_cancel_token.is_cancelled());
}
