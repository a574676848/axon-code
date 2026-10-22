use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use axon_agent_runtime::{
    AgentControlRuntime, AgentGraphStore, AgentPath, ChildExecutionInput, FileAgentGraphStore,
    LocalAgentGraphStore, SubagentForkContextReport, SubagentSessionRuntime, WorkspaceFileSession,
};
use axon_config::{AxonConfig, ConfigLoader, ModelConfig, ProviderConfig};
use axon_session_runtime::SubagentSessionSinkFactory;

use crate::stable_ids::stable_numeric_id;

use super::{
    RunCancelToken, RunCommand, RunStartupSnapshot, provider_stream_idle_timeout,
    run_model_routing::select_channel_balanced_model,
    run_progress::RunProgressEventBus,
    run_request::CLI_RUN_DEFAULT_PERMISSION_SNAPSHOT_ID,
    run_request::hash_session_selector,
    run_session_store::resolved_axon_root,
    run_subagent_executor::{
        ProviderRunSubagentEnvironment, ProviderRunSubagentExecutor, RunSubagentExecutor,
    },
};

/// 运行独立子代理；Goal Execution 与 cron 共用完整子代理生命周期。
pub(crate) fn execute_standalone_subagent(
    config_path: &Path,
    cwd: &Path,
    session_id: &str,
    task_path: AgentPath,
    prompt: &str,
    started_at_ms: u64,
) -> Result<super::run_subagent_executor::RunSubagentExecutionOutput, String> {
    let loaded = ConfigLoader::load_from_path(config_path).map_err(|error| format!("{error:?}"))?;
    let command = RunCommand {
        prompt: prompt.to_owned(),
        model: None,
        reasoning_effort: None,
        format: None,
        continue_last: false,
        session: Some(session_id.to_owned()),
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    };
    let startup = RunStartupSnapshot::resolve_for_run(config_path, Some(cwd), None)?;
    let selected = select_channel_balanced_model(
        &command,
        &loaded.config,
        session_id,
        hash_session_selector(session_id),
        started_at_ms,
    )?;
    let workspace_files = Arc::new(WorkspaceFileSession::default());
    let wiring = run_subagent_execution_wiring(RunSubagentExecutionWiringInput {
        config: &loaded.config,
        config_path,
        session_id,
        cwd,
        provider: &selected.provider,
        model: &selected.model,
        reasoning_effort: None,
        channel_chain: &selected.chain,
        routing_key: &selected.routing_key,
        command: &command,
        startup: &startup,
        cancel_token: None,
        workspace_files,
        subagent_event_bus: None,
        subagent_session_sinks: None,
    })?;
    let input = ChildExecutionInput {
        task_path,
        message: prompt.to_owned(),
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
        fork_context: SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms,
        max_runtime_ms: None,
        model_intent: axon_agent_runtime::ChildExecutionModelIntent::FastDelegate,
    };
    wiring
        .subagent_executor
        .execute(input)
        .ok_or_else(|| "cron 子代理执行器未接管任务".to_owned())
}

pub(super) struct RunSubagentExecutionWiring {
    pub(super) agent_control: Arc<Mutex<AgentControlRuntime>>,
    pub(super) agent_graph_store: Arc<dyn AgentGraphStore>,
    pub(super) subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
    pub(super) subagent_executor: Arc<dyn RunSubagentExecutor>,
    pub(super) subagent_cancel_tokens: Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
}

pub(super) struct RunSubagentExecutionWiringInput<'a> {
    pub(super) config: &'a AxonConfig,
    pub(super) config_path: &'a Path,
    pub(super) session_id: &'a str,
    pub(super) cwd: &'a Path,
    pub(super) provider: &'a ProviderConfig,
    pub(super) model: &'a ModelConfig,
    pub(super) reasoning_effort: Option<String>,
    pub(super) channel_chain: &'a [(ModelConfig, ProviderConfig)],
    pub(super) routing_key: &'a str,
    pub(super) command: &'a RunCommand,
    pub(super) startup: &'a Arc<RunStartupSnapshot>,
    pub(super) cancel_token: Option<RunCancelToken>,
    pub(super) workspace_files: Arc<WorkspaceFileSession>,
    pub(super) subagent_event_bus: Option<RunProgressEventBus>,
    /// 宿主提供的子代理会话 sink 工厂。
    ///
    /// 子代理是独立会话：它的逐字稿要落进它自己的会话事件流，
    /// 界面才能用与普通会话同一条管线渲染。工厂由持锁宿主实现，运行侧只转交。
    pub(super) subagent_session_sinks: Option<Arc<dyn SubagentSessionSinkFactory>>,
}

pub(super) fn run_subagent_execution_wiring(
    input: RunSubagentExecutionWiringInput<'_>,
) -> Result<RunSubagentExecutionWiring, String> {
    let root_thread_id = stable_numeric_id(input.session_id);
    // mailbox 落盘绑定在装配层：内核不依赖数据库，只持有端口实现。
    // 库路径与主链路同源（`<axon_root>/data/runtime.sqlite3`），因此本机 run
    // 与 daemon 共享同一份 mailbox 事实（合同 §5.3.1）。
    let mailbox_persistence = super::run_mailbox_persistence::runtime_mailbox_persistence(
        input.config_path,
        input.session_id,
    )?;
    let agent_control = Arc::new(Mutex::new(
        AgentControlRuntime::new_with_root_thread_id(
            axon_agent_runtime::AgentControlConfig {
                max_agents: input.config.agent.max_subagent_threads,
                max_active_turns: input.config.agent.max_subagent_active_turns,
                max_depth: input.config.agent.max_subagent_depth,
            },
            root_thread_id,
        )
        .with_mailbox_persistence(input.session_id, mailbox_persistence),
    ));
    let agent_graph_store = subagent_graph_store(input.config_path);
    let subagent_sessions = Arc::new(Mutex::new(SubagentSessionRuntime::default()));
    let subagent_cancel_tokens = Arc::new(Mutex::new(HashMap::new()));
    let subagent_executor = Arc::new(ProviderRunSubagentExecutor::new(
        ProviderRunSubagentEnvironment {
            config: input.config.clone(),
            config_path: input.config_path.to_path_buf(),
            parent_session_id: input.session_id.to_owned(),
            cwd: input.cwd.to_path_buf(),
            provider: input.provider.clone(),
            model: input.model.clone(),
            reasoning_effort: input.reasoning_effort,
            channel_chain: input.channel_chain.to_vec(),
            routing_key: input.routing_key.to_owned(),
            balancer: input.config.channel_balancer,
            timeout: provider_stream_idle_timeout(input.command.timeout),
            permission_snapshot_id: CLI_RUN_DEFAULT_PERMISSION_SNAPSHOT_ID,
            startup: Arc::clone(input.startup),
            cancel_token: input.cancel_token,
            workspace_files: input.workspace_files,
            agent_control: agent_control.clone(),
            agent_graph_store: agent_graph_store.clone(),
            subagent_sessions: subagent_sessions.clone(),
            subagent_event_bus: input.subagent_event_bus.clone(),
            subagent_session_sinks: input.subagent_session_sinks.clone(),
            subagent_cancel_tokens: subagent_cancel_tokens.clone(),
        },
    ));
    Ok(RunSubagentExecutionWiring {
        agent_control,
        agent_graph_store,
        subagent_sessions,
        subagent_executor,
        subagent_cancel_tokens,
    })
}

pub(super) fn subagent_graph_store(config_path: &Path) -> Arc<dyn AgentGraphStore> {
    resolved_axon_root(config_path)
        .ok()
        .map(|root| root.join("data").join("subagent-graph.json"))
        .and_then(|path| {
            FileAgentGraphStore::open(path)
                .ok()
                .map(|store| Arc::new(store) as Arc<dyn AgentGraphStore>)
        })
        .unwrap_or_else(|| Arc::new(LocalAgentGraphStore::default()))
}
