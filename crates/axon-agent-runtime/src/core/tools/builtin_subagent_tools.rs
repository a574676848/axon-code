//! 子代理协作工具面。
//!
//! 模型通过工具显式创建、列表与向子代理发消息；等待与终止**不在本层**：
//! 子代理是临时 Job，生命周期操作（读输出、等待、终止）一律走
//! `builtin.job_output` / `builtin.job_kill` 的 `job_id` 通道，协作寻址才走 AgentPath。
//! 当前层只落到 [`AgentControlRuntime`] 的确定性控制面，不伪造真实 mailbox 或线程执行。

use axon_domain::{AxonError, AxonResult};
use axon_time::unix_now_ms;
use serde::{Deserialize, Serialize};

use crate::AgentModelMessage;
use crate::agent_communication::AgentStatus;
use crate::agent_control_mailbox::{AgentMessageRequest, AgentMessageTrigger};
use crate::agent_control_runtime::{
    AGENT_CONTROL_REASON_NOT_FOUND, AgentControlRuntime, AgentPath, AgentSpawnRequest,
};
use crate::agent_mailbox_persistence::AgentMailboxMessageKind;
use crate::agent_runtime_note::{AgentRuntimeNoteKind, agent_mailbox_note};
/// 测试与同 crate 调用方仍需按名字寻址目标字段：`send_message` 的 target 由
/// [`builtin_subagent_tool_args`] 解析，本模块只保留常量出口。
#[cfg(test)]
pub use crate::builtin_subagent_tool_spec::SUBAGENT_TOOL_TARGET_FIELD;
pub use crate::builtin_subagent_tool_spec::{
    DELEGATE_TO_FAST_TOOL_NAME, LIST_AGENTS_TOOL_ID, LIST_AGENTS_TOOL_NAME, SEND_MESSAGE_TOOL_ID,
    SEND_MESSAGE_TOOL_NAME, SPAWN_AGENT_TOOL_ID, SPAWN_AGENT_TOOL_NAME,
    SUBAGENT_TOOL_FORK_TURNS_FIELD, SUBAGENT_TOOL_MESSAGE_FIELD, SUBAGENT_TOOL_PATH_PREFIX_FIELD,
    SUBAGENT_TOOL_SCHEMA_VERSION, SUBAGENT_TOOL_TASK_FIELD, SUBAGENT_TOOL_TASK_NAME_FIELD,
    list_agents_tool_definition, send_message_tool_definition, spawn_agent_tool_definition,
    subagent_tool_definitions,
};
use crate::provider_tool_call::ProviderToolCall;

#[path = "builtin_subagent_tool_args.rs"]
mod builtin_subagent_tool_args;

use builtin_subagent_tool_args::{
    DelegateArgs, ListAgentsArgs, SendMessageArgs, optional_string, parse_arguments,
    parse_delegate_args, parse_list_agents_args, parse_send_message_args, required_string,
    required_task_name, subagent_tool_name, validate_allowed_fields, validate_fork_turns,
};

/// 子代理工具调用已完成。
pub const SUBAGENT_TOOL_REASON_EXECUTED: &str = "subagent_tool:executed";
/// 子代理工具拒绝 root 作为目标。
pub const SUBAGENT_TOOL_REASON_ROOT_TARGET_REJECTED: &str = "subagent_tool:root_target_rejected";
/// 子代理工具拒绝当前代理作为目标。
pub const SUBAGENT_TOOL_REASON_SELF_TARGET_REJECTED: &str = "subagent_tool:self_target_rejected";
/// 子代理工具拒绝远控触发。
pub const SUBAGENT_TOOL_REASON_REMOTE_TRIGGER_CLOSED: &str = "subagent_tool:remote_trigger_closed";
/// 子代理拒绝继续派生子代理。
pub const SUBAGENT_TOOL_REASON_CHILD_SPAWN_BLOCKED: &str = "subagent_tool:child_spawn_blocked";
/// 子代理工具参数包含未知字段。
pub const SUBAGENT_TOOL_REASON_UNKNOWN_FIELD: &str = "subagent_tool:unknown_field";
const DELEGATE_FAST_TASK_NAME: &str = "delegate_fast";
const DELEGATE_KIND_FAST: &str = "fast";
const DELEGATE_FAST_MODEL_POLICY: &str = "fast_model";
const SUBAGENT_FORK_TURNS_NONE: &str = "none";
/// 委托模式字段：缺失时回执表示后台作业（带 `jobId`）。
const SUBAGENT_PAYLOAD_RUN_IN_BACKGROUND: &str = "runInBackground";

/// 子代理工具执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentToolExecutionInput {
    /// provider 归一化工具调用。
    pub tool_call: ProviderToolCall,
    /// 当前代理路径。
    pub current_path: AgentPath,
    /// 是否远控触发。
    pub remote_requested: bool,
    /// 完成时间。
    pub completed_at_ms: u64,
}

/// 子代理工具执行状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentToolExecutionStatus {
    /// 完成。
    Completed,
    /// 阻断。
    Blocked,
}

/// 子代理工具请求的结构化模型意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentModelIntent {
    /// 继承父 Agent 当前模型。
    Inherit,
    /// 使用快速模型槽位。
    Fast,
}

/// 子代理工具执行回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentToolExecutionReceipt {
    /// provider call ID。
    pub provider_call_id: String,
    /// provider 工具名。
    pub tool_name: String,
    /// 结构化模型选择意图；执行器不得再从 payload 字符串推断。
    pub model_intent: SubagentModelIntent,
    /// 执行状态。
    pub status: SubagentToolExecutionStatus,
    /// 有界摘要。
    pub bounded_summary: String,
    /// 结构化结果。
    pub payload: serde_json::Value,
    /// mailbox 中被消费的独立模型消息。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_messages: Vec<AgentModelMessage>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否发生文件写入。
    pub filesystem_write_performed: bool,
    /// 完成时间。
    pub completed_at_ms: u64,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 子代理工具 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SubagentToolRuntime;

impl SubagentToolRuntime {
    /// 判断 provider 工具调用是否是子代理工具。
    #[must_use]
    pub fn is_subagent_tool(tool_name: &str) -> bool {
        subagent_tool_name(tool_name).is_some()
    }

    /// 判断工具是否会创建新的子代理执行分支。
    #[must_use]
    pub fn is_spawn_tool(tool_name: &str) -> bool {
        matches!(
            subagent_tool_name(tool_name),
            Some(SPAWN_AGENT_TOOL_NAME | DELEGATE_TO_FAST_TOOL_NAME)
        )
    }

    /// 执行子代理工具。
    pub fn execute(
        &self,
        control: &mut AgentControlRuntime,
        input: SubagentToolExecutionInput,
    ) -> AxonResult<SubagentToolExecutionReceipt> {
        let Some(tool_name) = subagent_tool_name(input.tool_call.dispatch_name()) else {
            return Err(AxonError::tool_invalid_definition(
                "provider tool-call 不是子代理工具",
            ));
        };
        if input.remote_requested {
            return Ok(blocked_receipt(
                input,
                "子代理工具拒绝远控触发",
                SUBAGENT_TOOL_REASON_REMOTE_TRIGGER_CLOSED,
            ));
        }
        if is_fast_subagent(control, &input.current_path) && Self::is_spawn_tool(tool_name) {
            return Ok(blocked_receipt(
                input,
                "Fast 子代理禁止继续派生子代理",
                SUBAGENT_TOOL_REASON_CHILD_SPAWN_BLOCKED,
            ));
        }
        let arguments = parse_arguments(&input.tool_call.input)?;
        // mailbox 的第二个消费出口：子代理工具调用是当前代理 turn 内的一个 step 边界。
        // 运行期间到达的协作消息在这里被取走，并以 AgentMailbox runtime note 的形式
        // 随回执作为独立模型消息下发。它与「请求构造前消费」共用同一个 drain 出口，
        // 因此同一批消息只可能走其中一条（M3 设计 §7.5 的注入通道唯一性）。
        let mailbox_messages = take_mailbox_model_messages(control, &input.current_path)?;
        let outcome = match tool_name {
            SPAWN_AGENT_TOOL_NAME => SubagentToolOutcome::completed(execute_spawn(
                control,
                &input.current_path,
                arguments,
            )?),
            SEND_MESSAGE_TOOL_NAME => SubagentToolOutcome::completed(execute_message(
                control,
                &input.current_path,
                arguments,
            )?),
            LIST_AGENTS_TOOL_NAME => SubagentToolOutcome::completed(execute_list(
                control,
                &input.current_path,
                arguments,
            )?),
            DELEGATE_TO_FAST_TOOL_NAME => execute_delegate(control, &input.current_path, arguments)
                .map(SubagentToolOutcome::completed)?,
            _ => unreachable!("subagent_tool_name 只返回已知工具"),
        };
        let mut receipt = outcome.into_receipt(input);
        if !mailbox_messages.is_empty() {
            receipt.model_messages = mailbox_messages;
        }
        Ok(receipt)
    }
}

/// 取走当前代理 mailbox 的待读消息并渲染为 mailbox runtime note。
///
/// 读确认在 drain 时同步完成；没有消息时返回空向量，不产生任何注入。
fn take_mailbox_model_messages(
    control: &mut AgentControlRuntime,
    current_path: &AgentPath,
) -> AxonResult<Vec<AgentModelMessage>> {
    let messages = control.take_mailbox_messages(current_path, now_ms())?;
    Ok(messages
        .into_iter()
        .map(|message| {
            AgentModelMessage::runtime_note(
                AgentRuntimeNoteKind::AgentMailbox,
                agent_mailbox_note(message.author.as_str(), &message.content),
            )
        })
        .collect())
}

struct SubagentToolOutcome {
    status: SubagentToolExecutionStatus,
    payload: serde_json::Value,
    model_messages: Vec<AgentModelMessage>,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpawnAgentToolRequest {
    parent_path: AgentPath,
    task_name: String,
    message: String,
    fork_turns: Option<String>,
    role: Option<String>,
}

impl SubagentToolOutcome {
    fn completed(payload: serde_json::Value) -> Self {
        Self {
            status: SubagentToolExecutionStatus::Completed,
            payload,
            model_messages: Vec::new(),
            reasons: vec![SUBAGENT_TOOL_REASON_EXECUTED.to_owned()],
        }
    }

    fn into_receipt(self, input: SubagentToolExecutionInput) -> SubagentToolExecutionReceipt {
        execution_receipt(
            input,
            self.payload,
            self.model_messages,
            self.status,
            self.reasons,
        )
    }
}

fn execute_spawn(
    control: &mut AgentControlRuntime,
    current_path: &AgentPath,
    arguments: serde_json::Value,
) -> AxonResult<serde_json::Value> {
    validate_allowed_fields(
        &arguments,
        &[
            SUBAGENT_TOOL_TASK_NAME_FIELD,
            SUBAGENT_TOOL_TASK_FIELD,
            SUBAGENT_TOOL_FORK_TURNS_FIELD,
        ],
    )?;
    let task_name = required_task_name(&arguments)?;
    let task = required_string(&arguments, SUBAGENT_TOOL_TASK_FIELD)?;
    let fork_turns = optional_string(&arguments, SUBAGENT_TOOL_FORK_TURNS_FIELD);
    validate_fork_turns(fork_turns.as_deref())?;
    execute_spawn_agent_request(
        control,
        SpawnAgentToolRequest {
            parent_path: current_path.clone(),
            task_name,
            message: task,
            fork_turns,
            role: None,
        },
    )
}

fn execute_spawn_agent_request(
    control: &mut AgentControlRuntime,
    request: SpawnAgentToolRequest,
) -> AxonResult<serde_json::Value> {
    let report = control.spawn(AgentSpawnRequest {
        parent_path: request.parent_path,
        task_name: request.task_name,
        message: request.message.clone(),
        nickname: None,
        role: request.role.clone(),
    })?;
    Ok(serde_json::json!({
        "taskName": report.path.to_string(),
        "task": request.message,
        "forkTurns": request.fork_turns.unwrap_or_else(|| SUBAGENT_FORK_TURNS_NONE.to_owned()),
        "nickname": report.nickname,
        "status": status_name(&report.status),
        "threadId": report.thread_id,
        "role": request.role,
    }))
}

fn execute_message(
    control: &mut AgentControlRuntime,
    current_path: &AgentPath,
    arguments: serde_json::Value,
) -> AxonResult<serde_json::Value> {
    let SendMessageArgs { target, message } = parse_send_message_args(&arguments)?;
    let target = resolve_collaboration_target(control, current_path, &target)?;
    let report = control.send_message(AgentMessageRequest {
        author: current_path.clone(),
        recipient: target.clone(),
        content: message.clone(),
        trigger: AgentMessageTrigger::QueueOnly,
        created_at_ms: now_ms(),
        kind: AgentMailboxMessageKind::Collab,
    })?;
    Ok(serde_json::json!({
        "agent": report.recipient.to_string(),
        "author": report.author.to_string(),
        "message": message,
        "turnRequested": report.trigger_turn,
        "queuedCount": report.queued_count,
        "status": status_name(&report.recipient_status),
        // 投递结果如实回报：目标暂时不可接收时为 pending，消息已持久化并等待重试，
        // 不是静默丢弃（合同 §5.3.2）。
        "deliveryState": report.delivery_state.as_str(),
        // 目标空闲但合法：消息已进它的 mailbox，但只有宿主能唤醒它开新一轮
        // （合同 §5.3.5 第二语义）。编排层据此决定是否重挂会话。
        "wakeRequired": report.wake_required,
    }))
}

fn execute_list(
    control: &AgentControlRuntime,
    current_path: &AgentPath,
    arguments: serde_json::Value,
) -> AxonResult<serde_json::Value> {
    let ListAgentsArgs { path_prefix } = parse_list_agents_args(&arguments)?;
    // 显式前缀与 `send_message` 的 target 走同一条校验链；省略前缀表示不限定目标，
    // 列出当前可见的整棵代理树（这不是 root 寻址，只是默认作用域）。
    if let Some(prefix) = path_prefix.as_deref() {
        resolve_collaboration_target(control, current_path, prefix)?;
    }
    let agents = control
        .list_agents(current_path, path_prefix.as_deref())?
        .into_iter()
        .map(|agent| {
            serde_json::json!({
                "agentName": agent.agent_name,
                "status": status_name(&agent.status),
                "lastTaskMessage": agent.last_task_message,
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "agents": agents }))
}

fn execute_delegate(
    control: &mut AgentControlRuntime,
    current_path: &AgentPath,
    arguments: serde_json::Value,
) -> AxonResult<serde_json::Value> {
    let DelegateArgs {
        task: message,
        run_in_background,
    } = parse_delegate_args(&arguments)?;
    let task_name = next_delegate_task_name(control, current_path, DELEGATE_FAST_TASK_NAME);
    let mut payload = execute_spawn_agent_request(
        control,
        SpawnAgentToolRequest {
            parent_path: current_path.clone(),
            task_name,
            message,
            fork_turns: Some(SUBAGENT_FORK_TURNS_NONE.to_owned()),
            role: Some(DELEGATE_KIND_FAST.to_owned()),
        },
    )?;
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "delegateKind".to_owned(),
            serde_json::Value::String(DELEGATE_KIND_FAST.to_owned()),
        );
        object.insert(
            "modelPolicy".to_owned(),
            serde_json::Value::String(DELEGATE_FAST_MODEL_POLICY.to_owned()),
        );
        // 等待模式是执行层的事实，如实进载荷：编排层据此决定注册作业还是同步等待。
        object.insert(
            SUBAGENT_PAYLOAD_RUN_IN_BACKGROUND.to_owned(),
            serde_json::Value::Bool(run_in_background),
        );
    }
    Ok(payload)
}

fn next_delegate_task_name(
    control: &AgentControlRuntime,
    current_path: &AgentPath,
    base_task_name: &str,
) -> String {
    let mut index = 1_u32;
    loop {
        let task_name = format!("{base_task_name}_{index}");
        if current_path
            .join(&task_name)
            .is_ok_and(|path| control.status(&path) == AgentStatus::NotFound)
        {
            return task_name;
        }
        index = index.saturating_add(1);
    }
}

fn resolve_non_root(
    control: &AgentControlRuntime,
    current_path: &AgentPath,
    reference: &str,
) -> AxonResult<AgentPath> {
    let path = current_path.resolve(reference)?;
    if path.is_root() {
        return Err(AxonError::tool_invalid_arguments(
            SUBAGENT_TOOL_REASON_ROOT_TARGET_REJECTED,
        ));
    }
    if control.status(&path) == AgentStatus::NotFound {
        return Err(AxonError::tool_invalid_arguments(
            AGENT_CONTROL_REASON_NOT_FOUND,
        ));
    }
    Ok(path)
}

/// 协作工具（`send_message`、`list_agents`）的显式目标解析。
///
/// 两个工具的显式目标遵循**同一条**校验链与同一批拒绝理由：root 不可寻址
/// （它是主代理自身）、self 不可寻址（向自己发消息或列举自己的子树都是无意义回路）、
/// 不存在的目标统一报 not_found。类型化差异只保留在「省略前缀 = 不限定目标」这一
/// 个默认值上，见 [`ListAgentsArgs`]。
fn resolve_collaboration_target(
    control: &AgentControlRuntime,
    current_path: &AgentPath,
    reference: &str,
) -> AxonResult<AgentPath> {
    let path = resolve_non_root(control, current_path, reference)?;
    if path == *current_path {
        return Err(AxonError::tool_invalid_arguments(
            SUBAGENT_TOOL_REASON_SELF_TARGET_REJECTED,
        ));
    }
    Ok(path)
}

fn status_name(status: &AgentStatus) -> String {
    match status {
        AgentStatus::PendingInit => "pending_init".to_owned(),
        AgentStatus::Running => "running".to_owned(),
        AgentStatus::Interrupted => "interrupted".to_owned(),
        AgentStatus::Shutdown => "shutdown".to_owned(),
        AgentStatus::NotFound => "not_found".to_owned(),
        AgentStatus::Completed(message) => message.as_ref().map_or_else(
            || "completed".to_owned(),
            |message| format!("completed:{message}"),
        ),
        AgentStatus::Refused(message) => format!("refusal:{message}"),
        AgentStatus::Errored(message) => format!("errored:{message}"),
    }
}

fn execution_receipt(
    input: SubagentToolExecutionInput,
    payload: serde_json::Value,
    model_messages: Vec<AgentModelMessage>,
    status: SubagentToolExecutionStatus,
    reasons: Vec<String>,
) -> SubagentToolExecutionReceipt {
    let bounded_summary = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned());
    SubagentToolExecutionReceipt {
        provider_call_id: input.tool_call.id.clone(),
        tool_name: input.tool_call.dispatch_name().to_owned(),
        model_intent: model_intent_for_tool_name(input.tool_call.dispatch_name()),
        status,
        bounded_summary,
        payload,
        model_messages,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        completed_at_ms: input.completed_at_ms,
        reasons,
    }
}

fn blocked_receipt(
    input: SubagentToolExecutionInput,
    summary: &str,
    reason: &str,
) -> SubagentToolExecutionReceipt {
    SubagentToolExecutionReceipt {
        provider_call_id: input.tool_call.id.clone(),
        tool_name: input.tool_call.dispatch_name().to_owned(),
        model_intent: model_intent_for_tool_name(input.tool_call.dispatch_name()),
        status: SubagentToolExecutionStatus::Blocked,
        bounded_summary: summary.to_owned(),
        payload: serde_json::json!({ "blocked": true, "reason": reason }),
        model_messages: Vec::new(),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        completed_at_ms: input.completed_at_ms,
        reasons: vec![reason.to_owned()],
    }
}

fn model_intent_for_tool_name(tool_name: &str) -> SubagentModelIntent {
    match tool_name {
        DELEGATE_TO_FAST_TOOL_NAME | "builtin.delegate_to_fast" => SubagentModelIntent::Fast,
        _ => SubagentModelIntent::Inherit,
    }
}

fn is_fast_subagent(control: &AgentControlRuntime, path: &AgentPath) -> bool {
    control
        .controlled_agent(path)
        .ok()
        .and_then(|agent| agent.role.as_deref())
        .is_some_and(|role| role == DELEGATE_KIND_FAST)
        || is_fast_delegate_path(path)
}

fn is_fast_delegate_path(path: &AgentPath) -> bool {
    let name = path.name();
    name == DELEGATE_FAST_TASK_NAME
        || name
            .strip_prefix(DELEGATE_FAST_TASK_NAME)
            .is_some_and(|suffix| suffix.starts_with('_'))
}

fn now_ms() -> u64 {
    unix_now_ms()
}
