//! Daemon Host 的领域无关应用模型。

/// Host 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonHostStatus {
    /// 正常提供服务。
    Running,
    /// 已收到升级请求，不再接收会导致新 Provider turn 的 Action。
    UpgradeRequested,
    /// 等待 Worker 到达 checkpoint。
    AdmittingShutdown,
    /// 新 Host 可接管。
    HandoffReady,
    /// 正在停止。
    Stopping,
    /// 已停止。
    Stopped,
}

/// 升级 admission。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeAdmission {
    /// 当前 Host generation。
    pub host_generation: String,
    /// 一次性 handoff token。
    pub handoff_token: String,
    /// 最晚 checkpoint 时间。
    pub deadline_at_ms: u64,
}
