use super::*;

use axon_domain::DaemonSessionId;

fn event(session_id: &DaemonSessionId, sequence: u64) -> SessionEventRecord {
    SessionEventRecord {
        session_id: session_id.clone(),
        sequence,
        emitted_at_ms: sequence,
        generation: "generation-a".to_owned(),
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"interactionId":"interaction-a"}"#.to_owned(),
    }
}

#[test]
fn publish_order_violation_resets_instead_of_projecting_late_event() {
    let bridge = EventStreamBridge::new();
    let session = DaemonSessionId::new("order-session").expect("Session ID");
    let subscription = bridge
        .subscribe(
            session.clone(),
            InteractionId::new("interaction-a").expect("Interaction ID"),
            None,
        )
        .expect("订阅");

    bridge.publish(&event(&session, 2), &ConsumerCursor::new("generation-a", 2));
    bridge.publish(&event(&session, 1), &ConsumerCursor::new("generation-a", 1));

    assert!(matches!(
        subscription.recv().expect("首个事件"),
        StreamNotification::SessionEvent(StreamEvent { cursor, .. })
            if cursor.sequence == 2
    ));
    assert_eq!(
        subscription.recv().expect("必须收到重置"),
        StreamNotification::StreamReset {
            cursor: Some(ConsumerCursor::new("generation-a", 2)),
            reason: STREAM_RESET_REASON_PUBLISH_ORDER,
        }
    );
}
