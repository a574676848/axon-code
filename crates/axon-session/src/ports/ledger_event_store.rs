//! Work Ledger Jarvis Event 持久化端口。

use axon_domain::{GoalId, JarvisEvent};
use std::fmt;

/// Event 存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerEventStoreError {
    /// 幂等键、sequence 或载荷冲突。
    Conflict(String),
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for LedgerEventStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(detail) => write!(formatter, "Jarvis Event 冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Jarvis Event 存储失败：{detail}"),
        }
    }
}

impl std::error::Error for LedgerEventStoreError {}

/// Work Ledger Event 持久化端口。
pub trait LedgerEventStore: Send + Sync {
    /// 幂等追加 Event；重复且内容一致时返回 false。
    fn append_ledger_event(&self, event: &JarvisEvent) -> Result<bool, LedgerEventStoreError>;

    /// 读取 sequence 之后的 Event，不修改业务状态。
    fn replay_ledger_events(
        &self,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<JarvisEvent>, LedgerEventStoreError>;

    /// 按 Goal ID 读取有界 Event；实现应使用结构化索引避免全局回放。
    fn list_events_for_goal(
        &self,
        goal_id: &GoalId,
        limit: u32,
    ) -> Result<Vec<JarvisEvent>, LedgerEventStoreError> {
        let events = self.replay_ledger_events(0, limit)?;
        Ok(events
            .into_iter()
            .filter(|event| {
                serde_json::from_str::<serde_json::Value>(&event.payload_json)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("goal_id")
                            .or_else(|| value.get("goalId"))
                            .and_then(serde_json::Value::as_str)
                            .map(|value| value == goal_id.as_str())
                    })
                    .unwrap_or(false)
            })
            .collect())
    }
}
