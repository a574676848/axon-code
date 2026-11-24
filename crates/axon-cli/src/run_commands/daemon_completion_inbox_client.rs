//! TUI 主 turn 客户端接入 Daemon 完成通知收件箱。
//!
//! Daemon 侧持久收件箱按「所有当前可用渠道」投递：在线客户端 presence 与外部渠道
//! 白名单都会登记独立 delivery。TUI 必须先声明 presence，随后持续轮询已派发通知，
//! 并且只在通知真正展示之后回执；不存在可用渠道时通知保留在持久收件箱。
//! 注册失败或进程退出都不会造成静默确认，其他已登记渠道仍可继续投递。

use axon_protocol::DaemonCommand;
use axon_tui_runtime::{
    AgentTuiCompletionInbox, AgentTuiCompletionInboxClient, AgentTuiCompletionNotification,
};
use serde::Deserialize;

use super::daemon_interaction_client::{DAEMON_CLIENT_ID, send_command};
use crate::runtime_paths::resolve_daemon_endpoint;

/// 完成通知收件箱后台线程名。
const COMPLETION_INBOX_THREAD_NAME: &str = "axon-tui-completion-inbox";

/// Daemon 完成通知收件箱应答。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PollNotificationsResponse {
    /// 已派发到当前客户端的通知。
    #[serde(default)]
    notifications: Vec<PolledNotification>,
}

/// 单条已派发通知。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PolledNotification {
    /// 稳定通知 ID。
    notification_id: String,
    /// 通知正文。
    content: String,
}

/// Daemon 收件箱客户端：注册 presence、轮询通知、回执确认。
struct DaemonCompletionInboxClient {
    endpoint: String,
}

impl DaemonCompletionInboxClient {
    /// 注册 TUI presence；注册成功才代表当前客户端在线。
    fn register(endpoint: &str, workspace_id: &str, destination: &str) -> Result<Self, String> {
        send_command(
            endpoint,
            DaemonCommand::RegisterClient {
                client_id: DAEMON_CLIENT_ID.to_owned(),
                workspace_id: workspace_id.to_owned(),
                channel: crate::run_commands::interactive_tui::TUI_CHANNEL.to_owned(),
                destination: destination.to_owned(),
            },
        )?;
        Ok(Self {
            endpoint: endpoint.to_owned(),
        })
    }
}

impl AgentTuiCompletionInboxClient for DaemonCompletionInboxClient {
    fn poll(&mut self, limit: u32) -> Result<Vec<AgentTuiCompletionNotification>, String> {
        let body = send_command(
            &self.endpoint,
            DaemonCommand::PollNotifications {
                client_id: DAEMON_CLIENT_ID.to_owned(),
                limit,
            },
        )?;
        let response: PollNotificationsResponse = serde_json::from_value(body)
            .map_err(|error| format!("解析 Daemon 完成通知应答失败：{error}"))?;
        Ok(response
            .notifications
            .into_iter()
            .map(|notification| AgentTuiCompletionNotification {
                notification_id: notification.notification_id,
                content: notification.content,
            })
            .collect())
    }

    fn acknowledge(&mut self, notification_ids: &[String]) -> Result<(), String> {
        if notification_ids.is_empty() {
            return Ok(());
        }
        send_command(
            &self.endpoint,
            DaemonCommand::AcknowledgeNotifications {
                client_id: DAEMON_CLIENT_ID.to_owned(),
                notification_ids: notification_ids.to_vec(),
            },
        )?;
        Ok(())
    }

    fn unregister(&mut self) {
        // 退出时主动注销 presence；其他可用渠道仍保留自己的 delivery。
        let _ = send_command(
            &self.endpoint,
            DaemonCommand::UnregisterClient {
                client_id: DAEMON_CLIENT_ID.to_owned(),
            },
        );
    }
}

/// 解析 Daemon 端点并启动 TUI 完成通知收件箱。
///
/// 返回 `None` 表示当前无法接入 Daemon 收件箱：完成通知仍由其他可用渠道投递，
/// 没有可用渠道时继续保留在持久收件箱，不会静默丢失。
pub(crate) fn start_completion_inbox(
    config_path: Option<&std::path::Path>,
    cwd: &std::path::Path,
    destination: &str,
) -> Option<AgentTuiCompletionInbox> {
    let endpoint =
        resolve_daemon_endpoint(crate::runtime_paths::DEFAULT_DAEMON_ENDPOINT, config_path).ok()?;
    let workspace = crate::run_commands::run_workspace_identity::workspace_record(
        cwd,
        axon_time::unix_now_ms(),
    )
    .ok()?;
    let client = DaemonCompletionInboxClient::register(
        &endpoint,
        workspace.workspace_id.as_str(),
        destination,
    )
    .ok()?;
    Some(AgentTuiCompletionInbox::spawn(
        COMPLETION_INBOX_THREAD_NAME,
        Box::new(client),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::daemon_interaction_client::{daemon_request, response_body};
    use super::*;

    #[test]
    fn poll_notifications_response_projects_camel_case_payload() {
        let body = serde_json::json!({
            "clientId": "axon-cli-primary-turn",
            "registered": true,
            "notifications": [{
                "notificationId": "completion-notification-e-1",
                "taskId": "t-1",
                "executionId": "e-1",
                "channel": "tui",
                "content": "后台任务【示例】执行完成",
                "createdAtMs": 1,
                "expiresAtMs": 2,
            }],
        });
        let response: PollNotificationsResponse =
            serde_json::from_value(body).expect("应答应可解析");

        assert_eq!(response.notifications.len(), 1);
        assert_eq!(
            response.notifications[0].notification_id,
            "completion-notification-e-1"
        );
        assert_eq!(
            response.notifications[0].content,
            "后台任务【示例】执行完成"
        );
    }

    #[test]
    fn poll_notifications_response_tolerates_empty_body() {
        let response: PollNotificationsResponse =
            serde_json::from_value(serde_json::json!({})).expect("空应答应可解析");

        assert!(response.notifications.is_empty());
    }

    #[test]
    fn request_helper_uses_primary_turn_client_identity() {
        let request = daemon_request(DaemonCommand::UnregisterClient {
            client_id: DAEMON_CLIENT_ID.to_owned(),
        });
        let body = response_body(axon_protocol::DaemonResponse::success(
            request.request_id.clone(),
            serde_json::json!({"registered": false}),
        ))
        .expect("成功应答应可解包");

        assert_eq!(request.client_id, DAEMON_CLIENT_ID);
        assert_eq!(body["registered"], serde_json::json!(false));
    }
}
