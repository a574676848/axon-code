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
    /// 本轮人格装配产生的降级告警；随响应回传给用户。
    pub(super) persona_warnings: Vec<String>,
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
#[path = "run_request_tests.rs"]
mod tests;
