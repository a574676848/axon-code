//! Action 调度投影。

use axon_domain::SessionActionId;

/// 一次已选择的 Action。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionDispatch {
    /// Action ID。
    pub action_id: SessionActionId,
    /// 选择时使用的策略优先级。
    pub effective_priority: i32,
}
