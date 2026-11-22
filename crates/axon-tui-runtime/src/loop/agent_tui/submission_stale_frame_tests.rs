//! 存量逆序日志在 TUI 订阅路径上的可观测性。
//!
//! 夹具完全由构造的 wire 事件组成，不读 `$HOME/.axon` 下的任何真实会话数据：这里要证明
//! 的是「低序号迟到帧被拒时计数可读」与「终态正文不会跟着一起消失」两条判定，与现场
//! 数据无关。

use axon_protocol::wire::{
    WireAssistantMessage, WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase,
    WireLifecycleStatus, WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
};

use super::submission_worker_mailbox::WorkerMailbox;
use super::test_support::*;
use super::*;
use crate::surface::mailbox_telemetry::MailboxTelemetry;

const STALE_FIXTURE_SESSION: &str = "session-stale-fixture";
const STALE_FIXTURE_RUN: &str = "run-stale-fixture";

fn stale_fixture_event(sequence: u64, kind: WireEventKind) -> AgentTuiRuntimeEvent {
    runtime_driver_event(AgentTuiRuntimePayload::WireEvent {
        event: Box::new(WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("event:{sequence}")),
            correlation_id: WireCorrelationId("turn:stale".to_owned()),
            session: WireSessionRef {
                session_id: STALE_FIXTURE_SESSION.to_owned(),
                run_id: Some(STALE_FIXTURE_RUN.to_owned()),
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap_or(i64::MAX),
            kind,
        }),
    })
}

fn assistant_delta(sequence: u64, part_id: &str, content: &str) -> AgentTuiRuntimeEvent {
    stale_fixture_event(
        sequence,
        WireEventKind::AssistantMessage(WireAssistantMessage {
            part_id: part_id.to_owned(),
            message_id: format!("message:{part_id}"),
            content: content.to_owned(),
            delta: true,
        }),
    )
}

fn assistant_terminal_body(sequence: u64, part_id: &str, content: &str) -> AgentTuiRuntimeEvent {
    stale_fixture_event(
        sequence,
        WireEventKind::AssistantMessage(WireAssistantMessage {
            part_id: part_id.to_owned(),
            message_id: format!("message:{part_id}"),
            content: content.to_owned(),
            delta: false,
        }),
    )
}

fn terminal_run_lifecycle(sequence: u64) -> AgentTuiRuntimeEvent {
    stale_fixture_event(
        sequence,
        WireEventKind::TurnLifecycle(WireTurnLifecycle {
            part_id: "lifecycle:run".to_owned(),
            turn_id: "turn:stale".to_owned(),
            phase: WireLifecyclePhase::RegularTurn,
            status: WireLifecycleStatus::Completed,
            title: "本轮完成".to_owned(),
            kind: Some("RunLifecycle".to_owned()),
            ui_phase: None,
            tool_name: None,
            attempt_index: None,
            summary: None,
            duration_ms: None,
            started_at_ms: None,
            total_tokens: None,
            cache_hit_rate_basis_points: None,
            warnings: Vec::new(),
        }),
    )
}

/// 把邮箱里的内容按真实主循环路径投影进状态。
fn drain_mailbox(state: &mut AgentTuiState, mailbox: &WorkerMailbox) {
    let mut notifications = TuiNotificationManager::default();
    let mut last_runtime_event_id = None;
    let mut pending_assistant_delta = PendingAssistantDeltaBatch::default();
    let mut pending_assistant_delta_started_at = None;
    drain_worker_mailbox_with_event_id(
        state,
        mailbox,
        &mut notifications,
        &mut last_runtime_event_id,
        &mut pending_assistant_delta,
        &mut pending_assistant_delta_started_at,
        true,
    )
    .expect("邮箱投影不应失败");
}

#[test]
fn stale_frames_are_counted_and_visible() {
    let mailbox = WorkerMailbox::with_telemetry(MailboxTelemetry::new(true));

    mailbox.push_progress(assistant_delta(4, "assistant:stale", "先到的后续片段"));
    // 存量脏日志形态：低序号帧在更高水位之后才到达，展示层已越过它。
    mailbox.push_progress(assistant_delta(2, "assistant:stale", "迟到的低序号片段"));

    let snapshot = mailbox.telemetry_snapshot();
    assert!(snapshot.enabled, "计数必须真的被采集，禁用时全是 0");
    assert_eq!(
        snapshot.worker.stale_count, 1,
        "低序号迟到帧必须计入 staleCount"
    );
    assert_eq!(
        snapshot.worker.late_terminal_count, 0,
        "普通增量帧不享受终态放行"
    );

    // 出口：同一个计数进入 TUI 遥测快照，随启动报告交给宿主落日志。
    let mut state = test_state();
    state.attach_mailbox_telemetry(mailbox.telemetry());
    let telemetry = state.telemetry_snapshot();
    assert!(telemetry.mailbox.enabled);
    assert_eq!(telemetry.mailbox.worker.stale_count, 1);
}

#[test]
fn terminal_body_survives_stale_frame_drops() {
    let mailbox = WorkerMailbox::with_telemetry(MailboxTelemetry::new(true));
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("构造逆序日志"));

    mailbox.push_progress(assistant_delta(6, "assistant:stale", "过程的后续片段\n"));
    // 终态正文快照与终态生命周期都带着更小的序号：存量乱序日志的典型形态。
    mailbox.push_progress(assistant_terminal_body(3, "assistant:stale", "最终正文"));
    mailbox.push_progress(terminal_run_lifecycle(5));

    drain_mailbox(&mut state, &mailbox);

    let visible = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(crate::surface::cell::RenderCellData::Message(_)) => Some(line.text.clone()),
            _ => None,
        })
        .expect("终态正文必须落进 transcript");
    assert!(
        visible.contains("最终正文"),
        "终态正文不得因低序号被拒而整段消失：{visible}"
    );

    let snapshot = mailbox.telemetry_snapshot();
    assert_eq!(
        snapshot.worker.late_terminal_count, 2,
        "正文快照与终态生命周期都必须按非单调接受并计数"
    );
    assert_eq!(
        snapshot.worker.stale_count, 0,
        "终态帧被接受时不应误记为丢帧"
    );
}

#[test]
fn stale_delta_after_terminal_body_does_not_erase_final_text() {
    // 终态正文之后才到的低序号增量是「过程碎片」，被拒后正文必须还在。
    let mailbox = WorkerMailbox::with_telemetry(MailboxTelemetry::new(true));
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("构造逆序日志"));

    mailbox.push_progress(assistant_terminal_body(4, "assistant:stale", "最终正文"));
    mailbox.push_progress(assistant_delta(2, "assistant:stale", "更早的过程碎片"));

    drain_mailbox(&mut state, &mailbox);

    let visible = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(crate::surface::cell::RenderCellData::Message(_)) => Some(line.text.clone()),
            _ => None,
        })
        .expect("终态正文必须落进 transcript");
    assert_eq!(visible, "最终正文");
    assert_eq!(mailbox.telemetry_snapshot().worker.stale_count, 1);
}

#[test]
fn closed_mailbox_ignores_late_progress() {
    // 收口后使订阅失效：旧流不得再污染下一轮提交，计数也不再增长。
    let mailbox = WorkerMailbox::with_telemetry(MailboxTelemetry::new(true));
    mailbox.invalidate();
    mailbox.push_progress(assistant_delta(1, "assistant:stale", "旧流片段"));
    let snapshot = mailbox.telemetry_snapshot();
    assert_eq!(snapshot.worker.enqueued_count, 0);
    assert_eq!(snapshot.worker.stale_count, 0);
}
