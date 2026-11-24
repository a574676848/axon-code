/// 渠道熔断策略：连续失败阈值与冷却窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerPolicy {
    /// 连续失败阈值（默认 3）。
    pub failure_threshold: u32,
    /// 冷却窗口（默认 30_000 毫秒）。
    pub cooldown_ms: u64,
}
