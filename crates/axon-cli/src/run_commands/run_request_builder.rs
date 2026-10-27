//! 普通 coding `RunRequest` 的唯一最终装配入口。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use axon_agent_runtime::{
    AgentControlRuntime, AgentGraphStore, AgentPath, ProviderStructuredOutputContract,
    SubagentSessionRuntime, ToolRegistry, WorkspaceFileSession,
};
use axon_client_identity::ClientChannel;
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{ModelProfileId, PermissionSnapshotId};
use axon_session_runtime::TurnPerformanceRecorder;
use uuid::Uuid;

use super::{
    RunCancelToken, RunRequest, RunStartupSnapshot,
    run_request_payload::BuildRunModelPayloadOutput, run_steer::RunSteerInbox,
    run_subagent_executor::RunSubagentExecutor,
};

pub(super) struct RunRequestAssemblyInput {
    pub(super) client_channel: ClientChannel,
    pub(super) config_path: PathBuf,
    pub(super) session_id: String,
    pub(super) resumed_from: Option<String>,
    pub(super) parent_session_id: Option<String>,
    pub(super) cwd: PathBuf,
    pub(super) prompt: String,
    pub(super) structured_output: Option<ProviderStructuredOutputContract>,
    pub(super) model_input: BuildRunModelPayloadOutput,
    pub(super) startup: Arc<RunStartupSnapshot>,
    pub(super) execution_tools: Arc<RwLock<ToolRegistry>>,
    pub(super) workspace_files: Arc<WorkspaceFileSession>,
    pub(super) provider: ProviderConfig,
    pub(super) model: ModelConfig,
    pub(super) reasoning_effort: Option<String>,
    pub(super) channel_chain: Vec<(ModelConfig, ProviderConfig)>,
    pub(super) routing_key: String,
    pub(super) balancer: ChannelBalancerConfig,
    pub(super) timeout: Duration,
    pub(super) cancel_token: Option<RunCancelToken>,
    pub(super) model_profile_id: ModelProfileId,
    pub(super) permission_snapshot_id: PermissionSnapshotId,
    pub(super) current_agent_path: AgentPath,
    pub(super) agent_control: Arc<Mutex<AgentControlRuntime>>,
    pub(super) agent_graph_store: Arc<dyn AgentGraphStore>,
    pub(super) subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
    pub(super) subagent_executor: Arc<dyn RunSubagentExecutor>,
    pub(super) subagent_cancel_tokens: Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
    pub(super) steer_inbox: Option<RunSteerInbox>,
    pub(super) auto_compaction_threshold: f64,
    pub(super) performance: Option<Arc<dyn TurnPerformanceRecorder>>,
}

pub(super) fn assemble_run_request(input: RunRequestAssemblyInput) -> RunRequest {
    RunRequest {
        client_channel: input.client_channel,
        config_path: input.config_path,
        session_id: input.session_id,
        invocation_id: Uuid::new_v4().to_string(),
        resumed_from: input.resumed_from,
        parent_session_id: input.parent_session_id,
        cwd: input.cwd,
        prompt: input.prompt,
        quoted_message: None,
        structured_output: input.structured_output,
        selected_skill_name: input.model_input.skill_selection.name,
        selected_skill_source_ref: input.model_input.skill_selection.source_ref,
        model_payload: input.model_input.model_payload,
        persona_warnings: input.model_input.persona_warnings,
        startup: input.startup,
        execution_tools: input.execution_tools,
        workspace_files: input.workspace_files,
        provider: input.provider,
        model: input.model,
        reasoning_effort: input.reasoning_effort,
        channel_chain: input.channel_chain,
        routing_key: input.routing_key,
        balancer: input.balancer,
        timeout: input.timeout,
        memory_recall: input.model_input.memory_recall,
        goal_disclosure_policy: input.model_input.memory_disclosure_policy,
        goal_transport_context: None,
        cancel_token: input.cancel_token,
        model_profile_id: input.model_profile_id,
        permission_snapshot_id: input.permission_snapshot_id,
        current_agent_path: input.current_agent_path,
        agent_control: input.agent_control,
        agent_graph_store: input.agent_graph_store,
        subagent_sessions: input.subagent_sessions,
        subagent_executor: input.subagent_executor,
        subagent_cancel_tokens: input.subagent_cancel_tokens,
        steer_inbox: input.steer_inbox,
        user_question_port: None,
        auto_compaction_threshold: input.auto_compaction_threshold,
        performance: input.performance,
    }
}
