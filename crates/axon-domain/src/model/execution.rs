//! Work Ledger Execution 领域模型。

use super::{DaemonSessionId, ExecutionId, GoalId};
use serde::{Deserialize, Serialize};

/// Execution 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// 等待 Worker 领取。
    Queued,
    /// 正在执行。
    Running,
    /// 等待外部条件。
    Waiting,
    /// 外部副作用结果未知，禁止盲目重试。
    Uncertain,
    /// 已成功完成。
    Completed,
    /// 已失败。
    Failed,
    /// 已取消。
    Cancelled,
}

impl ExecutionStatus {
    /// 判断是否为终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// 判断是否允许 Worker claim。
    #[must_use]
    pub const fn is_claimable(self) -> bool {
        matches!(self, Self::Queued)
    }

    /// 判断是否已形成最终结论：终态或外部结果未知的 `uncertain`。
    ///
    /// 形成结论的 Execution 必须已释放 lease、已提交 Result，且绝不允许自动重试。
    #[must_use]
    pub const fn is_conclusive(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Uncertain
        )
    }
}

/// Execution checkpoint。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionCheckpoint {
    /// 已提交的事件序号。
    pub event_sequence: u64,
    /// 可恢复的上下文引用。
    pub context_ref: String,
    /// 保存时间。
    pub saved_at_ms: u64,
}

/// 首次 Execution 尚未绑定业务 payload 时的空 JSON 对象。
pub const EMPTY_EXECUTION_PAYLOAD: &str = "{}";

/// 一次 Goal 执行尝试。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Execution 标识。
    pub execution_id: ExecutionId,
    /// 所属 Goal。
    pub goal_id: GoalId,
    /// 内部绑定的运行时会话标识。
    pub session_id: Option<DaemonSessionId>,
    /// Execution 启动所需的 Provider turn payload。
    pub payload_json: String,
    /// 业务幂等键；同一 Goal 的非终态 Execution 不得重复。
    pub dedupe_key: Option<String>,
    /// 调度优先级，数值越大越优先。
    pub priority: i32,
    /// 尝试次数，从 1 开始。
    pub attempt: u32,
    /// 当前状态。
    pub status: ExecutionStatus,
    /// 当前租约。
    pub lease: Option<ExecutionLease>,
    /// 下一次允许领取的时间；为空表示立即可领取。
    pub retry_at_ms: Option<u64>,
    /// 最近 checkpoint。
    pub checkpoint: Option<ExecutionCheckpoint>,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
}

/// Execution Worker 租约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionLease {
    /// owner 标识。
    pub owner: String,
    /// fencing token。
    pub token: String,
    /// 到期时间。
    pub expires_at_ms: u64,
}

impl ExecutionRecord {
    /// 创建排队中的首次 Execution。
    pub fn new(execution_id: ExecutionId, goal_id: GoalId, now_ms: u64) -> Self {
        Self {
            execution_id,
            goal_id,
            session_id: None,
            payload_json: EMPTY_EXECUTION_PAYLOAD.to_owned(),
            dedupe_key: None,
            priority: 0,
            attempt: 1,
            status: ExecutionStatus::Queued,
            lease: None,
            retry_at_ms: None,
            checkpoint: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    /// 绑定内部运行时会话。
    #[must_use]
    pub fn with_session_id(mut self, session_id: DaemonSessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// 绑定执行 Provider turn 所需的 payload。
    #[must_use]
    pub fn with_payload_json(mut self, payload_json: impl Into<String>) -> Self {
        self.payload_json = payload_json.into();
        self
    }

    /// 判断当前记录是否可以被 Worker 领取。
    #[must_use]
    pub fn can_claim(&self) -> bool {
        self.status.is_claimable() && self.lease.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_execution_is_not_claimable() {
        let mut execution = ExecutionRecord::new(
            ExecutionId::new("execution-1").expect("ID"),
            GoalId::new("task-1").expect("ID"),
            1,
        );
        assert!(execution.can_claim());
        execution.status = ExecutionStatus::Completed;
        assert!(execution.status.is_terminal());
        assert!(!execution.can_claim());
    }
}
