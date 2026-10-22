//! Daemon Interaction 提交与命令 IPC。

use std::{path::Path, time::Duration};

use axon_daemon::{infrastructure::ipc::JsonlIpcClient, primary_session_id_for_interaction};
use axon_domain::InteractionRecord;
use axon_protocol::{
    DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest, DaemonResponse,
    SubmitInteractionReceipt, SubmitInteractionRequest,
};
use axon_session::WorkspaceRecord;
use uuid::Uuid;

use super::{register_workspace, register_workspace_record};
use crate::runtime_paths::resolve_daemon_endpoint;

pub(super) const DAEMON_CLIENT_ID: &str = "axon-cli-primary-turn";
const DAEMON_MESSAGE_TYPE: &str = "command";
/// Daemon IPC 请求/响应往返上限。
///
/// `send_command` 是提交 Interaction 与探测 Interaction 状态的唯一入口，无界等待
/// 会让前台（TUI / `axon run`）在 Daemon 卡死时永久停在 Working，且连失败原因都
/// 拿不到。超时按「结果未知」处理：调用方必须按未确认上报，而不是假定未提交。
const DAEMON_IPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) fn submit_daemon_interaction(
    interaction: &InteractionRecord,
    cwd: &Path,
    config_path: Option<&Path>,
) -> Result<(axon_domain::DaemonSessionId, axon_domain::InteractionId), String> {
    register_workspace(cwd, interaction.received_at_ms, config_path)?;
    submit_registered_interaction(interaction, config_path)
}

pub(crate) fn submit_prepared_daemon_interaction(
    interaction: &InteractionRecord,
    workspace: &WorkspaceRecord,
    config_path: Option<&Path>,
) -> Result<(axon_domain::DaemonSessionId, axon_domain::InteractionId), String> {
    let workspace_id = interaction_workspace_id(interaction)?;
    if workspace_id.as_str() != workspace.workspace_id {
        return Err("TUI Interaction 与已解析 Workspace 身份不一致".to_owned());
    }
    register_workspace_record(workspace, config_path)?;
    submit_registered_interaction(interaction, config_path)
}

fn submit_registered_interaction(
    interaction: &InteractionRecord,
    config_path: Option<&Path>,
) -> Result<(axon_domain::DaemonSessionId, axon_domain::InteractionId), String> {
    let endpoint =
        resolve_daemon_endpoint(crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT, config_path)?;
    let response = send_command(
        &endpoint,
        DaemonCommand::SubmitInteraction(SubmitInteractionRequest {
            interaction: interaction.clone(),
        }),
    )?;
    let receipt: SubmitInteractionReceipt = serde_json::from_value(response)
        .map_err(|error| format!("解析 Daemon Interaction 回执失败：{error}"))?;
    primary_session_id_for_interaction(interaction)
        .map(|session_id| (session_id, receipt.interaction_id))
}

fn interaction_workspace_id(
    interaction: &InteractionRecord,
) -> Result<&axon_domain::WorkspaceId, String> {
    interaction
        .scope
        .workspace_id()
        .ok_or_else(|| "Daemon Interaction 必须绑定 Workspace".to_owned())
}

pub(super) fn send_command(
    endpoint: &str,
    command: DaemonCommand,
) -> Result<serde_json::Value, String> {
    let request = daemon_request(command);
    let mut client = JsonlIpcClient::connect(endpoint)
        .map_err(|error| format!("连接 Daemon IPC 失败：{error}"))?;
    // 每次调用都新建连接，超时后连接随调用一起丢弃：迟到的响应不会被误配给下一次请求。
    let response = client
        .send_with_timeout(&request, DAEMON_IPC_REQUEST_TIMEOUT)
        .map_err(|error| format!("Daemon IPC 请求失败：{error}"))?;
    response_body(response)
}

pub(super) fn daemon_request(command: DaemonCommand) -> DaemonRequest {
    DaemonRequest {
        message_type: DAEMON_MESSAGE_TYPE.to_owned(),
        request_id: Uuid::new_v4().to_string(),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: DAEMON_CLIENT_ID.to_owned(),
        session_id: None,
        command,
    }
}

pub(super) fn response_body(response: DaemonResponse) -> Result<serde_json::Value, String> {
    if response.ok {
        return Ok(response.body);
    }
    Err(response
        .body
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Daemon IPC 返回未知错误")
        .to_owned())
}
