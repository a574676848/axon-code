//! Workspace 注册领域实体。

use serde::{Deserialize, Serialize};

/// Workspace 注册信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    /// Workspace ID。
    pub workspace_id: String,
    /// 显示名称。
    pub display_name: String,
    /// 工作区根目录。
    pub workspace_root: String,
    /// Git remote。
    pub git_remote: Option<String>,
    /// 默认分支。
    pub default_branch: Option<String>,
    /// 目录别名。
    pub aliases: Vec<String>,
    /// Memory namespace。
    pub memory_namespace: String,
    /// 是否固定保留。
    pub pinned: bool,
    /// 最近访问时间。
    pub last_seen_at_ms: u64,
}
