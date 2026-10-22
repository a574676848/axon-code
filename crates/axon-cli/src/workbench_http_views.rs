//! 工作台协议视图：把领域记录摊平成小驼峰 JSON 载荷。
//!
//! 从 `workbench_http_adapter` 拆出：这些类型是**对外协议**，字段名一旦发布就不能随领域类型改名，
//! 因此它们值得单独成文，免得协议与路由逻辑混在一起被顺手重构掉。

use super::*;

/// 工作区协议载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    /// Workspace 标识。
    pub workspace_id: String,
    /// 显示名称。
    pub display_name: String,
    /// 工作区根目录。
    pub workspace_root: String,
    /// Git remote。
    pub git_remote: Option<String>,
    /// 默认分支。
    pub default_branch: Option<String>,
    /// 是否固定保留。
    pub pinned: bool,
    /// 最近访问时间（毫秒）。
    pub last_seen_at_ms: u64,
}

impl From<&WorkspaceRecord> for WorkspaceView {
    fn from(record: &WorkspaceRecord) -> Self {
        Self {
            workspace_id: record.workspace_id.clone(),
            display_name: record.display_name.clone(),
            workspace_root: record.workspace_root.clone(),
            git_remote: record.git_remote.clone(),
            default_branch: record.default_branch.clone(),
            pinned: record.pinned,
            last_seen_at_ms: record.last_seen_at_ms,
        }
    }
}

/// 车道协议载荷。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneView {
    /// 车道标识。
    pub lane_id: String,
    /// 所属 Workspace 标识。
    pub workspace_id: String,
    /// 会话表面渠道。
    pub channel: String,
    /// 会话表面目标。
    pub destination: String,
    /// 受众边界标签。
    pub audience: String,
    /// 车道标题。
    pub title: String,
    /// Interaction 总数。
    pub interaction_count: u64,
    /// 是否有一条 Interaction 正在处理中。
    pub running: bool,
    /// 最早活动时间（毫秒）。
    pub created_at_ms: u64,
    /// 最近活动时间（毫秒）。
    pub updated_at_ms: u64,
    /// 待答提问数量（pending 或 suspended 状态）。
    pub pending_question_count: u64,
}

impl From<&LaneSnapshot> for LaneView {
    fn from(snapshot: &LaneSnapshot) -> Self {
        Self {
            lane_id: snapshot.lane_id.clone(),
            workspace_id: snapshot.workspace_id.clone(),
            channel: snapshot.channel.clone(),
            destination: snapshot.destination.clone(),
            audience: snapshot.audience.clone(),
            title: snapshot.title.clone(),
            interaction_count: snapshot.interaction_count,
            running: snapshot.running,
            created_at_ms: snapshot.created_at_ms,
            updated_at_ms: snapshot.updated_at_ms,
            pending_question_count: 0,
        }
    }
}

/// 车道内一条 Interaction 的协议载荷。
///
/// 只透出回放所需的最小事实：稳定标识、状态与接收时间。正文不走这里，
/// 它由会话事件订阅流按 `part_id` 归并还原，避免同一份正文出现两条口径。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionView {
    /// Interaction 标识。
    pub interaction_id: String,
    /// 处理状态。
    pub status: String,
    /// 来源渠道。
    pub channel: String,
    /// 接收时间（毫秒）。
    pub received_at_ms: u64,
    /// 入站正文。会话事件流不含用户消息事件，回放时用户气泡只能由这里还原。
    pub content: String,
    /// 入站时落地的本地附件绝对路径；回放历史时据此还原图片与文件。
    pub attachments: Vec<String>,
}
