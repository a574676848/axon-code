//! `interaction_turn_event_sink` 的回归用例：实时帧与边界快照的分流。

use super::*;
use serde_json::json;

fn interaction_id() -> InteractionId {
    InteractionId::new("interaction-projection-test").expect("Interaction ID 应合法")
}

fn wire_payload(kind_type: &str, payload: Value) -> String {
    json!({
        "schemaVersion": "v1",
        "eventId": "event-1",
        "correlationId": "assistant",
        "session": {"sessionId": "session-1"},
        "sequence": 0,
        "recordedAtMs": 1,
        "kind": {"type": kind_type, "payload": payload},
    })
    .to_string()
}

#[test]
fn stream_event_annotation_adds_only_interaction_id() {
    let payload = annotate_live_event(
        r#"{"kind":{"type":"assistantMessage","payload":{}}}"#,
        &interaction_id(),
    )
    .expect("流事件应可标注");
    let payload: Value = serde_json::from_str(&payload).expect("payload 应合法");

    assert_eq!(payload["interactionId"], "interaction-projection-test");
    assert!(payload.get("transportContext").is_none());
}

#[test]
fn stream_deltas_stay_off_the_durable_path_and_usage_keeps_a_replay_copy() {
    let text = wire_payload(
        "reasoningDelta",
        json!({"partId": "reasoning-part:1", "reasoningId": "r1", "delta": "想"}),
    );
    assert!(matches!(
        classify_frame(&text),
        FrameClass::LivePart {
            kind: LivePartKind::Reasoning,
            ..
        }
    ));

    // 用量必须能回放：切回会话时这一轮读数只可能来自持久副本。
    let usage = wire_payload("usageUpdate", json!({"inputTokens": 1}));
    assert!(matches!(classify_frame(&usage), FrameClass::Snapshot));

    // 供应商状态是瞬态读数，不进持久层。
    let provider = wire_payload("providerStatus", json!({"activity": "thinking"}));
    assert!(matches!(
        classify_frame(&provider),
        FrameClass::LiveOnly { remember: true }
    ));

    let tool_call = wire_payload("toolCall", json!({"partId": "tool-1"}));
    assert!(matches!(classify_frame(&tool_call), FrameClass::Durable));

    let canonical = wire_payload(
        "assistantMessage",
        json!({"partId": "assistant-1", "content": "done"}),
    );
    assert!(matches!(classify_frame(&canonical), FrameClass::Durable));
}

#[test]
fn snapshot_reuses_the_same_part_envelope() {
    let first = wire_payload(
        "reasoningDelta",
        json!({"partId": "r-part", "reasoningId": "r1", "delta": "先想"}),
    );
    let envelope: Value = serde_json::from_str(&first).expect("信封");
    assert_eq!(
        live_part_text(&envelope, LivePartKind::Reasoning).as_deref(),
        Some("先想")
    );

    let assistant = wire_payload(
        "assistantMessage",
        json!({"partId": "a-part", "messageId": "m1", "content": "答", "delta": true}),
    );
    let envelope: Value = serde_json::from_str(&assistant).expect("信封");
    assert_eq!(
        live_part_text(&envelope, LivePartKind::Assistant).as_deref(),
        Some("答")
    );
}

fn runtime_event(event_type: &str, payload_json: String) -> RuntimeEvent {
    RuntimeEvent {
        event_type: event_type.to_owned(),
        payload_json,
    }
}

#[test]
fn turn_terminal_covers_the_completion_event_and_terminal_lifecycles() {
    assert!(is_turn_terminal(&runtime_event(
        "turn_completed",
        "{}".to_owned()
    )));

    for status in ["completed", "failed", "cancelled", "aborted"] {
        let payload = wire_payload("turnLifecycle", json!({ "status": status }));
        assert!(
            is_turn_terminal(&runtime_event("wire_event", payload)),
            "{status} 属于收口终态"
        );
    }

    // 运行中的生命周期不是收口：误判会把正在发生的阶段一起抹掉。
    let running = wire_payload("turnLifecycle", json!({ "status": "running" }));
    assert!(!is_turn_terminal(&runtime_event("wire_event", running)));

    // 其余 durable 帧与收口无关，不必每一条都去清账。
    let tool_call = wire_payload("toolCall", json!({ "partId": "tool-1" }));
    assert!(!is_turn_terminal(&runtime_event("wire_event", tool_call)));
}
