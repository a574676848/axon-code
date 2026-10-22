use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use axon_agent_runtime::{
    AgentControlRuntime, AgentGraphStore, AgentModelPayload, AgentPath,
    ProviderStructuredOutputContract, SubagentSessionRuntime, ToolRegistry, WorkspaceFileSession,
    validate_agent_run_provider_protocol,
};
use axon_client_identity::ClientChannel;
use axon_config::{AxonConfig, ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{DisclosurePolicy, ModelProfileId, PermissionSnapshotId, QuotedMessageContext};
use axon_session_runtime::TurnPerformanceRecorder;

use super::RunCancelToken;
use super::run_model_routing::{ChannelBalancedModel, resolve_model_profile_id};
use super::run_request_builder::{RunRequestAssemblyInput, assemble_run_request};
use super::run_request_payload::{
    BuildRunModelPayloadInput, BuildRunModelPayloadOutput, build_run_model_payload,
};
use super::run_request_session::RunRequestSession;
use super::run_request_subagents::{
    RunSubagentExecutionWiringInput, run_subagent_execution_wiring,
};
use super::run_steer::RunSteerInbox;
use super::run_subagent_executor::RunSubagentExecutor;
use super::*;

#[path = "run_request_build.rs"]
mod build;
pub(super) use build::resolved_request_channel;

/// 从 session_id 生成确定性 selector，用于渠道加权选择。
pub(super) fn hash_session_selector(session_id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    session_id.hash(&mut hasher);
    hasher.finish()
}

pub(super) const CLI_RUN_DEFAULT_MODEL_PROFILE_ID: ModelProfileId = ModelProfileId(0);
pub(super) const CLI_RUN_DEFAULT_PERMISSION_SNAPSHOT_ID: PermissionSnapshotId =
    PermissionSnapshotId(0);

#[derive(Clone)]
pub(super) struct RunRequest {
    pub(super) client_channel: ClientChannel,
    pub(super) config_path: PathBuf,
    pub(super) session_id: String,
    pub(super) invocation_id: String,
    pub(super) resumed_from: Option<String>,
    pub(super) parent_session_id: Option<String>,
    pub(super) cwd: PathBuf,
    pub(super) prompt: String,
    /// 本轮被回复消息的可见文本上下文。
    pub(super) quoted_message: Option<QuotedMessageContext>,
    /// 本轮普通 Coding turn 的可选结构化输出契约。
    pub(super) structured_output: Option<ProviderStructuredOutputContract>,
    pub(super) selected_skill_name: Option<String>,
    pub(super) selected_skill_source_ref: Option<String>,
    pub(super) model_payload: AgentModelPayload,
    pub(super) startup: Arc<RunStartupSnapshot>,
    pub(super) execution_tools: Arc<RwLock<ToolRegistry>>,
    pub(super) workspace_files: Arc<WorkspaceFileSession>,
    pub(super) provider: ProviderConfig,
    pub(super) model: ModelConfig,
    /// 本次请求发送给 provider 的 reasoning_effort。
    pub(super) reasoning_effort: Option<String>,
    /// 渠道失败重试链；第 0 项 == (model, provider)，后续为回退渠道。
    pub(super) channel_chain: Vec<(ModelConfig, ProviderConfig)>,
    /// 渠道负载均衡路由键（model.name 或 fallback 到 model.id）。
    /// 用于健康注册表与亲和缓存的键，保证 selection / failover 一致。
    pub(super) routing_key: String,
    /// 渠道负载均衡配置（断路器策略、亲和 TTL、故障转移开关）。
    pub(super) balancer: ChannelBalancerConfig,
    pub(super) timeout: Duration,
    pub(super) memory_recall: Option<RunMemoryRecallEvent>,
    /// 工具执行必须继承的受众边界；群聊等受限主 turn 不得访问 Global Work Ledger。
    pub(super) goal_disclosure_policy: DisclosurePolicy,
    /// Goal 创建时继承的原 Interaction 路由与受众。
    pub(super) goal_transport_context: Option<serde_json::Value>,
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
    /// 主 turn 专属的用户提问端口。
    ///
    /// 只有用户可见的主 turn 才装配它：子代理与后台执行不得向用户提问，
    /// 因此它们的 RunRequest 一律保持 `None`。
    pub(super) user_question_port: Option<Arc<dyn axon_agent_runtime::UserQuestionPort>>,
    /// 循环内自动压缩阈值（占 context_window 的比例）。
    pub(super) auto_compaction_threshold: f64,
    pub(super) performance: Option<Arc<dyn TurnPerformanceRecorder>>,
}

pub(super) struct PreparedRunInput {
    command: RunCommand,
    cwd: PathBuf,
    prompt: String,
    attached_files: Vec<PathBuf>,
    session_id: String,
    parent_session_id: Option<String>,
    model_history_json: Option<String>,
}

/// 本次 Provider turn 的执行身份。
///
/// 执行身份和受众信任域都由运行时权威写入 transportContext；Provider 层只消费
/// 该合同，不再自行推断。主代理承担用户可见的人格与决策，后台 Goal Execution
/// 是隔离的快速执行子代理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunAgentIdentity {
    /// 私聊或本地主代理 turn：可见 Personal Memory。
    PrimaryPrivate,
    /// 群聊或受限主代理 turn：只可见 Workspace Memory。
    PrimaryWorkspace,
    /// 后台 Goal Execution 的快速执行子代理：不装配 Soul、不继承主 turn 思考强度。
    GoalExecutionSubagent,
}

impl RunAgentIdentity {
    /// 是否装配完整用户 Soul；主代理即使进入群聊也仍是同一个主智能体。
    pub(crate) const fn includes_user_soul(self) -> bool {
        !matches!(self, Self::GoalExecutionSubagent)
    }

    /// 是否装配 owner 私有的 Personal Memory。
    pub(crate) const fn includes_personal_memory(self) -> bool {
        matches!(self, Self::PrimaryPrivate)
    }

    /// 是否保持主 turn 的模型槽位、推理强度、工具策略与稳定前缀。
    pub(crate) const fn is_primary(self) -> bool {
        !matches!(self, Self::GoalExecutionSubagent)
    }

    /// 本次请求的 Memory 披露边界。
    ///
    /// 群聊与子代理只可读取当前 Workspace Memory，避免跨边界泄露 Global Memory。
    pub(crate) const fn memory_disclosure_policy(self) -> axon_domain::DisclosurePolicy {
        match self {
            Self::PrimaryPrivate => axon_domain::DisclosurePolicy::PrivateOnly,
            Self::PrimaryWorkspace | Self::GoalExecutionSubagent => {
                axon_domain::DisclosurePolicy::Workspace
            }
        }
    }
}

fn resumed_parent_session_id(session_id: &str, parent_session_id: Option<&str>) -> Option<String> {
    parent_session_id
        .filter(|parent| *parent != session_id)
        .map(ToOwned::to_owned)
}

pub(super) fn prepare_run_input(
    command: &RunCommand,
    previous_session: Option<&run_thread_store::PreparedRunSession>,
    session_id: String,
    config: &AxonConfig,
    cwd: Option<&Path>,
) -> Result<PreparedRunInput, String> {
    let command = resumed_command(command, previous_session, config)?;
    let resumed_cwd = previous_session
        .and_then(|session| session.record.cwd.as_deref())
        .map(Path::new);
    let cwd = resolve_effective_run_cwd(cwd, resumed_cwd)?;
    let (prompt, attached_files) = compose_run_prompt(&command)?;
    let model_history_json = previous_session.map(|session| session.model_history_json.clone());
    let parent_session_id = previous_session.and_then(|session| {
        resumed_parent_session_id(
            &session.record.session_id,
            session.record.parent_session_id.as_deref(),
        )
    });
    Ok(PreparedRunInput {
        command,
        cwd,
        prompt,
        attached_files,
        session_id,
        parent_session_id,
        model_history_json,
    })
}

pub(super) fn prepare_model_input_and_tools(
    input: &BuildRunModelPayloadInput<'_>,
) -> Result<(BuildRunModelPayloadOutput, Arc<RwLock<ToolRegistry>>), String> {
    let tool_environment = input
        .startup
        .tool_environment_for_session(input.config_path, input.session_id);
    let execution_tools = tool_environment.execution_tools();
    let execution_tools = restrict_execution_tools(execution_tools, &input.command.tool)?;
    let registry = execution_tools
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let model_input = build_run_model_payload(input, &registry)?;
    Ok((model_input, execution_tools))
}

fn restrict_execution_tools(
    registry: Arc<RwLock<ToolRegistry>>,
    tool_names: &[String],
) -> Result<Arc<RwLock<ToolRegistry>>, String> {
    if tool_names.is_empty() {
        return Ok(registry);
    }
    let restricted = registry
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .restricted_to_qualified_names(tool_names)
        .map_err(|error| format!("--tool 限制无效：{error:?}"))?;
    Ok(Arc::new(RwLock::new(restricted)))
}

fn resolve_run_reasoning_effort(command: &RunCommand, config: &AxonConfig) -> String {
    command
        .reasoning_effort
        .clone()
        .unwrap_or_else(|| config.agent.reasoning_effort.clone())
}

fn validate_run_provider(provider: &ProviderConfig) -> Result<(), String> {
    validate_agent_run_provider_protocol(provider)
        .map_err(|_| "axon run 当前仅支持 chat-capable provider 协议".to_owned())
}

pub(super) fn resolve_run_cwd(cwd: Option<&Path>) -> Result<PathBuf, String> {
    let cwd = match cwd {
        Some(path) => path.to_path_buf(),
        None => {
            std::env::current_dir().map_err(|error| format!("无法读取当前工作目录：{error}"))?
        }
    };
    let canonical =
        fs::canonicalize(&cwd).map_err(|_| format!("工作目录不可访问：{}", cwd.display()))?;
    if !canonical.is_dir() {
        return Err(format!("工作目录不是目录：{}", canonical.display()));
    }
    Ok(canonical)
}

/// 解析本轮实际 Workspace：显式请求目录优先，否则沿用已持久化的上一轮目录。
pub(super) fn resolve_effective_run_cwd(
    requested_cwd: Option<&Path>,
    recorded_cwd: Option<&Path>,
) -> Result<PathBuf, String> {
    resolve_run_cwd(requested_cwd.or(recorded_cwd))
}

fn compose_run_prompt(command: &RunCommand) -> Result<(String, Vec<PathBuf>), String> {
    let prompt = command.prompt.trim().to_owned();
    let mut attached_files = Vec::with_capacity(command.file.len());
    for file in &command.file {
        let canonical =
            fs::canonicalize(file).map_err(|_| format!("附加文件不可访问：{}", file.display()))?;
        if !canonical.is_file() {
            return Err(format!("附加路径不是文件：{}", canonical.display()));
        }
        attached_files.push(canonical);
    }
    if prompt.is_empty() && attached_files.is_empty() {
        return Err("axon run prompt 和附件不能同时为空".to_owned());
    }
    Ok((prompt, attached_files))
}

fn resumed_command(
    command: &RunCommand,
    previous_session: Option<&run_thread_store::PreparedRunSession>,
    config: &AxonConfig,
) -> Result<RunCommand, String> {
    let Some(previous) = previous_session else {
        return Ok(command.clone());
    };
    let mut effective = command.clone();
    if effective.model.is_none() {
        let current_default_is_available = config
            .models
            .iter()
            .filter(|model| config.model_is_enabled(model))
            .any(|model| model.name == config.agent.default_model)
            || config
                .models
                .iter()
                .filter(|model| config.model_is_enabled(model))
                .any(|model| model.id == config.agent.default_model);
        if !current_default_is_available {
            let record = &previous.record;
            if let Some(model) = config
                .models
                .iter()
                .filter(|model| config.model_is_enabled(model))
                .find(|model| model.provider == record.provider && model.id == record.model)
            {
                effective.model = Some(model.name.clone());
            }
        }
    }
    Ok(effective)
}

#[cfg(test)]
#[path = "run_request_resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
mod tests {
    use super::super::run_request_subagents::subagent_graph_store;
    use super::*;
    use crate::stable_ids::stable_numeric_id;

    #[test]
    fn run_request_uses_configured_control_limits() {
        let workspace = axon_test_support::test_temp_root().join(format!(
            "axon-run-files-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("系统时间应晚于 UNIX_EPOCH")
                .as_nanos()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let config_path = workspace.join("config.toml");
        let mut config = axon_config::AxonConfig::default();
        config.providers.push(axon_config::ProviderConfig::new(
            "fixture",
            "openai-chatcompletions",
            "https://provider.invalid/v1",
        ));
        config.models.push(axon_config::ModelConfig {
            name: "fixture-chat".to_owned(),
            id: "fixture-chat".to_owned(),
            provider: "fixture".to_owned(),
            context_window: 32_000,
            max_output_tokens: None,
            supports_vision: false,
            supports_embedding: false,
            embedding_dimension: 0,
        });
        config.agent.default_model = "fixture-chat".to_owned();
        config.agent.fast_model = "fixture-chat".to_owned();
        axon_config::ConfigLoader::save_to_path(&config_path, &config).unwrap();
        let config = axon_config::ConfigLoader::load_from_path(&config_path)
            .expect("配置应可读取")
            .config;

        let command = RunCommand {
            prompt: "解释错误".to_owned(),
            model: None,
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let startup = Arc::new(RunStartupSnapshot::build(&config_path, &workspace).unwrap());

        let request = RunRequest::from_command(
            &command,
            &config,
            &config_path,
            RunRequestSession::with_id(None, super::new_run_session_id()),
            Some(&workspace),
            RunRequestBuildOptions {
                client_channel: ClientChannel::Cli,
                startup,
                cancel_token: None,
                subagent_event_bus: None,
                subagent_session_sinks: None,
                agent_identity: RunAgentIdentity::PrimaryPrivate,
                performance: None,
                workspace_snapshot_prewarm: None,
            },
        )
        .expect("RunRequest 应可构造");

        let control_config = request.agent_control.lock().unwrap().config();
        assert_eq!(control_config.max_agents, config.agent.max_subagent_threads);
        assert_eq!(
            control_config.max_active_turns,
            config.agent.max_subagent_active_turns
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn goal_execution_subagent_runs_fast_model_without_main_turn_reasoning() {
        let workspace = axon_test_support::test_temp_root().join(format!(
            "axon-run-task-execution-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("系统时间应晚于 UNIX_EPOCH")
                .as_nanos()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let config_path = workspace.join("config.toml");
        let mut config = axon_config::AxonConfig::default();
        config.providers.push(axon_config::ProviderConfig::new(
            "fixture",
            "openai-chatcompletions",
            "https://provider.invalid/v1",
        ));
        for model_id in ["main-model", "fast-model"] {
            config.models.push(axon_config::ModelConfig {
                name: model_id.to_owned(),
                id: model_id.to_owned(),
                provider: "fixture".to_owned(),
                context_window: 32_000,
                max_output_tokens: None,
                supports_vision: false,
                supports_embedding: false,
                embedding_dimension: 0,
            });
        }
        config.agent.default_model = "main-model".to_owned();
        config.agent.fast_model = "fast-model".to_owned();
        config.agent.reasoning_effort = "high".to_owned();
        axon_config::ConfigLoader::save_to_path(&config_path, &config).unwrap();
        let config = axon_config::ConfigLoader::load_from_path(&config_path)
            .expect("配置应可读取")
            .config;

        let command = RunCommand {
            prompt: "执行已承诺的 Task".to_owned(),
            model: None,
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let startup = Arc::new(RunStartupSnapshot::build(&config_path, &workspace).unwrap());
        let build = |agent_identity: RunAgentIdentity| {
            RunRequest::from_command(
                &command,
                &config,
                &config_path,
                RunRequestSession::with_id(None, super::new_run_session_id()),
                Some(&workspace),
                RunRequestBuildOptions {
                    client_channel: ClientChannel::Cli,
                    startup: Arc::clone(&startup),
                    cancel_token: None,
                    subagent_event_bus: None,
                    subagent_session_sinks: None,
                    agent_identity,
                    performance: None,
                    workspace_snapshot_prewarm: None,
                },
            )
            .expect("RunRequest 应可构造")
        };

        let primary = build(RunAgentIdentity::PrimaryPrivate);
        let execution = build(RunAgentIdentity::GoalExecutionSubagent);

        assert_eq!(primary.model.id, "main-model");
        assert_eq!(primary.reasoning_effort.as_deref(), Some("high"));
        assert!(RunAgentIdentity::PrimaryPrivate.includes_personal_memory());
        assert!(RunAgentIdentity::PrimaryWorkspace.includes_user_soul());
        assert!(!RunAgentIdentity::PrimaryWorkspace.includes_personal_memory());
        assert_eq!(
            RunAgentIdentity::PrimaryWorkspace.memory_disclosure_policy(),
            axon_domain::DisclosurePolicy::Workspace
        );
        // 后台 Goal Execution 与 fast 子代理一致：fast 模型、不继承主 turn 思考强度、不加载 Soul。
        assert_eq!(execution.model.id, "fast-model");
        assert_eq!(execution.reasoning_effort, None);
        assert!(!RunAgentIdentity::GoalExecutionSubagent.includes_user_soul());
        assert_eq!(
            RunAgentIdentity::GoalExecutionSubagent.memory_disclosure_policy(),
            axon_domain::DisclosurePolicy::Workspace
        );
        let context_text = |request: &RunRequest| {
            request
                .model_payload
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(context_text(&primary).contains("<tool_orchestration_protocol>"));
        assert!(!context_text(&execution).contains("<tool_orchestration_protocol>"));

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn image_attachment_is_not_inlined_into_prompt() {
        let workspace = axon_test_support::test_temp_root().join(format!(
            "axon-run-image-prompt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("系统时间应晚于 UNIX_EPOCH")
                .as_nanos()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let image_path = workspace.join("screen.png");
        std::fs::write(
            &image_path,
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .unwrap();
        let command = RunCommand {
            prompt: "分析图片".to_owned(),
            model: None,
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: vec![image_path.clone()],
            timeout: None,
            tool: Vec::new(),
        };

        let (prompt, attached_files) = compose_run_prompt(&command).expect("图片附件应可投影");

        assert_eq!(attached_files, vec![image_path.canonicalize().unwrap()]);
        assert_eq!(prompt, "分析图片");
        assert!(!prompt.contains(image_path.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn text_attachment_is_not_inlined_and_can_be_the_only_user_input() {
        let workspace = axon_test_support::test_temp_root()
            .join(format!("axon-run-file-prompt-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let file_path = workspace.join("report.txt");
        std::fs::write(&file_path, "不应进入 prompt 的附件正文").unwrap();
        let command = RunCommand {
            prompt: String::new(),
            model: None,
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: vec![file_path.clone()],
            timeout: None,
            tool: Vec::new(),
        };

        let (prompt, attached_files) = compose_run_prompt(&command).expect("纯附件请求应有效");

        assert!(prompt.is_empty());
        assert_eq!(attached_files, vec![file_path.canonicalize().unwrap()]);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn subagent_graph_store_uses_session_root_thread_id() {
        let workspace = axon_test_support::test_temp_root().join(format!(
            "axon-run-subagent-graph-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("系统时间应晚于 UNIX_EPOCH")
                .as_nanos()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let config_path = workspace.join("isolated-config.toml");
        std::fs::write(&config_path, "").unwrap();
        let mut config = axon_config::AxonConfig::default();
        config.providers.push(axon_config::ProviderConfig::new(
            "fixture",
            "openai-chatcompletions",
            "https://provider.invalid/v1",
        ));
        config.models.push(axon_config::ModelConfig {
            name: "fixture-chat".to_owned(),
            id: "fixture-chat".to_owned(),
            provider: "fixture".to_owned(),
            context_window: 32_000,
            max_output_tokens: None,
            supports_vision: false,
            supports_embedding: false,
            embedding_dimension: 0,
        });
        config.agent.default_model = "fixture-chat".to_owned();
        config.agent.fast_model = "fixture-chat".to_owned();
        let command = RunCommand {
            prompt: "审查当前改动".to_owned(),
            model: None,
            reasoning_effort: None,
            format: Some(OutputFormat::Json),
            continue_last: false,
            session: None,
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        };
        let startup = Arc::new(RunStartupSnapshot::build(&config_path, &workspace).unwrap());
        let request = RunRequest::from_command(
            &command,
            &config,
            &config_path,
            RunRequestSession::with_id(None, super::new_run_session_id()),
            Some(&workspace),
            RunRequestBuildOptions {
                client_channel: ClientChannel::Cli,
                startup,
                cancel_token: None,
                subagent_event_bus: None,
                subagent_session_sinks: None,
                agent_identity: RunAgentIdentity::PrimaryPrivate,
                performance: None,
                workspace_snapshot_prewarm: None,
            },
        )
        .expect("RunRequest 应可构造");
        let root_thread_id = request
            .agent_control
            .lock()
            .unwrap()
            .root_thread_id()
            .unwrap();
        assert_eq!(root_thread_id, stable_numeric_id(&request.session_id));

        let store = request.agent_graph_store.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            store
                .create_thread_node(
                    axon_domain::ThreadSpawnNode::new(
                        root_thread_id,
                        "root".to_owned(),
                        workspace.to_string_lossy().to_string(),
                        0,
                        None,
                        1,
                    )
                    .with_agent_path("/root"),
                )
                .await
                .unwrap();
            store
                .create_thread_node(
                    axon_domain::ThreadSpawnNode::new(
                        root_thread_id + 1,
                        "review".to_owned(),
                        workspace.to_string_lossy().to_string(),
                        1,
                        Some(root_thread_id),
                        1,
                    )
                    .with_agent_path("/root/review"),
                )
                .await
                .unwrap();
            store
                .upsert_thread_spawn_edge(root_thread_id, root_thread_id + 1, 1)
                .await
                .unwrap();
        });
        let graph_file = workspace.join("data").join("subagent-graph.json");
        assert!(graph_file.exists());

        let reopened = subagent_graph_store(&config_path);
        let child = rt
            .block_on(reopened.find_thread_spawn_child_by_path(root_thread_id, "/root/review"))
            .unwrap();
        assert_eq!(child.map(|node| node.id), Some(root_thread_id + 1));

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn resumed_parent_session_rejects_self_reference() {
        assert_eq!(
            resumed_parent_session_id("session-1", Some("session-1")),
            None
        );
        assert_eq!(
            resumed_parent_session_id("session-1", Some("parent-1")).as_deref(),
            Some("parent-1")
        );
    }
}
