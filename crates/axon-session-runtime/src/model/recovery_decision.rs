//! Session 恢复决策。

/// lease 失效后的恢复动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDecision {
    /// 从最后 checkpoint 继续。
    ResumeFromCheckpoint,
    /// 无可恢复 Action，回到空闲。
    MarkIdle,
}
