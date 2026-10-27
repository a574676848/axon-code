//! 工具调度与进度事件的数据模型。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

#[cfg(test)]
use axon_agent_runtime::ProviderToolCallScheduleReport;
use axon_agent_runtime::{
    AgentControlRuntime, AgentGraphStore, AgentPath, AgentRunToolResultReceiptFacts,
    ProviderToolCall, SubagentSessionRuntime, WorkspaceFileSession,
};
use axon_domain::{
    CellId, DisclosurePolicy, ExecutionRequestId, ModelProfileId, PermissionSnapshotId, SessionId,
    ToolCallId, ToolExecutionPermission,
};
use serde::{Deserialize, Serialize};

use crate::run_commands::{
    AgentMemoryWriteRuntime, RunCancelToken, run_subagent_executor::RunSubagentExecutor,
};

pub(crate) type ToolCancelToken = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolKind {
    Search,
    ReadFile,
    Glob,
    MemorySearch,
    MemoryWrite,
    History,
    Notes,
    GetContextRemaining,
    NewContext,
    ListMcpResources,
    ReadMcpResource,
    UpdatePlan,
    /// 开工合同（`work_contract`）。
    WorkContract,
    GoalTool,
    Subagent,
    Mcp,
    InvokeSkill,
    Shell,
    /// 后台作业工具（`job_output` / `job_kill`）。
    ///
    /// 与 `Shell` 分开：作业工具是 `ParallelSafe`，不会像 `write_stdin`
    /// 那样把整批工具调用拖成串行。
    Job,
    Edit,
    AskUserQuestion,
    Unsupported,
}

#[derive(Clone)]
pub(crate) struct ToolOrchestratorContext {
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) cwd: PathBuf,
    pub(crate) session_id: SessionId,
    pub(crate) thread_session_id: String,
    pub(crate) cell_id: CellId,
    pub(crate) model_messages: Vec<axon_agent_runtime::AgentModelMessage>,
    pub(crate) execution_tools: Arc<RwLock<axon_agent_runtime::ToolRegistry>>,
    pub(crate) memory_write_runtime: Arc<AgentMemoryWriteRuntime>,
    pub(crate) workspace_files: Arc<WorkspaceFileSession>,
    pub(crate) timeout: Duration,
    /// 当前 turn 的 Goal 账本可见边界，继承主 turn 受众信任域。
    pub(crate) goal_disclosure_policy: DisclosurePolicy,
    /// Goal 工具继承的原 Interaction 路由；只由宿主注入。
    pub(crate) goal_transport_context: Option<serde_json::Value>,
    pub(crate) cancel_token: Option<ToolCancelToken>,
    pub(crate) run_cancel_token: Option<RunCancelToken>,
    pub(crate) model_profile_id: ModelProfileId,
    pub(crate) permission_snapshot_id: PermissionSnapshotId,
    pub(crate) current_agent_path: AgentPath,
    pub(crate) agent_control: Arc<Mutex<AgentControlRuntime>>,
    pub(crate) agent_graph_store: Arc<dyn AgentGraphStore>,
    pub(crate) subagent_sessions: Arc<Mutex<SubagentSessionRuntime>>,
    pub(crate) subagent_executor: Arc<dyn RunSubagentExecutor>,
    pub(crate) subagent_cancel_tokens: Arc<Mutex<HashMap<AgentPath, RunCancelToken>>>,
    pub(crate) user_question_port: Option<Arc<dyn axon_agent_runtime::UserQuestionPort>>,
    /// 会话级开工合同状态机；跨批次复用同一实例。
    pub(crate) work_contract: Arc<Mutex<axon_agent_runtime::WorkContractRuntime>>,
}

impl std::fmt::Debug for ToolOrchestratorContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolOrchestratorContext")
            .field("config_path", &self.config_path)
            .field("cwd", &self.cwd)
            .field("session_id", &self.session_id)
            .field("thread_session_id", &self.thread_session_id)
            .field("cell_id", &self.cell_id)
            .field("model_messages_len", &self.model_messages.len())
            .field("timeout", &self.timeout)
            .field("has_cancel_token", &self.cancel_token.is_some())
            .field("has_run_cancel_token", &self.run_cancel_token.is_some())
            .field("model_profile_id", &self.model_profile_id)
            .field("permission_snapshot_id", &self.permission_snapshot_id)
            .field("current_agent_path", &self.current_agent_path)
            .field("has_agent_control", &true)
            .field("has_agent_graph_store", &true)
            .field("has_subagent_sessions", &true)
            .field("has_subagent_executor", &true)
            .field("has_subagent_cancel_tokens", &true)
            .field("has_workspace_files", &true)
            .finish_non_exhaustive()
    }
}

impl ToolOrchestratorContext {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_token
            .as_ref()
            .is_some_and(|is_cancelled| is_cancelled())
    }

    pub(crate) fn shell_cancel_token(&self) -> Option<axon_agent_runtime::ShellCommandCancelToken> {
        self.cancel_token.as_ref().map(|token| {
            let token = Arc::clone(token);
            Arc::new(move || token()) as axon_agent_runtime::ShellCommandCancelToken
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolOutputStream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolOutputChunk {
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) part_id: String,
    pub(crate) provider_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) kind: ToolKind,
    pub(crate) stream: ToolOutputStream,
    pub(crate) sequence: u64,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolProgressInfo {
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) part_id: String,
    pub(crate) provider_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) kind: ToolKind,
    pub(crate) phase: String,
    pub(crate) items_completed: Option<u64>,
    pub(crate) items_total: Option<u64>,
    pub(crate) current_item: Option<String>,
    pub(crate) elapsed_ms: Option<u64>,
    pub(crate) message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolRuntimeEvent {
    Output(ToolOutputChunk),
    Progress(ToolProgressInfo),
}

pub(crate) type ToolRuntimeObserver<'a> = dyn FnMut(ToolRuntimeEvent) + 'a;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolReceipt {
    pub(crate) tool_call_id: u64,
    pub(crate) part_id: String,
    pub(crate) provider_call_id: String,
    pub(crate) tool_name: String,
    pub(crate) status: String,
    pub(crate) summary: String,
    pub(crate) metadata: Option<serde_json::Value>,
    pub(crate) error_type: Option<String>,
    pub(crate) remote_trigger_allowed: bool,
    pub(crate) filesystem_write_performed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output_ref: Option<axon_domain::OutputRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) duration_ms: Option<u64>,
}

impl AgentRunToolResultReceiptFacts for ToolReceipt {
    fn provider_call_id(&self) -> &str {
        &self.provider_call_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn status(&self) -> &str {
        &self.status
    }

    fn summary(&self) -> &str {
        &self.summary
    }

    fn metadata(&self) -> Option<&serde_json::Value> {
        self.metadata.as_ref()
    }

    fn output_ref(&self) -> Option<&axon_domain::OutputRef> {
        self.output_ref.as_ref()
    }

    fn error_type(&self) -> Option<&str> {
        self.error_type.as_deref()
    }

    fn remote_trigger_allowed(&self) -> bool {
        self.remote_trigger_allowed
    }

    fn filesystem_write_performed(&self) -> bool {
        self.filesystem_write_performed
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutionRequest {
    pub(crate) tool_call: ProviderToolCall,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) execution_request_id: ExecutionRequestId,
    pub(crate) part_id: String,
    pub(crate) kind: ToolKind,
    pub(crate) execution_permission: ToolExecutionPermission,
    pub(crate) concurrency_policy: axon_domain::ToolConcurrencyPolicy,
}

#[derive(Debug, Clone)]
pub(crate) struct ToolExecutionSchedule {
    #[cfg(test)]
    pub(crate) report: ProviderToolCallScheduleReport,
    pub(crate) requests: Vec<ToolExecutionRequest>,
}
