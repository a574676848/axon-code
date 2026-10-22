//! Session Worker 状态。

/// Worker 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionWorkerState {
    /// 等待 Action。
    Idle,
    /// 正在执行 Action。
    Running,
    /// 正在从 checkpoint 恢复。
    Recovering,
    /// 已停止接收新 Action。
    Draining,
}
