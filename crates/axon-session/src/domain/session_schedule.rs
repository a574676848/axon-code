//! Session Schedule 领域值。

/// 调度投递模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScheduleDelivery {
    /// 普通 follow-up。
    FollowUp,
    /// 当前执行安全边界处理。
    Steer,
}
