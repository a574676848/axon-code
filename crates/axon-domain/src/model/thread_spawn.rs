//! Thread spawn 拓扑边状态与节点定义。
//!
//! 参考 codex-rs `agent-graph-store/types.rs` ThreadSpawnEdgeStatus 实现。

/// Thread spawn 边的状态。
///
/// 表示父子线程之间的 spawn 关系状态：
/// - `Open`：子线程仍在运行中，父线程可向其发送消息或等待其完成
/// - `Closed`：子线程已结束（成功/失败/被中断）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThreadSpawnEdgeStatus {
    /// 子线程仍在运行，父子之间可通信
    #[default]
    Open,
    /// 子线程已结束，不再接受新消息
    Closed,
}

impl ThreadSpawnEdgeStatus {
    /// 判定边是否仍活跃。
    #[inline]
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }

    /// 判定边是否已终结。
    #[inline]
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// Thread spawn 边的元数据。
#[derive(Debug, Clone)]
pub struct ThreadSpawnEdge {
    /// 父线程 ID
    pub parent_id: u64,
    /// 子线程 ID
    pub child_id: u64,
    /// 边的状态
    pub status: ThreadSpawnEdgeStatus,
    /// 创建时间戳（毫秒）
    pub created_at_ms: u64,
    /// 关闭时间戳（毫秒），仅在 `status == Closed` 时有值
    pub closed_at_ms: Option<u64>,
}

impl ThreadSpawnEdge {
    /// 构造新边，状态默认为 `Open`。
    #[must_use]
    pub fn new(parent_id: u64, child_id: u64, created_at_ms: u64) -> Self {
        Self {
            parent_id,
            child_id,
            status: ThreadSpawnEdgeStatus::Open,
            created_at_ms,
            closed_at_ms: None,
        }
    }

    /// 关闭此边，设置关闭时间戳。
    pub fn close(&mut self, closed_at_ms: u64) {
        self.status = ThreadSpawnEdgeStatus::Closed;
        self.closed_at_ms = Some(closed_at_ms);
    }
}

/// Thread spawn 节点（线程）元数据。
#[derive(Debug, Clone)]
pub struct ThreadSpawnNode {
    /// 线程 ID
    pub id: u64,
    /// 线程 nickname（来自 AgentRegistry nickname pool）
    pub nickname: String,
    /// 规范代理路径，例如 `/root/review/security`
    pub agent_path: Option<String>,
    /// 工作目录路径
    pub cwd: String,
    /// 深度（根线程 depth=0，每嵌套一层 +1）
    pub depth: u32,
    /// 父线程 ID，根线程为 `None`
    pub parent_id: Option<u64>,
    /// 是否为根线程
    pub is_root: bool,
    /// 创建时间戳（毫秒）
    pub created_at_ms: u64,
    /// 最近任务消息
    pub last_task_message: Option<String>,
    /// 最近状态快照
    pub status_snapshot: Option<String>,
    /// 结束时间戳（毫秒），仅在线程结束时填充
    pub ended_at_ms: Option<u64>,
}

impl ThreadSpawnNode {
    /// 构造新节点。
    #[must_use]
    pub fn new(
        id: u64,
        nickname: String,
        cwd: String,
        depth: u32,
        parent_id: Option<u64>,
        created_at_ms: u64,
    ) -> Self {
        Self {
            id,
            nickname,
            agent_path: None,
            cwd,
            depth,
            parent_id,
            is_root: parent_id.is_none(),
            created_at_ms,
            last_task_message: None,
            status_snapshot: None,
            ended_at_ms: None,
        }
    }

    /// 设置规范代理路径。
    #[must_use]
    pub fn with_agent_path(mut self, agent_path: impl Into<String>) -> Self {
        self.agent_path = Some(agent_path.into());
        self
    }

    /// 设置最近任务消息。
    #[must_use]
    pub fn with_last_task_message(mut self, last_task_message: Option<String>) -> Self {
        self.last_task_message = last_task_message;
        self
    }

    /// 设置最近状态快照。
    #[must_use]
    pub fn with_status_snapshot(mut self, status_snapshot: Option<String>) -> Self {
        self.status_snapshot = status_snapshot;
        self
    }

    /// 标记线程结束。
    pub fn end(&mut self, ended_at_ms: u64) {
        self.ended_at_ms = Some(ended_at_ms);
    }
}
