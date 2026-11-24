//! Token Budget 监控与代际窗口轮转 Wire 事件。

use serde::{Deserialize, Serialize};

/// Token budget 临界预警事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireTokenBudgetReminder {
    /// 触发预警的 turn ID。
    pub turn_id: u64,
    /// 当前活跃上下文剩余估算 token 数。
    pub tokens_remaining: u64,
    /// 注入模型的提示内容。
    pub reminder_message: String,
}

/// 上下文代际窗口轮转事件（Rollover）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireContextRollover {
    /// 新代际窗口序号（从 1 开始单调递增）。
    pub window_number: u64,
    /// 新代际窗口全局唯一 ID。
    pub window_id: String,
    /// 上一个代际窗口 ID（首个窗口为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_window_id: Option<String>,
    /// 归档至冷存的历史消息条数。
    pub archived_messages_count: usize,
    /// 是否在新窗口首部注入了战术笔记。
    pub notes_injected: bool,
}
