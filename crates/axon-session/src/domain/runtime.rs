//! 持久 Session Runtime 的存储模型。

use axon_domain::{
    ConsumerCursor, DaemonSessionId, GoalId, RuntimeHostId, SessionActionId, SessionActionStatus,
    SessionRuntimeStatus,
};
use serde::{Deserialize, Serialize};

/// Session Runtime 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRuntimeRecord {
    /// 稳定 Session ID。
    pub session_id: DaemonSessionId,
    /// 当前 rollout ID。
    pub run_session_id: String,
    /// 根 Session ID。
    pub root_session_id: DaemonSessionId,
    /// 工作目录。
    pub cwd: String,
    /// 当前状态。
    pub status: SessionRuntimeStatus,
    /// 当前 owner Host。
    pub owner_host_id: Option<RuntimeHostId>,
    /// 当前 owner generation。
    pub owner_generation: Option<String>,
    /// Worker 恢复尝试次数。
    pub worker_attempt: u32,
    /// Context 版本。
    pub context_version: u64,
    /// 最后提交的 checkpoint 序号。
    pub last_checkpoint_seq: u64,
    /// 最后事件序号。
    pub last_event_sequence: u64,
    /// 最后活动时间。
    pub last_activity_at_ms: u64,
}

/// 持久 Action 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionRecord {
    /// Action ID。
    pub action_id: SessionActionId,
    /// 关联的 Work Ledger Goal；普通内部 turn 可以为空。
    pub goal_id: Option<GoalId>,
    /// 所属 Session。
    pub session_id: DaemonSessionId,
    /// Action 类型。
    pub kind: String,
    /// 优先级，数值越大越优先。
    pub priority: i32,
    /// 当前状态。
    pub status: SessionActionStatus,
    /// 幂等键。
    pub dedupe_key: Option<String>,
    /// JSON payload。
    pub payload_json: String,
    /// 可选 Goal revision。
    pub goal_revision: Option<u64>,
    /// 生成该 Action 时的 context 版本。
    pub context_version: Option<u64>,
    /// 当前 claim Host。
    pub claimed_by: Option<RuntimeHostId>,
    /// 当前 claim generation。
    pub claimed_generation: Option<String>,
    /// claim lease 过期时间。
    pub lease_expires_at_ms: Option<u64>,
    /// 执行尝试次数。
    pub attempt: u32,
}

/// Session 事件记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEventRecord {
    /// 所属 Session。
    pub session_id: DaemonSessionId,
    /// Session 内单调递增序号。
    pub sequence: u64,
    /// Host generation。
    pub generation: String,
    /// 事件类型。
    pub event_type: String,
    /// JSON payload。
    pub payload_json: String,
    /// 事件时间。
    pub emitted_at_ms: u64,
}

impl SessionEventRecord {
    /// 返回事件 cursor。
    #[must_use]
    pub fn cursor(&self) -> ConsumerCursor {
        ConsumerCursor::new(self.generation.clone(), self.sequence)
    }
}

/// Host 运行记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonHostRecord {
    /// Host ID。
    pub host_id: RuntimeHostId,
    /// 当前 fencing generation。
    pub generation: String,
    /// 进程 ID。
    pub pid: u32,
    /// 进程启动身份。
    pub process_start_id: Option<String>,
    /// Host 状态。
    pub state: String,
    /// Host lease 过期时间戳。
    pub lease_expires_at_ms: u64,
}

/// 持久 schedule 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeScheduleRecord {
    /// schedule ID。
    pub schedule_id: String,
    /// 可选 Session。
    pub session_id: Option<DaemonSessionId>,
    /// schedule 类型。
    pub kind: String,
    /// 原始表达式。
    pub expression: String,
    /// 触发后写入 Session Action 的 JSON payload。
    pub payload_json: String,
    /// 投递模式。
    pub delivery_mode: String,
    /// 生命周期状态。
    pub status: String,
    /// 下次触发时间。
    pub next_run_at_ms: Option<u64>,
    /// claim owner。
    pub lease_owner: Option<RuntimeHostId>,
    /// claim lease 过期时间。
    pub lease_expires_at_ms: Option<u64>,
    /// 连续失败次数。
    pub failure_count: u32,
}
