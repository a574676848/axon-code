//! Subagent session metadata 同步。

use axon_agent_runtime::{
    SubagentForkContextReport, SubagentSessionHandle, SubagentSessionStatus,
    SubagentToolExecutionReceipt,
};

use super::{SubagentToolExecutionPlan, ToolOrchestratorContext};
use crate::run_commands::run_subagent_executor::{
    RunSubagentContinuationSink, RunSubagentModelIntent,
};

const WAIT_STATUS_RUNNING: &str = "running";
const WAIT_STATUS_COMPLETED: &str = "completed";
const WAIT_STATUS_ERRORED: &str = "errored";

pub(super) fn sync_messaged_subagent_session(
    context: &ToolOrchestratorContext,
    mut receipt: SubagentToolExecutionReceipt,
) -> axon_domain::AxonResult<SubagentToolExecutionPlan> {
    let Some(task_path) = receipt_agent_path(&receipt) else {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    };
    // 目标空闲但合法（已终态、尚未关闭）时，投递成功只是把消息放进了它的 mailbox：
    // 必须由宿主重挂会话、开启新轮次，否则消息会一直躺着（合同 §5.3.5 第二语义）。
    if wake_required(&receipt) {
        return wake_recovered_subagent(context, receipt, &task_path);
    }
    let summary = format!("message queued for {task_path}");
    let mut sessions = context
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(handle) = sessions.update_running_summary_by_task_path(&task_path, summary) {
        receipt.payload = merge_running_session_payload(&receipt.payload, &handle);
    }
    drop(sessions);
    set_turn_started(&mut receipt.payload, false);
    Ok(SubagentToolExecutionPlan::Ready(receipt))
}

fn set_turn_started(payload: &mut serde_json::Value, started: bool) {
    if let Some(payload) = payload.as_object_mut() {
        payload.insert("turnStarted".to_owned(), serde_json::Value::Bool(started));
    }
}

fn receipt_agent_path(receipt: &SubagentToolExecutionReceipt) -> Option<String> {
    receipt
        .payload
        .get("agent")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// 回执是否请求唤醒目标（由 `send_message` 的准入裁决写入）。
fn wake_required(receipt: &SubagentToolExecutionReceipt) -> bool {
    receipt
        .payload
        .get("wakeRequired")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// 唤醒已恢复的子代理开启新轮次（合同 §5.3.5 第二语义）。
///
/// 会话级 re-attach 的凭据是**子会话 ID 的确定性推导**：
/// `child_session_id(父会话, AgentPath)` 与终态写入 continuation 引用时是同一个口径，
/// 因此新执行的 `prepare_run_session` 会自然读到同一份子会话历史，从上一轮的上下文
/// 继续对话（设计 §6.2 的「按需触发」在最小闭环里就是这一步）。
///
/// 前置校验是「该子代理确有已落盘的终态引用」：通过不了就退回普通回执，
/// 消息仍留在它的 mailbox 里且状态可查，不假装唤醒成功。
fn wake_recovered_subagent(
    context: &ToolOrchestratorContext,
    receipt: SubagentToolExecutionReceipt,
    task_path: &str,
) -> axon_domain::AxonResult<SubagentToolExecutionPlan> {
    let message = receipt
        .payload
        .get("message")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default();
    if message.trim().is_empty() {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    }
    let (Some(config_path), Ok(path)) = (
        context.config_path.as_deref(),
        axon_agent_runtime::AgentPath::parse(task_path),
    ) else {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    };
    let Ok(sink) = RunSubagentContinuationSink::open(
        config_path,
        context.thread_session_id.clone(),
        context.thread_session_id.clone(),
    ) else {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    };
    if !sink.has_terminal_reference(&path) {
        return Ok(SubagentToolExecutionPlan::Ready(receipt));
    }
    // 唤醒是后台执行：父代理不该为了「叫醒一个空闲子代理」而阻塞，
    // 结果仍按统一的完成通知回流。
    let execution = super::subagent_execution_input(
        task_path,
        message,
        // 唤醒不是新委派：不复制父代理的任何历史，续接的是**子代理自己的**会话。
        &SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        crate::tool_orchestrator::now_ms(),
        RunSubagentModelIntent::Inherit,
    )?;
    Ok(SubagentToolExecutionPlan::Run {
        receipt,
        input: Box::new(execution),
        run_in_background: true,
    })
}

fn session_status_name(status: SubagentSessionStatus) -> &'static str {
    match status {
        SubagentSessionStatus::Running => WAIT_STATUS_RUNNING,
        SubagentSessionStatus::Completed => WAIT_STATUS_COMPLETED,
        SubagentSessionStatus::Refused => "refused",
        SubagentSessionStatus::Errored => WAIT_STATUS_ERRORED,
        SubagentSessionStatus::Cancelled => "cancelled",
    }
}

fn session_payload(handle: &SubagentSessionHandle) -> serde_json::Value {
    serde_json::json!({
        "taskName": handle.task_path,
        "parentSessionId": handle.parent_session_id.0,
        "sessionId": handle.session_id.0,
        "executionId": handle
            .execution_id
            .map(axon_agent_runtime::AgentExecutionId::get),
        "forkedFromSessionId": handle.forked_from_session_id.map(|id| id.0),
        "paneId": handle.pane_id.0,
        "cellId": handle.cell_id.0,
        "longRunningJobId": handle.long_running_job_id.0,
        "modelProfileId": handle.model_profile_id.0,
        "permissionSnapshotId": handle.permission_snapshot_id.0,
        "status": session_status_name(handle.status),
        "summary": handle.summary,
    })
}

fn fork_context_payload(report: &SubagentForkContextReport) -> serde_json::Value {
    serde_json::json!({
        "originalCount": report.original_count,
        "retainedCount": report.retained_count,
        "droppedCount": report.dropped_count,
        "reasons": report.reasons,
    })
}

fn running_session_payload(handle: &SubagentSessionHandle) -> serde_json::Value {
    serde_json::json!({
        "taskName": handle.task_path,
        "parentSessionId": handle.parent_session_id.0,
        "sessionId": handle.session_id.0,
        "executionId": handle
            .execution_id
            .map(axon_agent_runtime::AgentExecutionId::get),
        "forkedFromSessionId": handle.forked_from_session_id.map(|id| id.0),
        "paneId": handle.pane_id.0,
        "cellId": handle.cell_id.0,
        "longRunningJobId": handle.long_running_job_id.0,
        "modelProfileId": handle.model_profile_id.0,
        "permissionSnapshotId": handle.permission_snapshot_id.0,
        "status": "running",
        "summary": handle.summary,
    })
}

pub(super) fn merge_sessions_payload(
    payload: &serde_json::Value,
    handles: &[SubagentSessionHandle],
    fork_context: Option<&SubagentForkContextReport>,
) -> serde_json::Value {
    let mut payload = payload.as_object().cloned().unwrap_or_default();
    payload.insert(
        "sessions".to_owned(),
        serde_json::Value::Array(handles.iter().map(session_payload).collect()),
    );
    if let Some(fork_context) = fork_context {
        payload.insert("forkContext".to_owned(), fork_context_payload(fork_context));
    }
    serde_json::Value::Object(payload)
}

pub(super) fn merge_spawn_session_payload(
    payload: &serde_json::Value,
    task_name: &str,
    handle: &SubagentSessionHandle,
    fork_context: &SubagentForkContextReport,
) -> serde_json::Value {
    let mut payload = payload.as_object().cloned().unwrap_or_default();
    let mut session = running_session_payload(handle);
    if let Some(session) = session.as_object_mut() {
        session.insert(
            "taskName".to_owned(),
            serde_json::Value::String(task_name.to_owned()),
        );
    }
    payload.insert("session".to_owned(), session);
    payload.insert("forkContext".to_owned(), fork_context_payload(fork_context));
    serde_json::Value::Object(payload)
}

pub(super) fn merge_running_session_payload(
    payload: &serde_json::Value,
    handle: &SubagentSessionHandle,
) -> serde_json::Value {
    let mut payload = payload.as_object().cloned().unwrap_or_default();
    payload.insert("session".to_owned(), running_session_payload(handle));
    serde_json::Value::Object(payload)
}
