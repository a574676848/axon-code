//! Interaction 中断事件的统一出站投影。

use crate::application::egress_bridge::CommittedEventEgress;
use crate::application::event_stream_bridge::EventStreamBridge;
use axon_domain::{InteractionId, InteractionStatus};
use axon_session::SessionEventRecord;
use axon_session_runtime::EventPublisher;

/// 发布已持久化的中断事件；实时桥失败不影响 durable 收敛结果。
pub(super) fn project_interrupted_interactions<P>(
    publisher: &P,
    stream_bridge: &EventStreamBridge,
    records: &[SessionEventRecord],
) where
    P: EventPublisher,
{
    let egress = CommittedEventEgress::new(publisher, stream_bridge);
    for record in records {
        let cursor = record.cursor();
        let _ = egress.publish(record, &cursor);
        if record.event_type != "interaction_interrupted" {
            continue;
        }
        let Some(interaction_id) = parse_interaction_id(record) else {
            continue;
        };
        stream_bridge.publish_interaction_status(
            &record.session_id,
            &interaction_id,
            InteractionStatus::Interrupted,
        );
    }
}

fn parse_interaction_id(record: &SessionEventRecord) -> Option<InteractionId> {
    let payload = serde_json::from_str::<serde_json::Value>(&record.payload_json).ok()?;
    payload
        .get("interactionId")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| InteractionId::new(value).ok())
}
