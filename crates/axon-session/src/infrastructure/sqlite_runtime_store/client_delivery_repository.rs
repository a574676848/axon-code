//! `jarvis_client_presence` 与 `jarvis_completion_notifications` 的 SQLite 实现。
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
        let connection = self.connection().map_err(storage_error)?;
        connection
            .execute(
                "INSERT INTO jarvis_client_presence(client_id,workspace_id,channel,destination,registered_at_ms,expires_at_ms)
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
        let connection = self.connection().map_err(storage_error)?;
        connection
            .execute(
                "DELETE FROM jarvis_client_presence WHERE client_id=?1",
                params![client_id],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn active_client_presences(
        &self,
        workspace_id: &str,
        now_ms: u64,
    ) -> Result<Vec<ClientPresenceRecord>, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(
                "SELECT client_id,workspace_id,channel,destination,registered_at_ms,expires_at_ms
                 FROM jarvis_client_presence INDEXED BY jarvis_client_presence_workspace
                 WHERE workspace_id=?1 AND expires_at_ms>?2
                 ORDER BY registered_at_ms DESC, client_id ASC",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![workspace_id, i64_value(now_ms).map_err(storage_error)?],
                read_presence,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn active_client_presences_all(
        &self,
        now_ms: u64,
    ) -> Result<Vec<ClientPresenceRecord>, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(
                "SELECT client_id,workspace_id,channel,destination,registered_at_ms,expires_at_ms
                 FROM jarvis_client_presence
                 WHERE expires_at_ms>?1
                 ORDER BY registered_at_ms DESC, client_id ASC",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![i64_value(now_ms).map_err(storage_error)?],
                read_presence,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn client_presence(
        &self,
        client_id: &str,
    ) -> Result<Option<ClientPresenceRecord>, ClientDeliveryStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                "SELECT client_id,workspace_id,channel,destination,registered_at_ms,expires_at_ms
                 FROM jarvis_client_presence WHERE client_id=?1",
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
        let connection = self.connection().map_err(storage_error)?;
        let result_ids_json =
            serde_json::to_string(&notification.result_ids).map_err(storage_error)?;
        let changed = connection
            .execute(
                "INSERT INTO jarvis_completion_notifications(notification_id,workspace_id,goal_id,execution_id,content,summary,result_ids_json,state,created_at_ms,updated_at_ms,expires_at_ms)
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
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                &format!(
                    "SELECT {NOTIFICATION_COLUMNS}
                     FROM jarvis_completion_notifications WHERE notification_id=?1"
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
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS}
                 FROM jarvis_completion_notifications
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

    fn deliverable_completion_notifications(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS}
                 FROM jarvis_completion_notifications
                 WHERE state IN ('pending','dispatched','delivered') AND expires_at_ms>?1
                 ORDER BY created_at_ms ASC, notification_id ASC LIMIT ?2"
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

    fn stale_completion_notifications(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS}
                 FROM jarvis_completion_notifications INDEXED BY jarvis_completion_notifications_expiry_all
                 WHERE expires_at_ms <= ?1
                   AND (
                       state IN ('pending','dispatched')
                       OR EXISTS (
                           SELECT 1 FROM jarvis_completion_deliveries AS d
                           WHERE d.notification_id=jarvis_completion_notifications.notification_id
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
        let connection = self.connection().map_err(storage_error)?;
        let statuses = CONCLUSIVE_EXECUTION_STATUSES
            .iter()
            .map(|status| format!("'{status}'"))
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = connection
            .prepare(&format!(
                "SELECT e.execution_id FROM jarvis_executions AS e
                 LEFT JOIN jarvis_completion_notifications AS n ON n.execution_id=e.execution_id
                 WHERE e.status IN ({statuses}) AND n.notification_id IS NULL
                 ORDER BY e.updated_at_ms ASC, e.execution_id ASC LIMIT ?1"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(params![i64::from(limit)], |row| row.get::<_, String>(0))
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
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
        let connection = self.connection().map_err(storage_error)?;
        let now = i64_value(now_ms).map_err(storage_error)?;
        let mut added = 0_u32;
        for target in targets {
            // 幂等登记：已存在的目标保留既有状态，重复派发不会把已确认渠道退回。
            let changed = connection
                .execute(
                    "INSERT INTO jarvis_completion_deliveries(notification_id,channel,destination,state,created_at_ms,updated_at_ms)
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
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {DELIVERY_COLUMNS}
                 FROM jarvis_completion_deliveries
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
        let connection = self.connection().map_err(storage_error)?;
        // 正文不变量在存储层兜底：主 turn 尚未产出汇总回复（正文为空）时，渠道正文
        // 没有可交付内容，推进只能等汇总落库，绝不允许把空正文当成完成消息投递。
        let changed = connection
            .execute(
                "UPDATE jarvis_completion_deliveries
                 SET state='dispatched', updated_at_ms=?2
                 WHERE notification_id=?1 AND state='pending'
                   AND EXISTS (
                       SELECT 1 FROM jarvis_completion_notifications AS n
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
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {INBOX_COLUMNS}
                 FROM jarvis_completion_deliveries AS d
                 INDEXED BY jarvis_completion_deliveries_target_inbox
                 JOIN jarvis_completion_notifications AS n ON n.notification_id=d.notification_id
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
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {INBOX_COLUMNS}
                 FROM jarvis_completion_deliveries AS d
                 INDEXED BY jarvis_completion_deliveries_inbox
                 JOIN jarvis_completion_notifications AS n ON n.notification_id=d.notification_id
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

    fn dispatched_channel_inbox_for_destination(
        &self,
        channel: &str,
        destination: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<ClientInboxEntry>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {INBOX_COLUMNS}
                 FROM jarvis_completion_deliveries AS d
                 INDEXED BY jarvis_completion_deliveries_target_inbox
                 JOIN jarvis_completion_notifications AS n ON n.notification_id=d.notification_id
                 WHERE d.channel=?1 AND d.destination=?2
                   AND d.state='dispatched' AND n.state<>'expired' AND n.expires_at_ms>?3
                 ORDER BY n.created_at_ms ASC, n.notification_id ASC LIMIT ?4"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    channel,
                    destination,
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
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_completion_deliveries
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
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_completion_deliveries
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
        let connection = self.connection().map_err(storage_error)?;
        connection
            .execute(
                "UPDATE jarvis_completion_notifications
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
        let connection = self.connection().map_err(storage_error)?;
        connection
            .execute(
                "UPDATE jarvis_completion_notifications
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

    fn notifications_awaiting_channel_delivery(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<CompletionNotificationRecord>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {NOTIFICATION_COLUMNS}
                 FROM jarvis_completion_notifications
                 WHERE state='dispatched' AND expires_at_ms>?1
                   AND NOT EXISTS (
                       SELECT 1 FROM jarvis_completion_deliveries AS d
                       WHERE d.notification_id=jarvis_completion_notifications.notification_id
                   )
                 ORDER BY created_at_ms ASC, notification_id ASC LIMIT ?2"
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

    fn notification_ids_with_pending_deliveries(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<String>, ClientDeliveryStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        // 从投递行的 `pending` 索引区间出发：待推进集合通常为空，避免每轮扫描全部
        // 未结算通知。正文不变量在此再次校验，未产出汇总的通知保持 `pending`。
        let mut statement = connection
            .prepare(
                "SELECT d.notification_id
                 FROM jarvis_completion_deliveries AS d
                 INDEXED BY jarvis_completion_deliveries_retention
                 JOIN jarvis_completion_notifications AS n ON n.notification_id=d.notification_id
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
