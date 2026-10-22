//! Turn metadata 和上下文跟踪。
//!
//! 对齐 Codex turn_metadata.rs，提供完整的 turn 元数据袋，
//! 包括 session/thread/workspace/git 等上下文信息。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Turn 请求类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRequestKind {
    /// 正常 turn 请求。
    Turn,
    /// 预热请求。
    Prewarm,
    /// 压缩请求。
    Compaction,
    /// 记忆请求。
    Memory,
}

/// Workspace Git 元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkspaceGitMetadata {
    /// 关联的远程仓库 URLs。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub associated_remote_urls: Option<BTreeMap<String, String>>,
    /// 最新 Git 提交哈希。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_git_commit_hash: Option<String>,
    /// 是否有未提交的更改。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_changes: Option<bool>,
}

/// Turn metadata 核心袋，携带 turn 身份和上下文信息。
///
/// 对齐 Codex 的 TurnMetadataBag，用于在 provider 请求头中传递
/// session/thread/fork/subagent 血缘关系、workspace 状态等。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnMetadataBag {
    /// 请求类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_kind: Option<TurnRequestKind>,

    /// Session ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,

    /// Thread ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,

    /// Fork 来源 thread ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_thread_id: Option<String>,

    /// 父 thread ID（subagent 场景）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,

    /// Subagent 类型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_kind: Option<String>,

    /// Turn ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,

    /// Turn 开始时间戳（Unix 毫秒）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_started_at_unix_ms: Option<u64>,

    /// 模型名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Workspace Git 元数据（按 workspace 路径索引）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workspaces: BTreeMap<String, WorkspaceGitMetadata>,
}

impl Default for TurnMetadataBag {
    fn default() -> Self {
        Self {
            request_kind: Some(TurnRequestKind::Turn),
            session_id: None,
            thread_id: None,
            forked_from_thread_id: None,
            parent_thread_id: None,
            subagent_kind: None,
            turn_id: None,
            turn_started_at_unix_ms: None,
            model: None,
            workspaces: BTreeMap::new(),
        }
    }
}

impl TurnMetadataBag {
    /// 创建新的 turn metadata 袋。
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 session ID。
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// 设置 turn ID。
    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    /// 设置模型名称。
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// 设置 turn 开始时间戳。
    pub fn with_started_at_ms(mut self, started_at_ms: u64) -> Self {
        self.turn_started_at_unix_ms = Some(started_at_ms);
        self
    }

    /// 添加 workspace Git 元数据。
    pub fn with_workspace_git(
        mut self,
        workspace_path: impl Into<String>,
        metadata: WorkspaceGitMetadata,
    ) -> Self {
        self.workspaces.insert(workspace_path.into(), metadata);
        self
    }

    /// 序列化为 JSON 字符串，用于 HTTP 请求头。
    pub fn to_header_value(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}
