//! Scheduler 基础设施边界。

/// 调度循环节拍配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerLoopConfig {
    /// 两次扫描到期计划之间的间隔。
    pub poll_interval_ms: u64,
}
