//! Session Runtime 的应用模型。

pub mod action_dispatch;
pub mod execution_failure;
pub mod execution_worker_state;
pub mod recovery_decision;

pub use execution_failure::{TurnExecutionError, TurnFailureKind};

/// Provider turn 完成事件类型：payload 携带执行者最终回复文本。
pub const TURN_COMPLETED_EVENT: &str = "turn_completed";

use axon_session::{CheckpointTelemetry, SessionActionRecord};

use crate::application::JarvisRuntimeProfile;

/// Session Runtime 在执行边界提供的权威上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionExecutionContext {
    /// 当前 rollout ID。
    pub run_session_id: String,
    /// 当前工作目录。
    pub cwd: String,
    /// 当前 Context 版本。
    pub context_version: u64,
}

/// 交给 Agent Turn Executor 的请求。
#[derive(Debug, Clone)]
pub struct ActionExecutionRequest {
    /// 已持久 claim 的 Action。
    pub action: SessionActionRecord,
    /// 从 Session Runtime 读取的权威执行上下文。
    pub context: SessionExecutionContext,
    /// 应用层为本次 turn 选择的统一人格 profile。
    pub runtime_profile: JarvisRuntimeProfile,
}

/// completion/compaction 后需要显式创建的下一次 Execution。
#[derive(Debug, Clone)]
pub struct NextExecutionRequest {
    /// 关联的 Work Ledger Goal。
    pub goal_id: Option<axon_domain::GoalId>,
    /// 父 Execution。
    pub parent_execution_id: Option<axon_domain::ExecutionId>,
    /// 当前阶段标识。
    pub stage: Option<String>,
    /// 原始 transport context JSON。
    pub transport_context_json: Option<String>,
    /// Action 类型。
    pub kind: String,
    /// 优先级。
    pub priority: i32,
    /// 幂等键。
    pub dedupe_key: String,
    /// JSON payload。
    pub payload_json: String,
    /// Goal revision。
    pub goal_revision: Option<u64>,
    /// Context 版本。
    pub context_version: Option<u64>,
}

/// 一次 Agent 执行的持久化结果。
#[derive(Debug, Clone)]
pub struct TurnExecutionOutcome {
    /// 要追加到 Session journal 的事件。
    pub events: Vec<RuntimeEvent>,
    /// 成功后要创建的下一次 Execution。
    pub next_execution: Option<NextExecutionRequest>,
    /// Provider turn checkpoint 遥测。
    pub checkpoint: CheckpointTelemetry,
}

/// 运行时事件。
#[derive(Debug, Clone)]
pub struct RuntimeEvent {
    /// 稳定事件类型。
    pub event_type: String,
    /// JSON payload。
    pub payload_json: String,
}
