//! Daemon Host 错误契约。

use axon_session::RuntimeStoreError;
use thiserror::Error;

/// Host 应用服务失败。
#[derive(Debug, Error)]
pub enum DaemonHostError {
    /// Runtime Store 失败。
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    /// Host 尚未启动。
    #[error("Daemon Host 尚未启动")]
    NotStarted,
    /// Host 已进入排空。
    #[error("Daemon Host 正在排空，不再接收新 Action")]
    Draining,
    /// Session Worker 失败。
    #[error("Session Worker 失败：{0}")]
    Worker(String),
}

impl DaemonHostError {
    /// 判断错误是否表示当前 Host generation 已失效。
    #[must_use]
    pub fn is_fenced(&self) -> bool {
        matches!(self, Self::Store(RuntimeStoreError::Fenced))
    }
}
