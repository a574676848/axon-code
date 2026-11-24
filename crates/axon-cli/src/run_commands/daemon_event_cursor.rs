//! Daemon durable event cursor replay 与 Interaction 过滤。

use axon_protocol::wire::WireEventEnvelope;
#[cfg(test)]
use axon_protocol::wire::{
    WireAssistantMessage, WireCorrelationId, WireEventKind, WireSchemaVersion, WireSessionRef,
};
use axon_session::SessionEventRecord;
use serde::Deserialize;

pub(super) const TURN_COMPLETED_EVENT: &str = "turn_completed";
pub(super) const WIRE_EVENT: &str = "wire_event";
#[cfg(test)]
const ASSISTANT_PART_ID: &str = "assistant:daemon-primary";
#[cfg(test)]
const ASSISTANT_MESSAGE_ID: &str = "assistant-message-final";
#[cfg(test)]
const ASSISTANT_CORRELATION_ID: &str = "assistant";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnCompletedPayload {
    response: String,
}

pub(super) fn collect_events(
    events: Vec<SessionEventRecord>,
    interaction_id: &str,
    cursor: &mut Option<axon_domain::ConsumerCursor>,
    wire_events: &mut Vec<WireEventEnvelope>,
    on_wire_event: &mut Option<&mut dyn FnMut(WireEventEnvelope)>,
) -> Result<Option<String>, String> {
    let mut response = None;
    for event in events {
        if cursor
            .as_ref()
            .is_some_and(|previous| event.cursor() == *previous)
        {
            continue;
        }
        *cursor = Some(event.cursor());
        match event.event_type.as_str() {
            WIRE_EVENT => {
                if !event_matches_interaction(&event, interaction_id) {
                    continue;
                }
                let wire = serde_json::from_str::<WireEventEnvelope>(&event.payload_json)
                    .map_err(|error| format!("解析 durable wire event 失败：{error}"))?;
                if let Some(observer) = on_wire_event.as_deref_mut() {
                    observer(wire.clone());
                }
                wire_events.push(wire);
            }
            TURN_COMPLETED_EVENT => {
                if !event_matches_interaction(&event, interaction_id) {
                    continue;
                }
                let completed =
                    serde_json::from_str::<TurnCompletedPayload>(&event.payload_json)
                        .map_err(|error| format!("解析 turn_completed 事件失败：{error}"))?;
                response = Some(completed.response);
            }
            _ => {}
        }
    }
    Ok(response)
}

fn event_matches_interaction(event: &SessionEventRecord, interaction_id: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .ok()
        .and_then(|payload| {
            payload
                .get("interactionId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|value| value == interaction_id)
}

#[cfg(test)]
pub(super) fn ensure_assistant_event(
    session_id: &str,
    response: &str,
    wire_events: &mut Vec<WireEventEnvelope>,
) {
    let has_assistant = wire_events
        .iter()
        .any(|event| matches!(event.kind, WireEventKind::AssistantMessage(_)));
    if has_assistant || response.trim().is_empty() {
        return;
    }
    let next_sequence = u64::try_from(wire_events.len())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    wire_events.push(WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(format!("{session_id}:{ASSISTANT_PART_ID}")),
        correlation_id: WireCorrelationId(ASSISTANT_CORRELATION_ID.to_owned()),
        session: WireSessionRef {
            session_id: session_id.to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: next_sequence,
        recorded_at_ms: i64::try_from(axon_time::unix_now_ms()).unwrap_or(i64::MAX),
        kind: WireEventKind::AssistantMessage(WireAssistantMessage {
            part_id: ASSISTANT_PART_ID.to_owned(),
            message_id: ASSISTANT_MESSAGE_ID.to_owned(),
            content: response.to_owned(),
            delta: false,
        }),
    });
}
