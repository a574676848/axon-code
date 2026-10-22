//! Session Runtime 事件领域值。

/// 事件类型的稳定名称。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEventType(pub String);

impl SessionEventType {
    /// 恢复事件。
    #[must_use]
    pub fn recovered() -> Self {
        Self("session_recovered".to_owned())
    }
}
