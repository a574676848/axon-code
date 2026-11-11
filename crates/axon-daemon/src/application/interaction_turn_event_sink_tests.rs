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
fn stream_deltas_and_usage_stay_off_the_durable_path() {
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

    let usage = wire_payload("usageUpdate", json!({"inputTokens": 1}));
    assert!(matches!(classify_frame(&usage), FrameClass::LiveOnly));

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
