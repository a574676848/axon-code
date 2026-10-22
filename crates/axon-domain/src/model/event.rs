//! JARVIS 唯一 Event envelope。

use super::JarvisEventId;
use serde::{Deserialize, Serialize};

/// 可重放的 JARVIS Event。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JarvisEvent {
    /// 事件标识。
    pub event_id: JarvisEventId,
    /// 单调递增序号。
    pub sequence: u64,
    /// 事件类型。
    pub event_type: String,
    /// 幂等键。
    pub idempotency_key: String,
    /// 因果来源事件。
    pub cause_event_id: Option<JarvisEventId>,
    /// 已结构化编码的事件载荷。
    pub payload_json: String,
    /// 发生时间。
    pub occurred_at_ms: u64,
}

impl JarvisEvent {
    /// 判断两个事件是否具有相同幂等身份。
    #[must_use]
    pub fn same_idempotency_key(&self, other: &Self) -> bool {
        self.idempotency_key == other.idempotency_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_idempotency_is_independent_of_sequence() {
        let first = JarvisEvent {
            event_id: JarvisEventId::new("event-1").expect("ID"),
            sequence: 1,
            event_type: "goal.created".to_owned(),
            idempotency_key: "goal-1-created".to_owned(),
            cause_event_id: None,
            payload_json: "{}".to_owned(),
            occurred_at_ms: 1,
        };
        let second = JarvisEvent {
            sequence: 2,
            ..first.clone()
        };
        assert!(first.same_idempotency_key(&second));
    }
}
