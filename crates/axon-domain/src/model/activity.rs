use serde::{Deserialize, Serialize};

use super::{GoalRecord, JarvisEvent, ResultRecord, ScopeRef};

/// Activity 查询参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityQuery {
    /// 查询作用域。
    pub scope: ScopeRef,
    /// 最多读取的 active Goal 数量。
    pub goal_limit: u32,
    /// 最多读取的未确认 Result 数量。
    pub result_limit: u32,
    /// 只读取该全局 Event sequence 之后的记录。
    pub event_after_sequence: u64,
    /// 最多读取的 Event 数量。
    pub event_limit: u32,
}

/// Activity 查询结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySnapshot {
    /// 当前作用域的非终态 Goal。
    pub active_goals: Vec<GoalRecord>,
    /// 当前作用域尚未确认的 Result。
    pub unread_results: Vec<ResultRecord>,
    /// 全局 Event sequence 窗口。
    pub events: Vec<JarvisEvent>,
}
