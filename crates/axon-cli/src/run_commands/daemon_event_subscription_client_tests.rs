//! `daemon_event_subscription_client` 的回归用例。
//!
//! 从主文件拆出（INV-7 单文件 700 行铁律），经 `#[path]` 挂回原模块。

use super::*;
use axon_protocol::wire::{WireAssistantMessage, WireEventKind};

use super::super::daemon_event_cursor::{TURN_COMPLETED_EVENT, WIRE_EVENT};

#[test]
fn ready_without_events_records_replay_cursor() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    let cursor = ConsumerCursor::new("generation", 7);
    let message = SubscriptionMessage::Ready {
        events: Vec::new(),
        page_full: false,
        snapshot_required: false,
        replay_cursor: Some(cursor.clone()),
        interaction_status: InteractionStatus::Processing,
    };

    let control = handle_subscription_message(message, &mut state).expect("处理空 replay ready");

    assert_eq!(control, SubscriptionControl::Continue);
    assert_eq!(state.cursor, Some(cursor));
}

#[test]
fn ready_notifies_connected_phase() {
    let mut phases = Vec::new();
    let mut observe = |observation| match observation {
        DaemonTurnObservation::SubscriptionPhase(phase) => phases.push(phase),
        DaemonTurnObservation::Wire(_) | DaemonTurnObservation::RuntimeObservation(_) => {}
    };
    let mut state = DaemonTurnState::new("interaction".to_owned(), Some(&mut observe));
    let message = SubscriptionMessage::Ready {
        events: Vec::new(),
        page_full: false,
        snapshot_required: false,
        replay_cursor: None,
        interaction_status: InteractionStatus::Processing,
    };

    handle_subscription_message(message, &mut state).expect("处理 ready");

    assert_eq!(phases, vec![DaemonSubscriptionPhase::Connected]);
}

#[test]
fn runtime_observation_message_delivers_to_subscriber() {
    let mut received = Vec::new();
    let mut observe = |observation| match observation {
        DaemonTurnObservation::RuntimeObservation(frame) => received.push(frame),
        DaemonTurnObservation::Wire(_) | DaemonTurnObservation::SubscriptionPhase(_) => {}
    };
    let mut state = DaemonTurnState::new("interaction".to_owned(), Some(&mut observe));
    let frame = axon_domain::RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_lsp(
        axon_domain::LspMetricsSnapshot {
            active_clients: 2,
            pending_requests: 0,
            diagnostics_count: 0,
            last_latency_ms: 0,
        },
    );
    let message = SubscriptionMessage::RuntimeObservation {
        frame: frame.clone(),
    };

    let control = handle_subscription_message(message, &mut state).expect("处理 observation");
    assert_eq!(control, SubscriptionControl::Continue);
    assert_eq!(received, vec![frame]);
}

/// 静默不是失联：读超时不得被折算成「失去订阅」。
#[test]
fn silent_read_is_not_a_lost_subscription() {
    let silent = classify_subscription_read(&Ok(None));

    assert_eq!(silent, SubscriptionRead::Silent);
    assert!(
        !silent.lost_subscription(),
        "静默不得触发订阅重建与恢复预算，否则长工具调用期间的回合会被判死"
    );
}

#[test]
fn io_failure_is_a_lost_subscription() {
    let failed = classify_subscription_read(&Err(std::io::Error::other("管道断开")));

    assert_eq!(failed, SubscriptionRead::Failed);
    assert!(failed.lost_subscription(), "连接故障必须进入重建路径");
}

/// 收到的消息（含心跳）都是订阅活着的证据，不得被当成失联。
#[test]
fn delivered_heartbeat_is_not_a_lost_subscription() {
    let response = DaemonResponse::success(
        "request",
        serde_json::json!({"event": "subscription_heartbeat"}),
    );
    let delivered = classify_subscription_read(&Ok(Some(response)));

    assert_eq!(delivered, SubscriptionRead::Message);
    assert!(!delivered.lost_subscription());
}

/// 心跳只证明订阅活着，不构成 durable 进度。
#[test]
fn heartbeat_keeps_the_subscription_flowing_without_durable_progress() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);

    let control = handle_subscription_message(SubscriptionMessage::Heartbeat, &mut state)
        .expect("心跳必须被正常处理");

    assert_eq!(control, SubscriptionControl::Continue);
    assert_eq!(state.progress_serial, 0, "心跳不构成 durable 进度");
}

#[test]
fn generation_stream_reset_reconnects_with_confirmed_cursor() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    let cursor = ConsumerCursor::new("old-generation", 7);
    state.cursor = Some(cursor.clone());
    let message = SubscriptionMessage::StreamReset {
        cursor: Some(cursor.clone()),
    };

    let control = handle_subscription_message(message, &mut state).expect("处理 stream reset");

    assert_eq!(control, SubscriptionControl::Reconnect);
    assert_eq!(state.cursor, Some(cursor));
}

#[test]
fn publish_order_reset_replays_from_confirmed_cursor() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    state.cursor = Some(ConsumerCursor::new("generation", 9));
    state.wire_events.push(WireEventEnvelope {
        schema_version: axon_protocol::wire::WireSchemaVersion::default(),
        event_id: axon_protocol::wire::WireCorrelationId("event:9".to_owned()),
        correlation_id: axon_protocol::wire::WireCorrelationId("turn".to_owned()),
        session: axon_protocol::wire::WireSessionRef {
            session_id: "session".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 9,
        recorded_at_ms: 9,
        kind: axon_protocol::wire::WireEventKind::ProviderStatus(
            axon_protocol::wire::WireProviderStatus {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                phase: axon_protocol::wire::WireProviderStatusPhase::Generating,
                transport: axon_protocol::wire::WireProviderTransport::Http,
                attempt_index: 1,
                http_status: None,
                error_message: None,
                next_provider: None,
                next_model: None,
            },
        ),
    });
    let cursor = ConsumerCursor::new("generation", 7);

    let control = handle_subscription_message(
        SubscriptionMessage::StreamReset {
            cursor: Some(cursor.clone()),
        },
        &mut state,
    )
    .expect("处理发布顺序重置");

    assert_eq!(control, SubscriptionControl::Reconnect);
    assert_eq!(state.cursor, Some(cursor));
    assert_eq!(state.wire_events.len(), 1);
}

#[test]
fn subscriber_lag_reset_replays_from_daemon_cursor_without_clearing_projection() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    let cursor = ConsumerCursor::new("generation", 7);
    state.cursor = Some(cursor.clone());
    state.wire_events.push(WireEventEnvelope {
        schema_version: axon_protocol::wire::WireSchemaVersion::default(),
        event_id: axon_protocol::wire::WireCorrelationId("event:7".to_owned()),
        correlation_id: axon_protocol::wire::WireCorrelationId("turn".to_owned()),
        session: axon_protocol::wire::WireSessionRef {
            session_id: "session".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 7,
        recorded_at_ms: 7,
        kind: axon_protocol::wire::WireEventKind::ProviderStatus(
            axon_protocol::wire::WireProviderStatus {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                phase: axon_protocol::wire::WireProviderStatusPhase::Generating,
                transport: axon_protocol::wire::WireProviderTransport::Http,
                attempt_index: 1,
                http_status: None,
                error_message: None,
                next_provider: None,
                next_model: None,
            },
        ),
    });

    let control = handle_subscription_message(
        SubscriptionMessage::StreamReset {
            cursor: Some(cursor.clone()),
        },
        &mut state,
    )
    .expect("处理订阅落后重置");

    assert_eq!(control, SubscriptionControl::Reconnect);
    assert_eq!(state.cursor, Some(cursor));
    assert_eq!(state.wire_events.len(), 1);
}

#[test]
fn interaction_status_counts_as_durable_progress_but_heartbeats_do_not() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    let baseline = state.progress_serial;

    handle_subscription_message(SubscriptionMessage::Heartbeat, &mut state)
        .expect("心跳必须被正常处理");
    assert_eq!(
        state.progress_serial, baseline,
        "心跳不能算 durable 进度，否则挂死的回合永远无法被发现"
    );

    handle_subscription_message(
        SubscriptionMessage::InteractionStatus {
            status: InteractionStatus::Understood,
        },
        &mut state,
    )
    .expect("状态推进必须被正常处理");
    assert_eq!(state.progress_serial, baseline + 1);
}

#[test]
fn snapshot_ready_replaces_stale_cursor_with_new_replay_cursor() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    state.cursor = Some(ConsumerCursor::new("old-generation", 9));
    let cursor = ConsumerCursor::new("new-generation", 12);
    let message = SubscriptionMessage::Ready {
        events: Vec::new(),
        page_full: false,
        snapshot_required: true,
        replay_cursor: Some(cursor.clone()),
        interaction_status: InteractionStatus::Processing,
    };

    let control = handle_subscription_message(message, &mut state).expect("处理 snapshot ready");

    assert_eq!(control, SubscriptionControl::Continue);
    assert_eq!(state.cursor, Some(cursor));
}

#[test]
fn ready_replay_cursor_does_not_rewind_durable_cursor() {
    let mut state = DaemonTurnState::new("interaction".to_owned(), None);
    state.cursor = Some(ConsumerCursor::new("generation", 8));
    let message = SubscriptionMessage::Ready {
        events: Vec::new(),
        page_full: false,
        snapshot_required: false,
        replay_cursor: Some(ConsumerCursor::new("generation", 7)),
        interaction_status: InteractionStatus::Processing,
    };

    handle_subscription_message(message, &mut state).expect("处理回退 ready");

    assert_eq!(state.cursor, Some(ConsumerCursor::new("generation", 8)));
}

const REPLAY_GENERATION: &str = "generation-a";
const REPLAY_INTERACTION_ID: &str = "interaction-replay";
const REPLAY_SESSION_ID: &str = "replay-session";

/// 构造一条 durable replay 记录；payload 必须带顶层 `interactionId`（与生产一致）。
fn replay_record(
    event_type: &str,
    sequence: u64,
    interaction_id: &str,
    mut payload: serde_json::Value,
) -> SessionEventRecord {
    payload["interactionId"] = serde_json::Value::String(interaction_id.to_owned());
    SessionEventRecord {
        session_id: axon_domain::DaemonSessionId::new(REPLAY_SESSION_ID).expect("Session ID"),
        sequence,
        emitted_at_ms: sequence,
        generation: REPLAY_GENERATION.to_owned(),
        event_type: event_type.to_owned(),
        payload_json: payload.to_string(),
    }
}

fn replay_turn_completed(sequence: u64, response: &str) -> SessionEventRecord {
    replay_record(
        TURN_COMPLETED_EVENT,
        sequence,
        REPLAY_INTERACTION_ID,
        serde_json::json!({ "response": response }),
    )
}

fn replay_wire_event(sequence: u64) -> SessionEventRecord {
    let envelope = WireEventEnvelope {
        schema_version: axon_protocol::wire::WireSchemaVersion::default(),
        event_id: axon_protocol::wire::WireCorrelationId(format!("event:{sequence}")),
        correlation_id: axon_protocol::wire::WireCorrelationId("turn".to_owned()),
        session: axon_protocol::wire::WireSessionRef {
            session_id: REPLAY_SESSION_ID.to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence,
        recorded_at_ms: i64::try_from(sequence).expect("序号可表示为时间戳"),
        kind: WireEventKind::AssistantMessage(WireAssistantMessage {
            part_id: format!("assistant:{sequence}"),
            message_id: format!("assistant-message:{sequence}"),
            content: format!("增量 {sequence}"),
            delta: true,
        }),
    };
    replay_record(
        WIRE_EVENT,
        sequence,
        REPLAY_INTERACTION_ID,
        serde_json::to_value(envelope).expect("wire 事件可序列化"),
    )
}

/// 回归：重连页的**唯一**事件就是本轮 `turn_completed` 时，它必须被真正消费。
///
/// 旧顺序先用 `replayCursor` 覆盖客户端游标，页内事件会被去重逻辑当成重复丢弃；
/// 页尾恰是终态时前台等待将永久拿不到终态（TUI 一直显示「生成回复中」）。
#[test]
fn replay_page_tail_terminal_event_is_consumed_before_watermark() {
    let mut state = DaemonTurnState::new(REPLAY_INTERACTION_ID.to_owned(), None);
    state.cursor = Some(ConsumerCursor::new(REPLAY_GENERATION, 7));
    let message = SubscriptionMessage::Ready {
        events: vec![replay_turn_completed(8, "最终正文")],
        page_full: false,
        snapshot_required: false,
        replay_cursor: Some(ConsumerCursor::new(REPLAY_GENERATION, 8)),
        interaction_status: InteractionStatus::Processing,
    };

    let control = handle_subscription_message(message, &mut state).expect("处理页尾终态");

    assert_eq!(
        control,
        SubscriptionControl::Completed("最终正文".to_owned())
    );
    assert_eq!(
        state.cursor,
        Some(ConsumerCursor::new(REPLAY_GENERATION, 8))
    );
}

/// 同一缺陷的非终态表现：页内唯一事件也必须进入投影，不能被水位去重吞掉。
#[test]
fn replay_single_event_page_still_projects_its_event() {
    let mut state = DaemonTurnState::new(REPLAY_INTERACTION_ID.to_owned(), None);
    state.cursor = Some(ConsumerCursor::new(REPLAY_GENERATION, 7));
    let message = SubscriptionMessage::Ready {
        events: vec![replay_wire_event(8)],
        page_full: false,
        snapshot_required: false,
        replay_cursor: Some(ConsumerCursor::new(REPLAY_GENERATION, 8)),
        interaction_status: InteractionStatus::Processing,
    };

    let control = handle_subscription_message(message, &mut state).expect("处理单事件页");

    assert_eq!(control, SubscriptionControl::Continue);
    assert_eq!(state.wire_events.len(), 1, "页内事件必须进入投影");
    assert_eq!(
        state.cursor,
        Some(ConsumerCursor::new(REPLAY_GENERATION, 8))
    );
}

/// 空页仍必须采纳服务端水位并继续翻页，否则 `page_full` 重连会失去推进依据。
#[test]
fn replay_empty_full_page_reconnects_with_advanced_watermark() {
    let mut state = DaemonTurnState::new(REPLAY_INTERACTION_ID.to_owned(), None);
    let message = SubscriptionMessage::Ready {
        events: Vec::new(),
        page_full: true,
        snapshot_required: false,
        replay_cursor: Some(ConsumerCursor::new(REPLAY_GENERATION, 12)),
        interaction_status: InteractionStatus::Processing,
    };

    let control = handle_subscription_message(message, &mut state).expect("处理满页空页");

    assert_eq!(control, SubscriptionControl::Reconnect);
    assert_eq!(
        state.cursor,
        Some(ConsumerCursor::new(REPLAY_GENERATION, 12))
    );
}
