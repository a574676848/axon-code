//! Workspace registry 的持久化端口。

use crate::WorkspaceRecord;
use axon_domain::WorkspaceId;
use std::fmt;

/// Workspace registry 存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceStoreError {
    /// 相同 Workspace ID 已绑定不同身份。
    Conflict(String),
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for WorkspaceStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(detail) => write!(formatter, "Workspace 注册冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Workspace 注册失败：{detail}"),
        }
    }
}

impl std::error::Error for WorkspaceStoreError {}

/// Workspace ID 到真实根目录的持久映射。
pub trait WorkspaceStore: Send + Sync {
    /// 幂等注册或刷新 Workspace。
    fn register_workspace(&self, workspace: &WorkspaceRecord) -> Result<(), WorkspaceStoreError>;

    /// 按稳定 ID 读取 Workspace。
    fn get_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<Option<WorkspaceRecord>, WorkspaceStoreError>;

    /// 列出注册表中全部 Workspace，按最近访问时间降序。
    fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, WorkspaceStoreError>;
}
