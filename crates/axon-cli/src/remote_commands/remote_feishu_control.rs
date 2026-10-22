//! Feishu 控制命令的 Daemon 生命周期事务与 IPC 适配。

use std::time::Duration;

use axon_daemon::infrastructure::ipc::JsonlIpcClient;
use axon_domain::{DaemonSessionId, InteractionRecord, SessionId};
use axon_protocol::{DaemonCommand, DaemonRequest, SubmitInteractionRequest};
use axon_remote_feishu::{
    RemoteFeishuFeedbackTaskStoreRuntime, RemoteFeishuProcessingCleanupTaskInput,
    RemoteFeishuProcessingOutcome, RemoteFeishuProcessingStateStoreRuntime,
};
use axon_remote_runtime::{
    RemoteControlCommand, RemoteControlDispatchAction, RemoteControlMessagePlan,
};
use serde_json::Value;

use super::super::RemoteCliContext;
use super::{
    FEISHU_DAEMON_SESSION_PREFIX, REMOTE_FEISHU_GATEWAY_PAIRING_ID,
    remote_feishu_gateway_mapping::{reply_message_id, reply_thread_id},
};
use crate::{
    remote_commands::{REMOTE_FEISHU_DAEMON_ENDPOINT, now_ms},
    run_commands::run_remote_execution::{RemoteAgentExecutionInput, enqueue_remote_agent_reply},
    runtime_paths::daemon_endpoint_for_root,
};
const FEISHU_DAEMON_CLIENT_ID: &str = "axon-feishu-gateway";
const RUN_SESSION_ID_FIELD: &str = "runSessionId";
/// Daemon IPC 单次往返上限。
///
/// Daemon 卡死时入站 worker 必须能失败返回并按 durable 事件源重试，而不是永久
/// 停在这一条消息上；超时不改变幂等语义：重投由 Interaction ID 去重。
const FEISHU_DAEMON_IPC_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn submit_control_lifecycle(
    context: &RemoteCliContext,
    endpoint: &str,
    numeric_session_id: u64,
    event_report: &axon_remote_feishu::RemoteFeishuGatewayGovernanceEventReport,
    plan: &RemoteControlMessagePlan,
) -> Result<(), String> {
    let endpoint = daemon_endpoint_for_root_if_default(endpoint, &context.axon_root);
    if plan.action != RemoteControlDispatchAction::InlineControl {
        return Err("普通 Feishu 输入必须通过 SubmitInteraction 提交".to_owned());
    }
    execute_inline_control(context, &endpoint, numeric_session_id, event_report, plan)
}

pub(crate) fn submit_gateway_interaction(
    context: &RemoteCliContext,
    endpoint: &str,
    interaction: &InteractionRecord,
) -> Result<(), String> {
    let workspace = crate::run_commands::register_workspace(
        &context.axon_root,
        interaction.received_at_ms,
        Some(&context.config_path),
    )?;
    if interaction
        .scope
        .workspace_id()
        .map(axon_domain::WorkspaceId::as_str)
        != Some(workspace.workspace_id.as_str())
    {
        return Err("Feishu Interaction Workspace 与运行根目录不一致".to_owned());
    }
    let endpoint = daemon_endpoint_for_root_if_default(endpoint, &context.axon_root);
    let request = DaemonRequest {
        message_type: "request".to_owned(),
        request_id: format!(
            "{FEISHU_DAEMON_CLIENT_ID}-interaction-{}",
            interaction.interaction_id.as_str()
        ),
        protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
        client_id: FEISHU_DAEMON_CLIENT_ID.to_owned(),
        session_id: None,
        command: DaemonCommand::SubmitInteraction(SubmitInteractionRequest {
            interaction: interaction.clone(),
        }),
    };
    let mut client = JsonlIpcClient::connect(&endpoint)
        .map_err(|error| format!("连接 Daemon IPC 失败：{error}"))?;
    let _ = send_daemon_request(&mut client, &request, "提交 Feishu Interaction")?;
    Ok(())
}

fn daemon_endpoint_for_root_if_default(endpoint: &str, root: &std::path::Path) -> String {
    if endpoint == REMOTE_FEISHU_DAEMON_ENDPOINT {
        daemon_endpoint_for_root(root)
    } else {
        endpoint.to_owned()
    }
}

fn execute_inline_control(
    context: &RemoteCliContext,
    endpoint: &str,
    numeric_session_id: u64,
    event_report: &axon_remote_feishu::RemoteFeishuGatewayGovernanceEventReport,
    plan: &RemoteControlMessagePlan,
) -> Result<(), String> {
    let session_id = gateway_session_id(numeric_session_id)?;
    let (run_session_id, response) = match plan.command {
        Some(RemoteControlCommand::Stop) => {
            return Err(
                "Feishu Stop 控制已移除；请提交自然语言 Interaction 请求取消对应 Task".to_owned(),
            );
        }
        Some(RemoteControlCommand::Status) => {
            let body = query_gateway_state(endpoint, &session_id, &event_report.meta.request_id)?;
            let run_session_id = required_run_session_id(&body)?;
            let status = body
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            (run_session_id, status_control_response(status))
        }
        Some(RemoteControlCommand::Queue) => {
            return Err("Feishu Queue 控制已移除；请通过 Activity 查询当前任务".to_owned());
        }
        None => return Err("Feishu 控制命令无效".to_owned()),
    };
    enqueue_processing_cleanup(
        context,
        numeric_session_id,
        event_report,
        &format!("feishu-control-cleanup:{}", plan.delivery_id),
        RemoteFeishuProcessingOutcome::Success,
    )?;
    enqueue_control_reply(
        context,
        numeric_session_id,
        event_report,
        &run_session_id,
        &response,
    )
}

/// 把「结束 Typing」写成持久清理任务，由 Daemon feedback worker 异步收口。
///
/// 入站主链（含控制失败路径）不得同步等待飞书 reaction API：任务写入失败即上报，
/// 写入成功后由后台 tick 执行，进程重启也不会丢失这次收尾。
pub(super) fn enqueue_processing_cleanup(
    context: &RemoteCliContext,
    numeric_session_id: u64,
    event_report: &axon_remote_feishu::RemoteFeishuGatewayGovernanceEventReport,
    dispatch_id: &str,
    outcome: RemoteFeishuProcessingOutcome,
) -> Result<(), String> {
    if !context.feishu_websocket.enabled || !context.feishu_websocket.enable_notification_sending {
        return Ok(());
    }
    let Some(message_id) = event_report
        .meta
        .message_id
        .as_deref()
        .filter(|value| value.starts_with("om_"))
    else {
        return Ok(());
    };
    let processing_path =
        RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(&context.root);
    if RemoteFeishuProcessingStateStoreRuntime::active_reaction(&processing_path, message_id)
        .map_err(|error| format!("读取旧 Feishu Typing 状态失败: {error:?}"))?
        .is_none()
    {
        return Ok(());
    }
    let config = &context.feishu_websocket;
    let task = RemoteFeishuFeedbackTaskStoreRuntime::task_from_cleanup(
        RemoteFeishuProcessingCleanupTaskInput {
            app_id: &config.app_id,
            app_secret_env: &config.app_secret_env,
            session_id: SessionId(numeric_session_id),
            dispatch_id,
            message_id,
            outcome,
            api_base_url: (!config.api_base_url.trim().is_empty())
                .then_some(config.api_base_url.as_str()),
            recorded_at_ms: now_ms(),
        },
    );
    RemoteFeishuFeedbackTaskStoreRuntime::append_record(
        RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&context.root),
        &task,
    )
    .map_err(|error| format!("写入 Feishu Typing 清理任务失败: {error:?}"))
}

fn status_control_response(status: &str) -> String {
    format!("当前状态: {status}")
}

fn enqueue_control_reply(
    context: &RemoteCliContext,
    numeric_session_id: u64,
    event_report: &axon_remote_feishu::RemoteFeishuGatewayGovernanceEventReport,
    run_session_id: &str,
    body: &str,
) -> Result<(), String> {
    let thread_id = reply_thread_id(event_report);
    let reply_to_message_id = reply_message_id(event_report);
    let input = RemoteAgentExecutionInput {
        axon_root: &context.root,
        axon_session_id: SessionId(numeric_session_id),
        run_session_key: run_session_id,
        feishu_chat_id: &event_report.meta.chat_id,
        processing_message_id: None,
        reply_to_message_id: reply_to_message_id.as_deref(),
        thread_id: thread_id.as_deref(),
        pairing_id: REMOTE_FEISHU_GATEWAY_PAIRING_ID,
        delivery_id: &event_report.meta.request_id,
        recorded_at_ms: now_ms(),
    };
    enqueue_remote_agent_reply(&input, body).map(|_| {
        super::super::super::notify_delivery_waker();
    })
}

fn gateway_session_id(numeric_session_id: u64) -> Result<DaemonSessionId, String> {
    DaemonSessionId::new(format!(
        "{FEISHU_DAEMON_SESSION_PREFIX}{numeric_session_id}"
    ))
    .map_err(|error| format!("Feishu Daemon Session ID 无效：{error}"))
}

fn required_run_session_id(body: &Value) -> Result<String, String> {
    body.get(RUN_SESSION_ID_FIELD)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "Daemon 响应缺少 runSessionId".to_owned())
}

fn query_gateway_state(
    endpoint: &str,
    session_id: &DaemonSessionId,
    action_id: &str,
) -> Result<Value, String> {
    let request = daemon_request(
        format!("{FEISHU_DAEMON_CLIENT_ID}-status-{action_id}"),
        session_id,
        DaemonCommand::GetState {
            session_id: session_id.to_string(),
        },
    );
    send_control_request(endpoint, &request, "查询 Feishu Daemon Session")
}

fn send_control_request(
    endpoint: &str,
    request: &DaemonRequest,
    action: &str,
) -> Result<Value, String> {
    let mut client = JsonlIpcClient::connect(endpoint)
        .map_err(|error| format!("连接 Daemon IPC 失败：{error}"))?;
    send_daemon_request(&mut client, request, action)
}

fn daemon_request(
    request_id: String,
    session_id: &DaemonSessionId,
    command: DaemonCommand,
) -> DaemonRequest {
    DaemonRequest {
        message_type: "command".to_owned(),
        request_id,
        protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
        client_id: FEISHU_DAEMON_CLIENT_ID.to_owned(),
        session_id: Some(session_id.to_string()),
        command,
    }
}

fn send_daemon_request(
    client: &mut JsonlIpcClient,
    request: &DaemonRequest,
    action: &str,
) -> Result<serde_json::Value, String> {
    let response = client
        .send_with_timeout(request, FEISHU_DAEMON_IPC_TIMEOUT)
        .map_err(|error| format!("{action}失败：{error}"))?;
    if response.ok {
        Ok(response.body)
    } else {
        Err(format!("{action}被拒绝：{}", response.body))
    }
}

#[cfg(test)]
mod tests {
    use super::status_control_response;

    const INTERNAL_RUN_SESSION_ID: &str = "run-session-internal-42";

    #[test]
    fn user_visible_control_responses_do_not_expose_internal_run_session_id() {
        let response = status_control_response("running");
        assert!(!response.contains(INTERNAL_RUN_SESSION_ID));
        assert!(!response.contains("当前会话"));
        assert!(!response.contains("runSessionId"));
    }
}
