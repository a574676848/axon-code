//! Daemon Host 周期调度结果契约。

/// 一次后台 supervisor tick 的统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostTickReport {
    /// 本轮恢复的 Session 数量。
    pub recovered: u32,
    /// 本轮因进程中断收敛为 uncertain 的 Execution 数量。
    pub quarantined: u32,
    /// 本轮因进程中断收敛为 interrupted 的 Interaction 数量。
    pub interrupted_interactions: u32,
    /// 本轮实际提交的 Action 数量。
    pub pumped: u32,
}
