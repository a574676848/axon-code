//! Host 状态投影。

/// Host 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostStatus {
    /// 正常运行。
    Running,
    /// 已请求升级。
    UpgradeRequested,
    /// 正在排空连接与 Worker。
    AdmittingShutdown,
    /// 已完成交接 checkpoint。
    HandoffReady,
    /// 已停止。
    Stopped,
}
