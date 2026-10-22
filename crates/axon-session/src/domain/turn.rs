//! Turn 领域模型。

use serde::{Deserialize, Serialize};

use axon_domain::RuntimeFailure;

use super::ThreadHistoryItem;

/// Turn ID。
pub type TurnId = String;

/// Turn 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    /// 运行中。
    Active,
    /// 已完成。
    Completed,
    /// 已被中断。
    Interrupted,
    /// 执行失败。
    Failed,
}

/// 一次用户输入到 Agent 响应的完整边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turn {
    /// Turn ID。
    pub turn_id: TurnId,
    /// 顺序号。
    pub sequence: u64,
    /// 状态。
    pub status: TurnStatus,
    /// 按 provider-neutral 结构持久化的完整历史。
    pub history_items: Vec<ThreadHistoryItem>,
    /// token usage JSON 快照。
    pub token_usage_json: Option<String>,
    /// 开始时间。
    pub started_at_ms: u64,
    /// 完成时间。
    pub completed_at_ms: Option<u64>,
    /// 中断时间。
    pub interrupted_at_ms: Option<u64>,
    /// 失败事实；仅失败 turn 存在。
    pub failure: Option<RuntimeFailure>,
}
