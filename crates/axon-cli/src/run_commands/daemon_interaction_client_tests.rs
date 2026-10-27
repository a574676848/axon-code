//! `daemon_interaction_client` 的回归用例。
//!
//! 从主文件拆出（INV-7 单文件 700 行铁律），经 `#[path]` 挂回原模块。

use super::*;
use axon_domain::DaemonSessionId;
use axon_protocol::wire::{
    WireCorrelationId, WireLifecyclePhase, WireLifecycleStatus, WireSchemaVersion, WireSessionRef,
    WireTurnLifecycle,
};

use super::super::run_compaction_projection::{COMPACTION_COMPLETED_KIND, COMPACTION_STARTED_KIND};

#[test]
fn durable_events_restore_wire_output_and_final_response() {
    let session_id = DaemonSessionId::new("primary-session").expect("Session ID");
    let interaction_id = "subscription-interaction";
    let wire = synthetic_wire_event(session_id.as_str(), "增量完成");
    let events = vec![
        session_event(
            &session_id,
            1,
            WIRE_EVENT,
            wire_payload(&wire, interaction_id),
        ),
        session_event(
            &session_id,
            2,
            TURN_COMPLETED_EVENT,
            serde_json::json!({"interactionId": interaction_id, "response":"最终回答"}).to_string(),
        ),
    ];
    let mut cursor = None;
    let mut wire_events = Vec::new();
    let mut observer = Vec::new();
    let mut observe = |event| observer.push(event);
    let response = collect_events(
        events,
        "subscription-interaction",
        &mut cursor,
        &mut wire_events,
        &mut Some(&mut observe),
    )
    .expect("事件应可恢复");

    assert_eq!(response.as_deref(), Some("最终回答"));
    assert_eq!(wire_events, vec![wire.clone()]);
    assert_eq!(observer, vec![wire]);
    assert_eq!(cursor.expect("cursor").sequence, 2);
}

#[test]
fn subscription_ready_replay_completes_without_status_polling() {
    let session_id = DaemonSessionId::new("subscription-session").expect("Session ID");
    let mut state = DaemonTurnState::new("subscription-interaction".to_owned(), None);
    let message = SubscriptionMessage::Ready {
        events: vec![session_event(
            &session_id,
            1,
            TURN_COMPLETED_EVENT,
            serde_json::json!({"interactionId":"subscription-interaction", "response":"订阅完成"})
                .to_string(),
        )],
        page_full: false,
        snapshot_required: false,
        replay_cursor: Some(axon_domain::ConsumerCursor::new(
            "subscription-generation",
            1,
        )),
        interaction_status: InteractionStatus::Processing,
    };

    let control = handle_subscription_message(message, &mut state).expect("处理订阅 replay");

    assert_eq!(
        control,
        SubscriptionControl::Completed("订阅完成".to_owned())
    );
    assert_eq!(state.cursor.expect("cursor").sequence, 1);
}

#[test]
fn full_subscription_replay_page_requests_cursor_resubscribe() {
    assert_eq!(
        completed_or_status(None, InteractionStatus::Received, true).expect("满页应继续订阅"),
        SubscriptionControl::Reconnect
    );
}

#[test]
fn completed_interaction_with_full_historical_page_keeps_replaying() {
    assert_eq!(
        completed_or_status(None, InteractionStatus::Understood, true)
            .expect("完成状态的历史满页应继续追读"),
        SubscriptionControl::Reconnect
    );
}

#[test]
fn understood_status_without_completion_keeps_waiting_for_durable_event() {
    assert_eq!(
        completed_or_status(None, InteractionStatus::Understood, false)
            .expect("状态通知不应替代 durable 完成事件"),
        SubscriptionControl::Continue
    );
}

#[test]
fn no_deadline_keeps_waiting() {
    assert!(ensure_wait_can_continue(None, &|| false).is_ok());
}

#[test]
fn configured_deadline_still_times_out() {
    let deadline = Some(Instant::now() - Duration::from_secs(1));
    let error = ensure_wait_can_continue(deadline, &|| false).expect_err("超时期限必须报错");
    assert!(
        error.contains("仍在 Daemon 中继续执行"),
        "超时文案必须说明 Interaction 仍在执行，而不是让调用方误以为回合失败：{error}"
    );
}

#[test]
fn historical_events_are_ignored_but_cursor_advances() {
    let session_id = DaemonSessionId::new("history-session").expect("Session ID");
    let old_wire = synthetic_wire_event(session_id.as_str(), "旧回复");
    let events = vec![
        session_event(
            &session_id,
            20,
            WIRE_EVENT,
            serde_json::to_string(&old_wire).unwrap(),
        ),
        session_event(
            &session_id,
            21,
            TURN_COMPLETED_EVENT,
            serde_json::json!({"interactionId":"old-interaction", "response":"旧回复"}).to_string(),
        ),
    ];
    let mut cursor = None;
    let mut wire_events = Vec::new();
    let response = collect_events(
        events,
        "new-interaction",
        &mut cursor,
        &mut wire_events,
        &mut None,
    )
    .expect("历史事件应可跳过");
    assert_eq!(response, None);
    assert!(wire_events.is_empty());
    assert_eq!(cursor.expect("cursor").sequence, 21);
}

#[test]
fn text_live_output_deduplicates_terminal_assistant_with_different_message_id() {
    let session_id = DaemonSessionId::new("cross-id-live-output-session").expect("Session ID");
    let mut delta = synthetic_wire_event(session_id.as_str(), "跨 ID ");
    if let WireEventKind::AssistantMessage(message) = &mut delta.kind {
        message.delta = true;
        message.message_id = "provider-assistant:1".to_owned();
    }
    let mut terminal = synthetic_wire_event(session_id.as_str(), "跨 ID 输出");
    if let WireEventKind::AssistantMessage(message) = &mut terminal.kind {
        message.message_id = "assistant-message-final".to_owned();
    }
    let mut output = Vec::new();
    {
        let mut live = DaemonTextLiveOutput::new(Some(&mut output));
        live.observe(&delta);
        live.observe(&terminal);
        live.finish();
        assert!(live.error.is_none());
    }
    assert_eq!(
        String::from_utf8(output).expect("实时输出应为 UTF-8"),
        "跨 ID 输出\n"
    );
}

#[test]
fn text_render_skips_terminal_duplicate_after_live_output() {
    let projection = DaemonTurnProjection {
        session_id: "session".to_owned(),
        response: "最终回答".to_owned(),
        warnings: Vec::new(),
        wire_events: Vec::new(),
    };
    assert_eq!(
        render_projection_output(&projection, OutputFormat::Text, true)
            .expect("实时输出后不应重复终态"),
        ""
    );
}

#[test]
fn text_live_output_projects_compaction_lifecycle_on_its_own_line() {
    let session_id = DaemonSessionId::new("compaction-live-output-session").expect("Session ID");
    let mut delta = synthetic_wire_event(session_id.as_str(), "压缩前正文");
    if let WireEventKind::AssistantMessage(message) = &mut delta.kind {
        message.delta = true;
    }
    let mut output = Vec::new();
    {
        let mut live = DaemonTextLiveOutput::new(Some(&mut output));
        live.observe(&delta);
        live.observe(&compaction_lifecycle_event(
            session_id.as_str(),
            COMPACTION_COMPLETED_KIND,
            "空闲窗口轮转完成 · 消息 42 → 6 条",
        ));
        live.finish();
        assert!(live.error.is_none(), "压缩行投影不应报错");
    }
    assert_eq!(
        String::from_utf8(output).expect("实时输出应为 UTF-8"),
        "压缩前正文\n· 空闲窗口轮转完成 · 消息 42 → 6 条\n"
    );
}

#[test]
fn text_live_output_keeps_terminal_summary_when_only_compaction_is_projected() {
    let session_id = DaemonSessionId::new("compaction-only-output-session").expect("Session ID");
    let mut output = Vec::new();
    let emitted = {
        let mut live = DaemonTextLiveOutput::new(Some(&mut output));
        live.observe(&compaction_lifecycle_event(
            session_id.as_str(),
            COMPACTION_STARTED_KIND,
            "内联窗口轮转已开始",
        ));
        live.observe(&compaction_lifecycle_event(
            session_id.as_str(),
            COMPACTION_COMPLETED_KIND,
            "内联窗口轮转完成",
        ));
        live.finish();
        assert!(live.error.is_none(), "压缩行投影不应报错");
        live.emitted
    };
    assert_eq!(
        String::from_utf8(output).expect("实时输出应为 UTF-8"),
        "· 内联窗口轮转完成\n",
        "进行中的压缩事件不输出，避免同一轮转两行"
    );
    assert!(
        !emitted,
        "只有压缩事件时不得抑制终态摘要，否则用户会丢失最终回答"
    );
}

fn compaction_lifecycle_event(session_id: &str, kind: &str, summary: &str) -> WireEventEnvelope {
    WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(format!("{session_id}:{kind}")),
        correlation_id: WireCorrelationId(kind.to_owned()),
        session: WireSessionRef {
            session_id: session_id.to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 1,
        recorded_at_ms: 1,
        kind: WireEventKind::TurnLifecycle(WireTurnLifecycle {
            part_id: format!("lifecycle:compaction:inline:{session_id}"),
            turn_id: kind.to_owned(),
            phase: WireLifecyclePhase::RegularTurn,
            status: WireLifecycleStatus::Completed,
            title: "窗口轮转".to_owned(),
            kind: Some(kind.to_owned()),
            ui_phase: Some("history".to_owned()),
            tool_name: None,
            attempt_index: None,
            summary: Some(summary.to_owned()),
            duration_ms: None,
            started_at_ms: None,
            total_tokens: None,
            cache_hit_rate_basis_points: None,

            warnings: Vec::new(),
        }),
    }
}

fn synthetic_wire_event(session_id: &str, content: &str) -> WireEventEnvelope {
    let mut events = Vec::new();
    ensure_assistant_event(session_id, content, &mut events);
    events.remove(0)
}

fn wire_payload(wire: &WireEventEnvelope, interaction_id: &str) -> String {
    let mut payload = serde_json::to_value(wire).expect("wire payload");
    payload
        .as_object_mut()
        .expect("wire payload object")
        .insert(
            "interactionId".to_owned(),
            serde_json::Value::String(interaction_id.to_owned()),
        );
    serde_json::to_string(&payload).expect("wire payload json")
}

fn session_event(
    session_id: &DaemonSessionId,
    sequence: u64,
    event_type: &str,
    payload_json: String,
) -> SessionEventRecord {
    SessionEventRecord {
        session_id: session_id.clone(),
        sequence,
        generation: "generation-1".to_owned(),
        event_type: event_type.to_owned(),
        payload_json,
        emitted_at_ms: 1,
    }
}
