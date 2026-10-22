//! 提交级 wire 事件幂等与序号单调闸门。
//!
//! Daemon durable stream 已在持久化发布边界保证顺序。TUI 在这里只做两件 O(1) 的事，
//! 不引入本地定时排序窗口、迟到缓存或额外延迟：
//! - 身份去重：同一 durable `(stream, event_id)` 只投影一次；
//! - 序号单调：每个 stream 维护已投影水位，序号不高于水位的迟到投递不再投影。
//!
//! 水位把「信赖 durable 发布边界顺序契约」从隐式假设变成在投影边界被强制校验的不变式：
//! 契约成立时（每个 stream 的 `sequence` 严格递增）行为与逐个投递完全一致；契约被破坏时
//! 迟到帧不会把展示层拉回旧状态。被拒绝的流式增量由 canonical 终态全量重放补偿。

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use super::super::AgentTuiRuntimeEvent;

/// 通过闸门、可以由展示层立即消费的事件。
#[derive(Debug)]
pub(super) struct OrderedWireEvent {
    pub(super) event: AgentTuiRuntimeEvent,
    pub(super) enqueued_at: Option<Instant>,
}

/// wire 事件的接纳结果。
#[derive(Debug)]
pub(super) enum WireAdmission {
    /// 首次到达且序号前进，可以立即投影。
    Deliver(OrderedWireEvent),
    /// 同一 durable `(stream, event_id)` 的重复投递。
    Duplicate,
    /// 序号不高于已投影水位的迟到投递；展示层已越过该帧。
    Stale,
}

/// 单个 durable stream 的已投影水位与已接纳身份。
#[derive(Debug, Default)]
struct StreamProjection {
    last_sequence: u64,
    admitted: HashSet<String>,
}

/// 按 durable 流身份记录已投影事件，并按 stream 强制序号单调。
#[derive(Debug, Default)]
pub(super) struct WireStreamGate {
    streams: HashMap<String, StreamProjection>,
}

impl WireStreamGate {
    pub(super) fn admit(
        &mut self,
        stream: &str,
        event_id: &str,
        sequence: u64,
        ordered: OrderedWireEvent,
    ) -> WireAdmission {
        let projection = self.streams.entry(stream.to_owned()).or_default();
        if !projection.admitted.insert(event_id.to_owned()) {
            return WireAdmission::Duplicate;
        }
        if sequence <= projection.last_sequence {
            return WireAdmission::Stale;
        }
        projection.last_sequence = sequence;
        WireAdmission::Deliver(ordered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::agent_tui::run_output::runtime_driver_event;
    use axon_protocol::wire::{
        WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase,
        WireLifecycleStatus, WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
    };

    fn ordered(sequence: u64, event_id: &str) -> OrderedWireEvent {
        OrderedWireEvent {
            event: runtime_driver_event(
                crate::r#loop::agent_tui::run_output::AgentTuiRuntimePayload::WireEvent {
                    event: Box::new(WireEventEnvelope {
                        schema_version: WireSchemaVersion::default(),
                        event_id: WireCorrelationId(event_id.to_owned()),
                        correlation_id: WireCorrelationId("turn".to_owned()),
                        session: WireSessionRef {
                            session_id: "session".to_owned(),
                            run_id: Some("run".to_owned()),
                            parent_session_id: None,
                        },
                        sequence,
                        recorded_at_ms: 1,
                        kind: WireEventKind::TurnLifecycle(WireTurnLifecycle {
                            part_id: format!("part:{sequence}"),
                            turn_id: "turn".to_owned(),
                            phase: WireLifecyclePhase::RegularTurn,
                            status: WireLifecycleStatus::Running,
                            title: "步骤".to_owned(),
                            kind: Some("Step".to_owned()),
                            ui_phase: None,
                            tool_name: None,
                            attempt_index: None,
                            summary: None,
                            duration_ms: None,
                            started_at_ms: None,
                            total_tokens: None,
                            cache_hit_rate_basis_points: None,
                        }),
                    }),
                },
            ),
            enqueued_at: None,
        }
    }

    #[test]
    fn ordered_events_are_delivered_immediately_without_reorder_window() {
        let mut gate = WireStreamGate::default();
        assert!(matches!(
            gate.admit("run", "event:1", 1, ordered(1, "event:1")),
            WireAdmission::Deliver(_)
        ));
        assert!(matches!(
            gate.admit("run", "event:2", 2, ordered(2, "event:2")),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn late_lower_sequence_is_rejected_after_a_higher_one_was_projected() {
        let mut gate = WireStreamGate::default();
        assert!(matches!(
            gate.admit("run", "event:2", 2, ordered(2, "event:2")),
            WireAdmission::Deliver(_)
        ));
        assert!(matches!(
            gate.admit("run", "event:1", 1, ordered(1, "event:1")),
            WireAdmission::Stale
        ));
        // 水位不回退：紧随其后的正常递增投递仍然通过。
        assert!(matches!(
            gate.admit("run", "event:3", 3, ordered(3, "event:3")),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn duplicate_identity_is_rejected_per_stream() {
        let mut gate = WireStreamGate::default();
        let _ = gate.admit("run-a", "event:1", 1, ordered(1, "event:1"));
        assert!(matches!(
            gate.admit("run-a", "event:1", 1, ordered(1, "event:1")),
            WireAdmission::Duplicate
        ));
        assert!(matches!(
            gate.admit("run-b", "event:1", 1, ordered(1, "event:1")),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn watermark_is_scoped_per_stream() {
        let mut gate = WireStreamGate::default();
        let _ = gate.admit("run-a", "event:9", 9, ordered(9, "event:9"));
        assert!(matches!(
            gate.admit("run-b", "event:1", 1, ordered(1, "event:1")),
            WireAdmission::Deliver(_)
        ));
    }
}
