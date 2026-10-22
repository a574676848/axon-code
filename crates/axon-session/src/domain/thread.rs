//! Thread 领域模型与只读视图。

use serde::{Deserialize, Serialize};

use super::{ThreadSettings, Turn};

/// Thread ID。
pub type ThreadId = String;

/// Thread 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadSource {
    /// CLI run 入口。
    CliRun,
    /// TUI 入口。
    Tui,
    /// 远控入口。
    Remote,
}

/// Thread 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadStatus {
    /// 未加载。
    NotLoaded,
    /// 空闲可继续。
    Idle,
    /// 正在运行。
    Active,
    /// 系统错误。
    SystemError,
    /// 已归档。
    Archived,
}

/// Thread 聚合。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 会话树根 ID。
    pub session_root_id: ThreadId,
    /// 父 thread ID。
    pub parent_thread_id: Option<ThreadId>,
    /// fork 来源 ID。
    pub forked_from_thread_id: Option<ThreadId>,
    /// Thread 名称。
    pub name: Option<String>,
    /// 当前状态。
    pub status: ThreadStatus,
    /// 来源。
    pub source: ThreadSource,
    /// 是否仅内存态。
    pub ephemeral: bool,
    /// 设置快照。
    pub settings: ThreadSettings,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 归档时间。
    pub archived_at_ms: Option<u64>,
}

/// Thread 摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    /// Thread ID。
    pub thread_id: ThreadId,
    /// 父 thread ID。
    pub parent_thread_id: Option<ThreadId>,
    /// fork 来源 ID。
    pub forked_from_thread_id: Option<ThreadId>,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// Thread 名称。
    pub name: Option<String>,
    /// Provider 名称。
    pub provider: String,
    /// 模型名称。
    pub model: String,
    /// 工作目录。
    pub cwd: String,
    /// 状态。
    pub status: ThreadStatus,
    /// Turn 数。
    pub turn_count: usize,
}

/// Thread 列表查询。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListQuery {
    /// 返回数量上限。
    pub limit: usize,
    /// 游标；格式由应用服务维护，不暴露存储路径。
    pub cursor: Option<String>,
    /// 工作目录过滤。
    pub cwd: Option<String>,
    /// 是否只返回已归档 thread。
    pub archived: Option<bool>,
}

/// Thread 列表页。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummaryPage {
    /// 总记录数，已应用过滤条件但未应用 limit。
    pub total_records: usize,
    /// 下一页游标。
    pub next_cursor: Option<String>,
    /// 当前页摘要。
    pub threads: Vec<ThreadSummary>,
}

/// Thread 详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDetail {
    /// Thread 聚合。
    pub thread: Thread,
    /// 可见 turns。
    pub turns: Vec<Turn>,
    /// 最新压缩后的模型历史。
    pub compacted_history: Option<ThreadCompaction>,
}

/// Thread 窗口轮转投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadCompaction {
    /// FreshWindow 的 OpenAI-compatible messages JSON。
    pub history_json: String,
    /// 此 checkpoint 已覆盖的最后一个 Turn sequence。
    pub through_sequence: u64,
    /// Session Notes 生成并通过完整性门禁的 canonical checkpoint。
    pub checkpoint_text: String,
    /// 换窗前 token 计量。
    pub before_tokens: u64,
    /// 换窗后 token 估算。
    pub after_tokens: u64,
    /// 记录时间。
    pub recorded_at_ms: u64,
}
