//! `client_presence` 与 `notifications` 的 SQLite 实现。
//!
//! 完成通知是 daemon 的 durable 收件箱：先落库再派发，只有渠道确认送达或
//! 主 turn 汇总回复已写入渠道出站队列时才允许确认 Result。

use super::{SqliteRuntimeStore, helpers::i64_value};
use crate::ports::{
    ClientDeliveryStore, ClientDeliveryStoreError, ClientInboxEntry, ClientPresenceRecord,
    CompletionDeliveryRecord, CompletionDeliveryTarget, CompletionNotificationRecord,
    NotificationDeliveryState, RuntimeStore,
};
use rusqlite::{OptionalExtension, params};

#[path = "client_delivery_readers.rs"]
mod readers;
use readers::{read_delivery, read_inbox_entry, read_notification, read_presence, storage_error};

/// 已形成结论的 Execution 状态；与领域 `ExecutionStatus::is_conclusive` 保持一致。
const CONCLUSIVE_EXECUTION_STATUSES: [&str; 4] = ["completed", "failed", "cancelled", "uncertain"];
/// 完成通知读取列：全部读取路径共用，避免列顺序与列集合漂移。
const NOTIFICATION_COLUMNS: &str = "notification_id,workspace_id,goal_id,execution_id,\
     content,summary,result_ids_json,state,created_at_ms,updated_at_ms,expires_at_ms";
/// 渠道投递读取列。
const DELIVERY_COLUMNS: &str =
    "notification_id,channel,destination,state,created_at_ms,updated_at_ms";
/// 收件箱读取列：所有收件箱查询共用同一投影与顺序，避免列漂移。
const INBOX_COLUMNS: &str = "n.notification_id,n.goal_id,n.execution_id,d.channel,d.destination,n.summary,n.created_at_ms,n.expires_at_ms";
impl ClientDeliveryStore for SqliteRuntimeStore {
    fn register_client_presence(
        &self,
        presence: &ClientPresenceRecord,
    ) -> Result<(), ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        connection
            .execute(
                "INSERT INTO client_presence(client_id,workspace_id,channel,destination,registered_at_ms,expires_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(client_id) DO UPDATE SET
                    workspace_id=excluded.workspace_id,
                    channel=excluded.channel,
                    destination=excluded.destination,
                    registered_at_ms=excluded.registered_at_ms,
                    expires_at_ms=excluded.expires_at_ms",
                params![
                    presence.client_id,
                    presence.workspace_id,
                    presence.channel,
                    presence.destination,
                    i64_value(presence.registered_at_ms).map_err(storage_error)?,
                    i64_value(presence.expires_at_ms).map_err(storage_error)?,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn unregister_client_presence(&self, client_id: &str) -> Result<(), ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        connection
            .execute(
                "DELETE FROM client_presence WHERE client_id=?1",
                params![client_id],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn client_presence(
        &self,
        client_id: &str,
    ) -> Result<Option<ClientPresenceRecord>, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        connection
            .query_row(
                "SELECT client_id,workspace_id,channel,destination,registered_at_ms,expires_at_ms
                 FROM client_presence WHERE client_id=?1",
                params![client_id],
                read_presence,
            )
            .optional()
            .map_err(storage_error)
    }

    fn upsert_completion_notification(
        &self,
        notification: &CompletionNotificationRecord,
    ) -> Result<bool, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let result_ids_json =
            serde_json::to_string(&notification.result_ids).map_err(storage_error)?;
        let changed = connection
            .execute(
                "INSERT INTO notifications(notification_id,workspace_id,goal_id,execution_id,content,summary,result_ids_json,state,created_at_ms,updated_at_ms,expires_at_ms)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                 ON CONFLICT(execution_id) DO NOTHING",
                params![
                    notification.notification_id,
                    notification.workspace_id,
                    notification.goal_id,
                    notification.execution_id,
                    notification.content,
                    notification.summary,
                    result_ids_json,
                    notification.state.as_str(),
                    i64_value(notification.created_at_ms).map_err(storage_error)?,
                    i64_value(notification.updated_at_ms).map_err(storage_error)?,
                    i64_value(notification.expires_at_ms).map_err(storage_error)?,
                ],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
    }

    fn completion_notification(
        &self,
        notification_id: &str,
    ) -> Result<Option<CompletionNotificationRecord>, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        connection
            .query_row(
                &format!(
                    "SELECT {NOTIFICATION_COLUMNS}
                     FROM notifications WHERE notification_id=?1"
                ),
                params![notification_id],
                read_notification,
            )
            .optional()
            .map_err(storage_error)
    }

    fn pending_completion_notifications(
        &self,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS}
                 FROM notifications
                 WHERE state='pending'
                 ORDER BY created_at_ms ASC, notification_id ASC LIMIT ?1"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(params![i64::from(limit)], read_notification)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn stale_completion_notifications(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS}
                 FROM notifications INDEXED BY notifications_expiry_all
                 WHERE expires_at_ms <= ?1
                   AND (
                       state IN ('pending','dispatched')
                       OR EXISTS (
                           SELECT 1 FROM notification_deliveries AS d
                           WHERE d.notification_id=notifications.notification_id
                             AND d.state IN ('pending','dispatched')
                       )
                   )
                 ORDER BY expires_at_ms ASC, notification_id ASC LIMIT ?2"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![i64_value(now_ms).map_err(storage_error)?, i64::from(limit)],
                read_notification,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn unnotified_conclusive_executions(
        &self,
        limit: u32,
    ) -> Result<Vec<String>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let statuses = CONCLUSIVE_EXECUTION_STATUSES
            .iter()
            .map(|status| format!("'{status}'"))
            .collect::<Vec<_>>()
            .join(",");
        // 结论状态在控制面库，通知在队列库，跨库无法 JOIN：按 (updated_at_ms, execution_id)
        // 顺序分页取候选，再逐页剔除「已有通知」的项。只取一批会在老项通知齐备后永远返回
        // 空结果，因此这里按页推进到凑满 limit 或候选耗尽。
        let wanted = usize::try_from(limit).unwrap_or(usize::MAX);
        let page_size = i64::from(limit.max(64));
        let mut offset = 0_i64;
        let mut unnotified = Vec::with_capacity(wanted);
        while unnotified.len() < wanted {
            let candidates = {
                let connection = self.connection().map_err(storage_error)?;
                let mut statement = connection
                    .prepare(&format!(
                        // 终态行已搬进历史表：补偿扫描必须走覆盖在途与历史的视图。
                        "SELECT execution_id FROM work_items_all
                         WHERE status IN ({statuses})
                         ORDER BY updated_at_ms ASC, execution_id ASC LIMIT ?1 OFFSET ?2"
                    ))
                    .map_err(storage_error)?;
                statement
                    .query_map(params![page_size, offset], |row| row.get::<_, String>(0))
                    .map_err(storage_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(storage_error)?
            };
            if candidates.is_empty() {
                break;
            }
            let fetched = candidates.len();
            let already_notified = {
                let connection = self.outbox_connection().map_err(storage_error)?;
                let placeholders = vec!["?"; candidates.len()].join(",");
                let mut statement = connection
                    .prepare(&format!(
                        "SELECT execution_id FROM notifications WHERE execution_id IN ({placeholders})"
                    ))
                    .map_err(storage_error)?;
                statement
                    .query_map(rusqlite::params_from_iter(candidates.iter()), |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(storage_error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(storage_error)?
            };
            unnotified.extend(
                candidates
                    .into_iter()
                    .filter(|execution_id| !already_notified.contains(execution_id)),
            );
            offset += i64::try_from(fetched).unwrap_or(i64::MAX);
            if fetched < usize::try_from(page_size).unwrap_or(usize::MAX) {
                break;
            }
        }
        unnotified.truncate(wanted);
        Ok(unnotified)
    }
    fn upsert_completion_deliveries(
        &self,
        notification_id: &str,
        targets: &[CompletionDeliveryTarget],
        now_ms: u64,
    ) -> Result<u32, ClientDeliveryStoreError> {
        if targets.is_empty() {
            return Ok(0);
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let now = i64_value(now_ms).map_err(storage_error)?;
        let mut added = 0_u32;
        for target in targets {
            // 幂等登记：已存在的目标保留既有状态，重复派发不会把已确认渠道退回。
            let changed = connection
                .execute(
                    "INSERT INTO notification_deliveries(notification_id,channel,destination,state,created_at_ms,updated_at_ms)
                     VALUES(?1,?2,?3,'pending',?4,?4)
                     ON CONFLICT(notification_id,channel,destination) DO NOTHING",
                    params![notification_id, target.channel, target.destination, now],
                )
                .map_err(storage_error)?;
            added = added.saturating_add(u32::try_from(changed).unwrap_or(u32::MAX));
        }
        Ok(added)
    }

    fn completion_deliveries(
        &self,
        notification_id: &str,
    ) -> Result<Vec<CompletionDeliveryRecord>, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {DELIVERY_COLUMNS}
                 FROM notification_deliveries
                 WHERE notification_id=?1
                 ORDER BY created_at_ms ASC, channel ASC, destination ASC"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(params![notification_id], read_delivery)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn dispatch_completion_deliveries(
        &self,
        notification_id: &str,
        now_ms: u64,
    ) -> Result<u32, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        // 正文不变量在存储层兜底：主 turn 尚未产出汇总回复（正文为空）时，渠道正文
        // 没有可交付内容，推进只能等汇总落库，绝不允许把空正文当成完成消息投递。
        let changed = connection
            .execute(
                "UPDATE notification_deliveries
                 SET state='dispatched', updated_at_ms=?2
                 WHERE notification_id=?1 AND state='pending'
                   AND EXISTS (
                       SELECT 1 FROM notifications AS n
                       WHERE n.notification_id=?1 AND trim(n.summary) <> ''
                   )",
                params![notification_id, i64_value(now_ms).map_err(storage_error)?],
            )
            .map_err(storage_error)?;
        Ok(u32::try_from(changed).unwrap_or(u32::MAX))
    }

    fn dispatched_completion_inbox(
        &self,
        workspace_id: &str,
        channel: &str,
        destination: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClientInboxEntry>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {INBOX_COLUMNS}
                 FROM notification_deliveries AS d
                 INDEXED BY notification_deliveries_target_inbox
                 JOIN notifications AS n ON n.notification_id=d.notification_id
                 WHERE d.channel=?1 AND d.destination=?2 AND n.workspace_id=?3
                   AND d.state='dispatched' AND n.state<>'expired' AND n.expires_at_ms>?4
                 ORDER BY n.created_at_ms ASC, n.notification_id ASC LIMIT ?5"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    channel,
                    destination,
                    workspace_id,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit)
                ],
                read_inbox_entry,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn dispatched_channel_inbox(
        &self,
        channel: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClientInboxEntry>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {INBOX_COLUMNS}
                 FROM notification_deliveries AS d
                 INDEXED BY notification_deliveries_inbox
                 JOIN notifications AS n ON n.notification_id=d.notification_id
                 WHERE d.channel=?1
                   AND d.state='dispatched' AND n.state<>'expired' AND n.expires_at_ms>?2
                 ORDER BY n.created_at_ms ASC, n.notification_id ASC LIMIT ?3"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    channel,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit)
                ],
                read_inbox_entry,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn settle_completion_delivery(
        &self,
        notification_id: &str,
        channel: &str,
        destination: &str,
        state: NotificationDeliveryState,
        now_ms: u64,
    ) -> Result<bool, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE notification_deliveries
                 SET state=?4, updated_at_ms=?5
                 WHERE notification_id=?1 AND channel=?2 AND destination=?3
                   AND state='dispatched'",
                params![
                    notification_id,
                    channel,
                    destination,
                    state.as_str(),
                    i64_value(now_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
    }

    fn expire_completion_deliveries(
        &self,
        notification_id: &str,
        now_ms: u64,
    ) -> Result<u32, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE notification_deliveries
                 SET state='expired', updated_at_ms=?2
                 WHERE notification_id=?1 AND state IN ('pending','dispatched')",
                params![notification_id, i64_value(now_ms).map_err(storage_error)?],
            )
            .map_err(storage_error)?;
        Ok(u32::try_from(changed).unwrap_or(u32::MAX))
    }

    fn record_completion_summary(
        &self,
        notification_id: &str,
        summary: &str,
        now_ms: u64,
    ) -> Result<(), ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        connection
            .execute(
                "UPDATE notifications
                 SET summary=?2, updated_at_ms=?3
                 WHERE notification_id=?1",
                params![
                    notification_id,
                    summary,
                    i64_value(now_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn settle_completion_notification(
        &self,
        notification_id: &str,
        state: NotificationDeliveryState,
        now_ms: u64,
    ) -> Result<(), ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        connection
            .execute(
                "UPDATE notifications
                 SET state=?2, updated_at_ms=?3
                 WHERE notification_id=?1 AND state<>?2",
                params![
                    notification_id,
                    state.as_str(),
                    i64_value(now_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn notification_ids_with_pending_deliveries(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<String>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.outbox_connection().map_err(storage_error)?;
        // 从投递行的 `pending` 索引区间出发：待推进集合通常为空，避免每轮扫描全部
        // 未结算通知。正文不变量在此再次校验，未产出汇总的通知保持 `pending`。
        let mut statement = connection
            .prepare(
                "SELECT d.notification_id
                 FROM notification_deliveries AS d
                 INDEXED BY notification_deliveries_retention
                 JOIN notifications AS n ON n.notification_id=d.notification_id
                 WHERE d.state='pending' AND n.state<>'expired' AND n.expires_at_ms>?1
                   AND trim(n.summary) <> ''
                 GROUP BY d.notification_id
                 ORDER BY min(n.created_at_ms) ASC, d.notification_id ASC LIMIT ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![i64_value(now_ms).map_err(storage_error)?, i64::from(limit)],
                |row| row.get(0),
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }
}
