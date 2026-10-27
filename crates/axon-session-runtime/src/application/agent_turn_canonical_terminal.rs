//! Canonical 终止事件的持久投影边界。
//!
//! 每一轮必须满足顺序契约：
//! `canonical assistant -> foreground/run completed -> turn_completed`。
//! 该批次是 durable 事实的收口，不允许降级为告警：提交失败必须向上返回，
//! 由调用方在缺口上终止本轮，禁止继续写入 `turn_completed`。

use crate::model::{RuntimeEvent, TurnExecutionError};
use crate::ports::TurnEventSink;
use axon_protocol::wire::{WireEventEnvelope, WireEventKind, WireLifecycleStatus};

const RUN_LIFECYCLE_KIND: &str = "RunLifecycle";

/// 按 wire 顺序原子提交本轮 canonical 终止事件。
pub(super) fn publish_canonical_terminal_events(
    sink: &dyn TurnEventSink,
    wire_events: &[WireEventEnvelope],
) -> Result<(), TurnExecutionError> {
    let events = canonical_terminal_events(wire_events);
    if events.is_empty() {
        return Ok(());
    }
    sink.publish_batch(&events).map_err(|error| {
        TurnExecutionError::permanent(format!(
            "canonical 终止事件持久投影失败，本轮已在缺口上终止：{error}"
        ))
    })
}

fn canonical_terminal_events(wire_events: &[WireEventEnvelope]) -> Vec<RuntimeEvent> {
    wire_events
        .iter()
        .filter(|event| is_canonical_terminal_event(event))
        .map(|event| {
            super::wire_event_to_runtime_event(event)
                .expect("canonical terminal WireEventEnvelope 序列化必须成功")
        })
        .collect()
}

fn is_canonical_terminal_event(event: &WireEventEnvelope) -> bool {
    matches!(&event.kind, WireEventKind::AssistantMessage(message) if !message.delta)
        || matches!(&event.kind,
            WireEventKind::TurnLifecycle(lifecycle)
                if lifecycle.status == WireLifecycleStatus::Completed
                    && lifecycle.kind.as_deref() == Some(RUN_LIFECYCLE_KIND)
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::wire::{
        WireAssistantMessage, WireCorrelationId, WireLifecyclePhase, WireSchemaVersion,
        WireSessionRef, WireTurnLifecycle,
    };

    fn envelope(event_id: &str, kind: WireEventKind) -> WireEventEnvelope {
        WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(event_id.to_owned()),
            correlation_id: WireCorrelationId("turn".to_owned()),
            session: WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            sequence: 1,
            recorded_at_ms: 1,
            kind,
        }
    }

    fn run_completed() -> WireEventEnvelope {
        envelope(
            "run-completed",
            WireEventKind::TurnLifecycle(WireTurnLifecycle {
                part_id: "lifecycle:run".to_owned(),
                turn_id: "run".to_owned(),
                phase: WireLifecyclePhase::RegularTurn,
                status: WireLifecycleStatus::Completed,
                title: "Run completed".to_owned(),
                kind: Some(RUN_LIFECYCLE_KIND.to_owned()),
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

    #[test]
    fn canonical_projection_keeps_assistant_before_run_completed() {
        let events = [
            envelope(
                "assistant-final",
                WireEventKind::AssistantMessage(WireAssistantMessage {
                    part_id: "assistant-part:1".to_owned(),
                    message_id: "assistant-message-final".to_owned(),
                    content: "最终回答".to_owned(),
                    delta: false,
                }),
            ),
            run_completed(),
        ];

        let projected = canonical_terminal_events(&events);

        assert_eq!(projected.len(), 2);
        assert!(projected[0].payload_json.contains("assistant-part:1"));
        assert!(projected[1].payload_json.contains("RunLifecycle"));
    }

    #[test]
    fn streaming_deltas_are_not_part_of_the_canonical_projection() {
        let events = [envelope(
            "assistant-delta",
            WireEventKind::AssistantMessage(WireAssistantMessage {
                part_id: "assistant-part:1".to_owned(),
                message_id: "assistant-message-final".to_owned(),
                content: "部分".to_owned(),
                delta: true,
            }),
        )];

        assert!(canonical_terminal_events(&events).is_empty());
    }
}
