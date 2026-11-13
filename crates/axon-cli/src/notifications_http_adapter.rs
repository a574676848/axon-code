//! 后台完成通知收件箱：桌面端经网关拉取后台任务与定时任务的完成结论。
//!
//! 完成通知是 Daemon 的 durable 收件箱：先落库，再由渠道自己消费。TUI 走 IPC
//! 拉取，桌面端经本族 HTTP 拉取，两条入口共享同一份存储与同一套状态机。这里
//! 不复制任何状态，只做「读已派发条目」与「结算送达」两件事。

use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_session::{ClientDeliveryStore, NotificationDeliveryState, SqliteRuntimeStore};
use serde::Deserialize;
use std::path::PathBuf;

/// 路由前缀。
const NOTIFICATIONS_PATH: &str = "/api/v1/notifications";
/// 单次拉取的默认与上限条数：收件箱按有界批次消费，客户端需要更多就再拉一次。
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;
/// 桌面端渠道标识；其它渠道（TUI、飞书）各自走自己的入口。
const DESKTOP_CHANNEL: &str = "desktop";

/// 通知收件箱路由族。
pub struct NotificationsHttpAdapter {
    runtime_root: PathBuf,
}

impl NotificationsHttpAdapter {
    /// 以运行时根目录构造：数据库固定落在它下面。
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    fn store(&self) -> Result<SqliteRuntimeStore, NotificationsHttpError> {
        Ok(SqliteRuntimeStore::from_root(&self.runtime_root))
    }

    fn response(result: Result<serde_json::Value, NotificationsHttpError>) -> GatewayHttpResponse {
        match result {
            Ok(payload) => GatewayHttpResponse::json(200, &payload),
            Err(error) => GatewayHttpResponse::json(
                error.status,
                &serde_json::json!({
                    "error": { "code": error.code, "message": error.message }
                }),
            ),
        }
    }

    /// 拉取已派发但尚未结算的通知。
    ///
    /// 只返回 `dispatched` 且未过期的条目：`pending` 代表主 turn 汇总还没产出，
    /// 此时展示等于把半成品当成结论。
    fn poll(&self, query: Option<&str>) -> Result<serde_json::Value, NotificationsHttpError> {
        let channel = query_value(query, "channel")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DESKTOP_CHANNEL.to_owned());
        let limit = query_value(query, "limit")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);
        let now_ms = axon_time::unix_now_ms();
        let entries = self
            .store()?
            .dispatched_channel_inbox(&channel, now_ms, limit)
            .map_err(NotificationsHttpError::internal)?;
        let notifications = entries
            .into_iter()
            .map(|entry| {
                serde_json::json!({
                    "notificationId": entry.notification_id,
                    "goalId": entry.goal_id,
                    "executionId": entry.execution_id,
                    "channel": entry.channel,
                    "destination": entry.destination,
                    "content": entry.content,
                    "createdAtMs": entry.created_at_ms,
                    "expiresAtMs": entry.expires_at_ms,
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "channel": channel, "notifications": notifications }))
    }

    /// 结算送达：正文已经交给系统通知，才允许把投递项标记为 delivered。
    fn acknowledge(&self, body: &[u8]) -> Result<serde_json::Value, NotificationsHttpError> {
        let request: AcknowledgeRequest = serde_json::from_slice(body).map_err(|error| {
            NotificationsHttpError::bad_request(format!("解析通知回执失败：{error}"))
        })?;
        let channel = request.channel.trim();
        if channel.is_empty() {
            return Err(NotificationsHttpError::bad_request("通知回执缺少渠道"));
        }
        let store = self.store()?;
        let now_ms = axon_time::unix_now_ms();
        let mut acknowledged = 0_u32;
        for entry in &request.entries {
            let changed = store
                .settle_completion_delivery(
                    &entry.notification_id,
                    channel,
                    &entry.destination,
                    NotificationDeliveryState::Delivered,
                    now_ms,
                )
                .map_err(NotificationsHttpError::internal)?;
            if !changed {
                continue;
            }
            acknowledged = acknowledged.saturating_add(1);
            store
                .settle_completion_notification(
                    &entry.notification_id,
                    NotificationDeliveryState::Delivered,
                    now_ms,
                )
                .map_err(NotificationsHttpError::internal)?;
        }
        Ok(serde_json::json!({ "acknowledged": acknowledged }))
    }
}

impl GatewayHttpRouteHandler for NotificationsHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        let suffix = path.strip_prefix(NOTIFICATIONS_PATH)?;
        let parts = suffix
            .trim_matches('/')
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let result = match (method, parts.as_slice()) {
            ("GET", []) => self.poll(query),
            ("POST", ["ack"]) => self.acknowledge(body),
            _ => return None,
        };
        Some(Self::response(result))
    }
}

/// 回执里的一条投递项：渠道内目标由拉取时下发的同一份事实给出。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcknowledgeEntry {
    notification_id: String,
    destination: String,
}

/// 送达回执请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcknowledgeRequest {
    channel: String,
    #[serde(default)]
    entries: Vec<AcknowledgeEntry>,
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query?.as_bytes())
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
}

/// 本族错误：与网关其余路由族一样给出统一错误模型。
#[derive(Debug)]
struct NotificationsHttpError {
    status: u16,
    code: &'static str,
    message: String,
}

impl NotificationsHttpError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "BAD_REQUEST",
            message: message.into(),
        }
    }

    /// 内部错误：接受任何可展示的错误，存储层与解析层的错误类型都能直接接进来。
    fn internal(message: impl std::fmt::Display) -> Self {
        Self {
            status: 500,
            code: "INTERNAL",
            message: message.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_session::{CompletionDeliveryTarget, CompletionNotificationRecord, RuntimeStore};

    fn seeded(root: &std::path::Path) -> CompletionNotificationRecord {
        let store = SqliteRuntimeStore::from_root(root);
        store.initialize().expect("初始化运行态");
        let now_ms = axon_time::unix_now_ms();
        let notification = CompletionNotificationRecord {
            notification_id: "completion-notification-test".to_owned(),
            workspace_id: "workspace-a".to_owned(),
            goal_id: "goal-a".to_owned(),
            execution_id: "execution-a".to_owned(),
            content: "后台任务已完成".to_owned(),
            summary: "后台任务已完成".to_owned(),
            result_ids: Vec::new(),
            state: NotificationDeliveryState::Dispatched,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(60_000),
        };
        store
            .upsert_completion_notification(&notification)
            .expect("写通知");
        let target = CompletionDeliveryTarget {
            channel: DESKTOP_CHANNEL.to_owned(),
            destination: "workbench-1".to_owned(),
        };
        store
            .upsert_completion_deliveries(
                &notification.notification_id,
                std::slice::from_ref(&target),
                now_ms,
            )
            .expect("写投递");
        store
            .dispatch_completion_deliveries(&notification.notification_id, now_ms)
            .expect("推进到已派发");
        notification
    }

    #[test]
    fn poll_lists_dispatched_then_ack_settles_them() {
        let directory = tempfile::tempdir().expect("临时目录");
        let root = directory.path();
        let notification = seeded(root);
        let adapter = NotificationsHttpAdapter::new(root);

        let listed = adapter.poll(Some("channel=desktop")).expect("拉取通知");
        let entries = listed["notifications"]
            .as_array()
            .expect("通知数组")
            .clone();
        assert_eq!(entries.len(), 1, "已派发条目必须可拉取：{listed}");
        assert_eq!(entries[0]["destination"], "workbench-1");
        assert_eq!(entries[0]["content"], "后台任务已完成");

        let receipt = serde_json::json!({
            "channel": DESKTOP_CHANNEL,
            "entries": [{
                "notificationId": notification.notification_id,
                "destination": "workbench-1",
            }],
        });
        let acknowledged = adapter
            .acknowledge(receipt.to_string().as_bytes())
            .expect("送达回执");
        assert_eq!(acknowledged["acknowledged"], 1, "回执必须真的推进状态");

        let remaining = adapter.poll(Some("channel=desktop")).expect("再次拉取");
        assert!(
            remaining["notifications"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "回执后不得再次下发：{remaining}"
        );
    }

    #[test]
    fn unknown_subpath_is_not_handled() {
        let adapter = NotificationsHttpAdapter::new(".");
        assert!(
            adapter
                .handle_request("GET", "/api/v1/notifications/elsewhere", None, &[])
                .is_none()
        );
        assert!(
            adapter
                .handle_request("GET", "/api/v1/crons", None, &[])
                .is_none()
        );
    }
}
