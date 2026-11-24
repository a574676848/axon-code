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
//!
//! 存量乱序日志有一个例外：终态语义帧（canonical 正文快照与本轮终态生命周期）即使序号
//! 低于水位也照样采纳。这两类帧各自是一条完整事实，重新投影是幂等的；把它们当迟到增量
//! 丢弃，会让整段收口正文或整轮结论从展示层消失，而这正是 WP-01 之前写入侧留下的常见
//! 脏日志形态。取舍是「宁可让迟到的终态覆盖一次展示，也不让整段收口消失」：收口那一步
//! 还会按 durable 顺序重放一次终态投影，因此同时到的旧正文快照会被正确版本覆盖回来。
//! 放行同时单独计数，丢帧数与「被接受的迟到终态」因此都可见。

use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

use axon_protocol::wire::WireEventKind;

use super::super::{AgentTuiRuntimeEvent, AgentTuiRuntimePayload};

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
    /// 序号不高于水位，但帧本身是终态语义事实：非单调也接受，并单独计数。
    ///
    /// 只用于 canonical 正文快照与本轮终态生命周期。二者都是完整事实，丢弃会让这一段
    /// 收口永久消失；接受则最多让旧版本正文短暂覆盖一次，收口时的 durable 重放会纠正。
    LateTerminal(OrderedWireEvent),
}

/// 判断帧是否为「非单调也要接受」的终态语义事实。
///
/// 判据只认 wire 载荷本身，不看到达顺序：非增量的 assistant 消息是本轮正文的完整快照，
/// `RunLifecycle` 的终态是整个回合的结论。其余帧（增量、工具、计划、用量等）仍然严格
/// 受水位约束。
pub(super) fn is_terminal_semantic_frame(event: &AgentTuiRuntimeEvent) -> bool {
    let AgentTuiRuntimePayload::WireEvent { event, .. } = &event.payload else {
        return false;
    };
    match &event.kind {
        WireEventKind::TurnLifecycle(lifecycle) => lifecycle.is_terminal_run(),
        WireEventKind::AssistantMessage(message) => !message.delta,
        _ => false,
    }
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
        late_terminal: bool,
    ) -> WireAdmission {
        let projection = self.streams.entry(stream.to_owned()).or_default();
        if !projection.admitted.insert(event_id.to_owned()) {
            return WireAdmission::Duplicate;
        }
        if sequence <= projection.last_sequence {
            if !late_terminal {
                return WireAdmission::Stale;
            }
            // 水位不因迟到终态回退：后续仍按已见最大序号判定单调。
            projection.last_sequence = projection.last_sequence.max(sequence);
            return WireAdmission::LateTerminal(ordered);
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

                            warnings: Vec::new(),
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
            gate.admit("run", "event:1", 1, ordered(1, "event:1"), false),
            WireAdmission::Deliver(_)
        ));
        assert!(matches!(
            gate.admit("run", "event:2", 2, ordered(2, "event:2"), false),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn late_lower_sequence_is_rejected_after_a_higher_one_was_projected() {
        let mut gate = WireStreamGate::default();
        assert!(matches!(
            gate.admit("run", "event:2", 2, ordered(2, "event:2"), false),
            WireAdmission::Deliver(_)
        ));
        assert!(matches!(
            gate.admit("run", "event:1", 1, ordered(1, "event:1"), false),
            WireAdmission::Stale
        ));
        // 水位不回退：紧随其后的正常递增投递仍然通过。
        assert!(matches!(
            gate.admit("run", "event:3", 3, ordered(3, "event:3"), false),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn duplicate_identity_is_rejected_per_stream() {
        let mut gate = WireStreamGate::default();
        let _ = gate.admit("run-a", "event:1", 1, ordered(1, "event:1"), false);
        assert!(matches!(
            gate.admit("run-a", "event:1", 1, ordered(1, "event:1"), false),
            WireAdmission::Duplicate
        ));
        assert!(matches!(
            gate.admit("run-b", "event:1", 1, ordered(1, "event:1"), false),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn watermark_is_scoped_per_stream() {
        let mut gate = WireStreamGate::default();
        let _ = gate.admit("run-a", "event:9", 9, ordered(9, "event:9"), false);
        assert!(matches!(
            gate.admit("run-b", "event:1", 1, ordered(1, "event:1"), false),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn late_terminal_frame_is_accepted_without_moving_watermark_backwards() {
        let mut gate = WireStreamGate::default();
        assert!(matches!(
            gate.admit("run", "event:5", 5, ordered(5, "event:5"), false),
            WireAdmission::Deliver(_)
        ));
        // 终态语义帧：序号低于水位也放行，且水位停在已见最大值。
        assert!(matches!(
            gate.admit("run", "event:2", 2, ordered(2, "event:2"), true),
            WireAdmission::LateTerminal(_)
        ));
        // 水位没有回退：紧随其后的普通帧仍按 5 判。
        assert!(matches!(
            gate.admit("run", "event:4", 4, ordered(4, "event:4"), false),
            WireAdmission::Stale
        ));
        assert!(matches!(
            gate.admit("run", "event:6", 6, ordered(6, "event:6"), false),
            WireAdmission::Deliver(_)
        ));
    }

    #[test]
    fn only_terminal_semantic_frames_qualify_for_late_acceptance() {
        use axon_protocol::wire::{
            WireAssistantMessage, WireCorrelationId as Id, WireEventEnvelope as Envelope,
            WireEventKind as Kind, WireSchemaVersion as Version, WireSessionRef as SessionRef,
        };

        fn envelope(kind: Kind) -> AgentTuiRuntimeEvent {
            crate::r#loop::agent_tui::run_output::runtime_driver_event(
                crate::r#loop::agent_tui::run_output::AgentTuiRuntimePayload::WireEvent {
                    event: Box::new(Envelope {
                        schema_version: Version::default(),
                        event_id: Id("event".to_owned()),
                        correlation_id: Id("turn".to_owned()),
                        session: SessionRef {
                            session_id: "session".to_owned(),
                            run_id: Some("run".to_owned()),
                            parent_session_id: None,
                        },
                        sequence: 1,
                        recorded_at_ms: 1,
                        kind,
                    }),
                },
            )
        }

        fn terminal_lifecycle() -> WireTurnLifecycle {
            WireTurnLifecycle {
                part_id: "part:run".to_owned(),
                turn_id: "turn".to_owned(),
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
            }
        }

        fn assistant(content: &str, delta: bool) -> Kind {
            Kind::AssistantMessage(WireAssistantMessage {
                part_id: "assistant:1".to_owned(),
                message_id: "message:1".to_owned(),
                content: content.to_owned(),
                delta,
            })
        }

        assert!(is_terminal_semantic_frame(&envelope(Kind::TurnLifecycle(
            terminal_lifecycle()
        ))));
        assert!(is_terminal_semantic_frame(&envelope(assistant(
            "终态正文",
            false
        ))));
        // 运行中的生命周期与流式增量都还是可重建增量，不在放行之列。
        assert!(!is_terminal_semantic_frame(&envelope(assistant(
            "增量", true
        ))));
        let mut running = terminal_lifecycle();
        running.status = WireLifecycleStatus::Running;
        assert!(!is_terminal_semantic_frame(&envelope(Kind::TurnLifecycle(
            running
        ))));
    }
}
