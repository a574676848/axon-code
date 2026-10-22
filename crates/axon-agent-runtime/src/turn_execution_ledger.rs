//! Turn 执行幂等账本。
//!
//! 所有运行事件、工具执行和渲染投影都通过同一套判定规则，resume/replay
//! 不再维护独立的去重路径。

use std::collections::{HashMap, HashSet};

/// 事件进入 reducer 前的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLedgerDecision {
    /// 首次接收。
    Accept,
    /// 相同 event_id 已处理。
    DuplicateEvent,
    /// sequence 早于当前流的最新序号。
    StaleSequence,
}

/// 工具进入执行器前的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLedgerDecision<R> {
    /// 可以执行。
    Execute,
    /// 已有回执，直接重放。
    Reuse(R),
    /// 相同 operation 正在另一个执行线程中运行。
    InFlight,
    /// 执行器拒绝当前调用。
    RejectUnchanged(String),
}

/// projection 的稳定身份。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectionKey {
    /// transcript part ID。
    pub part_id: String,
    /// projection 类型，例如 assistant、tool、final_receipt。
    pub kind: String,
}

/// 单个 turn 的事件、工具和 projection 账本。
#[derive(Debug, Clone)]
pub struct TurnExecutionLedger<R> {
    event_keys: HashSet<(String, String)>,
    latest_sequences: HashMap<String, u64>,
    tool_receipts: HashMap<String, R>,
    operation_receipts: HashMap<String, R>,
    in_flight_operations: HashSet<String>,
    settled_projections: HashSet<ProjectionKey>,
}

impl<R> Default for TurnExecutionLedger<R> {
    fn default() -> Self {
        Self {
            event_keys: HashSet::new(),
            latest_sequences: HashMap::new(),
            tool_receipts: HashMap::new(),
            operation_receipts: HashMap::new(),
            in_flight_operations: HashSet::new(),
            settled_projections: HashSet::new(),
        }
    }
}

impl<R: Clone> TurnExecutionLedger<R> {
    /// 接收一个 wire/runtime 事件。
    pub fn accept_event(
        &mut self,
        event_id: impl Into<String>,
        stream: impl Into<String>,
        sequence: u64,
    ) -> EventLedgerDecision {
        let event_id = event_id.into();
        let stream = stream.into();
        let event_key = (stream.clone(), event_id);
        if self.event_keys.contains(&event_key) {
            return EventLedgerDecision::DuplicateEvent;
        }
        if let Some(latest) = self.latest_sequences.get(&stream)
            && sequence < *latest
        {
            // 被拒绝的事件不能抢占幂等身份；订阅层补投同一事件时必须仍可判定。
            return EventLedgerDecision::StaleSequence;
        }
        self.event_keys.insert(event_key);
        self.latest_sequences.insert(stream, sequence);
        EventLedgerDecision::Accept
    }

    /// 只按事件身份去重，不解释上游 envelope 的局部 sequence。
    ///
    /// durable session sequence 由持久化发布边界统一分配；客户端 adapter
    /// 不应使用 Provider/wire 自带的局部序号做迟到过滤。
    pub fn accept_event_identity(
        &mut self,
        event_id: impl Into<String>,
        stream: impl Into<String>,
    ) -> EventLedgerDecision {
        let event_id = event_id.into();
        let stream = stream.into();
        let event_key = (stream, event_id);
        if self.event_keys.contains(&event_key) {
            return EventLedgerDecision::DuplicateEvent;
        }
        self.event_keys.insert(event_key);
        EventLedgerDecision::Accept
    }

    /// 工具调用进入执行前的幂等判定。
    pub fn begin_tool(
        &mut self,
        tool_call_id: impl Into<String>,
        operation_id: impl Into<String>,
        _fingerprint: impl Into<String>,
        _workspace_revision: u64,
        _turn_id: impl Into<String>,
    ) -> ToolLedgerDecision<R> {
        let tool_call_id = tool_call_id.into();
        let operation_id = operation_id.into();
        if let Some(receipt) = self.tool_receipts.get(&tool_call_id) {
            return ToolLedgerDecision::Reuse(receipt.clone());
        }
        if let Some(receipt) = self.operation_receipts.get(&operation_id) {
            return ToolLedgerDecision::Reuse(receipt.clone());
        }
        if self.in_flight_operations.contains(&operation_id) {
            return ToolLedgerDecision::InFlight;
        }
        self.in_flight_operations.insert(operation_id);
        ToolLedgerDecision::Execute
    }

    /// 保存工具最终回执。
    pub fn settle_tool(
        &mut self,
        tool_call_id: impl Into<String>,
        operation_id: impl Into<String>,
        receipt: R,
    ) {
        let tool_call_id = tool_call_id.into();
        let operation_id = operation_id.into();
        self.tool_receipts.insert(tool_call_id, receipt.clone());
        self.operation_receipts
            .insert(operation_id.clone(), receipt);
        self.in_flight_operations.remove(&operation_id);
    }

    /// 从持久化历史恢复一个尚未结算的 operation。
    pub fn restore_in_flight(&mut self, operation_id: impl Into<String>) {
        self.in_flight_operations.insert(operation_id.into());
    }

    /// 从持久化历史恢复一个已结算的工具回执。
    pub fn restore_settled_tool(
        &mut self,
        tool_call_id: impl Into<String>,
        operation_id: impl Into<String>,
        receipt: R,
    ) {
        self.settle_tool(tool_call_id, operation_id, receipt);
    }

    /// final receipt 仅允许 settle 一次。
    pub fn settle_projection_once(
        &mut self,
        part_id: impl Into<String>,
        kind: impl Into<String>,
    ) -> bool {
        self.settled_projections.insert(ProjectionKey {
            part_id: part_id.into(),
            kind: kind.into(),
        })
    }

    /// 判断 projection 是否已经完成结算。
    pub fn projection_is_settled(&self, part_id: &str, kind: &str) -> bool {
        self.settled_projections.contains(&ProjectionKey {
            part_id: part_id.to_owned(),
            kind: kind.to_owned(),
        })
    }

    /// 当前账本是否已记录事件流中的事件。
    pub fn contains_event(&self, stream: &str, event_id: &str) -> bool {
        self.event_keys
            .contains(&(stream.to_owned(), event_id.to_owned()))
    }

    /// 从 resume snapshot 预置一个事件流的最新序号。
    pub fn seed_sequence(&mut self, stream: impl Into<String>, sequence: u64) {
        self.latest_sequences
            .entry(stream.into())
            .and_modify(|current| *current = (*current).max(sequence))
            .or_insert(sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_event_and_stale_sequence_are_rejected() {
        let mut ledger = TurnExecutionLedger::<()>::default();
        assert_eq!(
            ledger.accept_event("event-1", "run", 2),
            EventLedgerDecision::Accept
        );
        assert_eq!(
            ledger.accept_event("event-1", "run", 2),
            EventLedgerDecision::DuplicateEvent
        );
        assert_eq!(
            ledger.accept_event("event-2", "run", 1),
            EventLedgerDecision::StaleSequence
        );
        assert_eq!(
            ledger.accept_event("event-2", "run", 1),
            EventLedgerDecision::StaleSequence
        );
        assert_eq!(
            ledger.accept_event("event-2", "another-run", 1),
            EventLedgerDecision::Accept
        );
    }

    #[test]
    fn identity_admission_does_not_reject_late_wire_sequence() {
        let mut ledger = TurnExecutionLedger::<()>::default();
        assert_eq!(
            ledger.accept_event_identity("event-2", "wire:session:run"),
            EventLedgerDecision::Accept
        );
        assert_eq!(
            ledger.accept_event_identity("event-1", "wire:session:run"),
            EventLedgerDecision::Accept
        );
        assert_eq!(
            ledger.accept_event_identity("event-1", "wire:session:run"),
            EventLedgerDecision::DuplicateEvent
        );
    }

    #[test]
    fn tool_call_replay_returns_original_receipt() {
        let mut ledger = TurnExecutionLedger::default();
        assert_eq!(
            ledger.begin_tool("call-1", "op-1", "read:a", 1, "turn-1"),
            ToolLedgerDecision::Execute
        );
        ledger.settle_tool("call-1", "op-1", "receipt");
        assert_eq!(
            ledger.begin_tool("call-1", "op-1", "read:a", 1, "turn-1"),
            ToolLedgerDecision::Reuse("receipt")
        );
        assert_eq!(
            ledger.begin_tool("call-2", "op-1", "read:a", 1, "turn-1"),
            ToolLedgerDecision::Reuse("receipt")
        );
    }

    #[test]
    fn same_operation_is_marked_in_flight_until_receipt_settles() {
        let mut ledger = TurnExecutionLedger::<&str>::default();
        assert_eq!(
            ledger.begin_tool("call-1", "op-1", "shell:echo", 1, "turn-1"),
            ToolLedgerDecision::Execute
        );
        assert_eq!(
            ledger.begin_tool("call-1", "op-1", "shell:echo", 1, "turn-1"),
            ToolLedgerDecision::InFlight
        );
        ledger.settle_tool("call-1", "op-1", "done");
        assert_eq!(
            ledger.begin_tool("call-1", "op-1", "shell:echo", 1, "turn-1"),
            ToolLedgerDecision::Reuse("done")
        );
    }

    #[test]
    fn persisted_in_flight_and_receipt_restore_keep_idempotency() {
        let mut ledger = TurnExecutionLedger::default();
        ledger.restore_in_flight("op-running");
        assert_eq!(
            ledger.begin_tool("call-running", "op-running", "read:a", 1, "turn-1"),
            ToolLedgerDecision::InFlight
        );

        ledger.restore_settled_tool("call-done", "op-done", "receipt");
        assert_eq!(
            ledger.begin_tool("call-retry", "op-done", "read:b", 1, "turn-2"),
            ToolLedgerDecision::Reuse("receipt")
        );
    }

    #[test]
    fn distinct_call_ids_can_execute_in_same_or_later_turns() {
        let mut ledger = TurnExecutionLedger::<()>::default();
        assert_eq!(
            ledger.begin_tool("call-1", "op-1", "read:a", 1, "turn-1"),
            ToolLedgerDecision::Execute
        );
        assert_eq!(
            ledger.begin_tool("call-2", "op-2", "read:a", 1, "turn-1"),
            ToolLedgerDecision::Execute
        );
        assert_eq!(
            ledger.begin_tool("call-3", "op-3", "read:a", 1, "turn-2"),
            ToolLedgerDecision::Execute
        );
    }

    #[test]
    fn projection_settles_once_per_part_and_kind() {
        let mut ledger = TurnExecutionLedger::<()>::default();
        assert!(!ledger.projection_is_settled("part-1", "final_receipt"));
        assert!(ledger.settle_projection_once("part-1", "final_receipt"));
        assert!(ledger.projection_is_settled("part-1", "final_receipt"));
        assert!(!ledger.settle_projection_once("part-1", "final_receipt"));
        assert!(ledger.settle_projection_once("part-1", "tool"));
    }
}
