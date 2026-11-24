use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use axon_agent_runtime::{
    AgentControlRuntime, AgentGraphStore, AgentPath, FileAgentGraphStore, LocalAgentGraphStore,
    SubagentSessionRuntime, WorkspaceFileSession,
};
use axon_config::{AxonConfig, ModelConfig, ProviderConfig};
use axon_session_runtime::SubagentSessionSinkFactory;

use crate::stable_ids::stable_numeric_id;

use super::run_session_store::resolved_axon_root;
use super::{
    RunCancelToken, RunCommand, RunStartupSnapshot, provider_stream_idle_timeout,
    run_progress::RunProgressEventBus,
    run_request::CLI_RUN_DEFAULT_PERMISSION_SNAPSHOT_ID,
    run_subagent_executor::{
        ProviderRunSubagentEnvironment, ProviderRunSubagentExecutor, RunSubagentExecutor,
    },
};

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
    /// 父会话身份（`sessions.session_id`）。
    pub(super) session_id: &'a str,
    /// 父运行窗口（`sessions.current_run_id`）。
    pub(super) run_session_id: &'a str,
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
    // 库路径与主链路同源（`<axon_root>/data/control.sqlite3`），因此本机 run
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
                // 子代理不再持有派发工具，新树恒为一层；这里沿运行时默认值，
                // 只用于恢复旧版本遗留的多层嵌套节点。
                max_depth: axon_agent_runtime::AGENT_CONTROL_DEFAULT_MAX_DEPTH,
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
            parent_run_session_id: input.run_session_id.to_owned(),
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
