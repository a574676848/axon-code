//! `--continue` 的恢复指针。
//!
//! 指针只回答一个问题：下一次不带 `--session` 的接续应该落到哪个会话。
//! 它是控制面的单行状态，与会话内容无关；桌面与 TUI 走显式 `session_id`，不读它。

use axon_domain::DaemonSessionId;

/// 恢复指针读写端口。
pub trait ResumePointerStore {
    /// 读取当前指针；从未设置时返回 `None`。
    fn read_resume_pointer(&self) -> Result<Option<DaemonSessionId>, ResumePointerStoreError>;

    /// 把指针指向某个会话。
    fn write_resume_pointer(
        &self,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<(), ResumePointerStoreError>;

    /// 清空指针。
    fn clear_resume_pointer(&self) -> Result<(), ResumePointerStoreError>;
}

/// 指针读写错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumePointerStoreError {
    /// 存储层错误。
    Storage(String),
}

impl std::fmt::Display for ResumePointerStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(reason) => write!(formatter, "恢复指针存储错误：{reason}"),
        }
    }
}

impl std::error::Error for ResumePointerStoreError {}
