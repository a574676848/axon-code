//! `RunRequest` 构建期依赖。

use std::sync::Arc;

use axon_client_identity::ClientChannel;
use axon_session_runtime::{SubagentSessionSinkFactory, TurnPerformanceRecorder};

use super::{
    RunAgentIdentity, RunCancelToken, RunStartupSnapshot, run_progress::RunProgressEventBus,
    run_request_payload::RunWorkspaceSnapshotPrewarm,
};

pub(super) struct RunRequestBuildOptions {
    pub(super) client_channel: ClientChannel,
    pub(super) startup: Arc<RunStartupSnapshot>,
    pub(super) cancel_token: Option<RunCancelToken>,
    pub(super) subagent_event_bus: Option<RunProgressEventBus>,
    /// 宿主提供的子代理会话 sink 工厂；缺省表示本轮不持久化子代理逐字稿。
    pub(super) subagent_session_sinks: Option<Arc<dyn SubagentSessionSinkFactory>>,
    /// 本次请求的执行身份；决定稳定 System 前缀、模型槽位与思考强度。
    pub(super) agent_identity: RunAgentIdentity,
    pub(super) performance: Option<Arc<dyn TurnPerformanceRecorder>>,
    /// 在 history 准备前启动、由 payload 构建阶段一次消费的 Workspace 快照。
    pub(super) workspace_snapshot_prewarm: Option<RunWorkspaceSnapshotPrewarm>,
    /// 本轮 Action 的运行时上下文；用于把会话车道标识并入动态环境上下文。
    pub(super) transport_context: Option<serde_json::Value>,
}
