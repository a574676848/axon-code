//! `inbox` 的读取侧：入站事实与工作项状态的派生查询。
//!
//! 写入与状态机在 `interaction_repository`；这里只负责把两者 JOIN 起来读。

use super::SqliteRuntimeStore;
use super::interaction_repository_row::{parse_status, read_record, runtime_error, storage_error};
use crate::ports::{
    InteractionCursor, InteractionQueryStore, InteractionStoreError, InteractionTurnHead,
    RuntimeStore,
};
use axon_domain::{InteractionId, InteractionRecord};
use rusqlite::{OptionalExtension, params};

/// 派生的交互状态表达式：读取侧只有这一处口径。
///
/// 终态列优先：派发轮没有 work_item，终态直接写在入站行上；正常入站轮仍按
/// 「在跑优先、其次历史归档、再次工作项终态、最后已收到」的顺序派生。
pub(super) const INBOX_STATE_EXPR: &str = "CASE WHEN i.terminal_kind IS NOT NULL THEN i.terminal_kind \
     WHEN w.status='running' THEN 'processing' \
     WHEN h.terminal_kind IS NOT NULL THEN h.terminal_kind \
     WHEN w.terminal_kind IS NOT NULL THEN w.terminal_kind \
     ELSE 'received' END";

/// 完整记录列：状态列复用同一份表达式，避免两处口径漂移。
pub(super) fn inbox_columns() -> String {
    format!(
        "i.interaction_id,i.source_channel,i.source_message_id,i.scope_json,\
i.audience_json,i.reply_route_json,i.content,{INBOX_STATE_EXPR},\
i.received_at_ms,i.attachments_json,i.model,i.reasoning_effort,i.client,i.session_id"
    )
}
pub(super) const INBOX_STATE_JOIN: &str = "FROM inbox AS i \
     LEFT JOIN work_items AS w ON w.inbox_id = i.interaction_id \
     LEFT JOIN work_item_history AS h ON h.inbox_id = i.interaction_id \
         AND h.terminal_kind IS NOT NULL \
         AND h.rowid = (SELECT MIN(h2.rowid) FROM work_item_history AS h2 \
                        WHERE h2.inbox_id = i.interaction_id AND h2.terminal_kind IS NOT NULL)";
/// 工作项的在途状态。
pub(super) const WORK_RUNNING: &str = "running";
pub(super) const SYSTEM_SOURCE_CHANNEL: &str = axon_domain::SYSTEM_SOURCE_CHANNEL;
/// 执行侧派发记录的渠道：不属于用户真实渠道使用记录，最近渠道扫描必须与系统渠道一并排除。
pub(super) const AGENT_DISPATCH_SOURCE_CHANNEL: &str = axon_domain::AGENT_DISPATCH_SOURCE_CHANNEL;

pub(super) fn select_inbox(where_clause: &str) -> String {
    format!(
        "SELECT {} {INBOX_STATE_JOIN} {where_clause}",
        inbox_columns()
    )
}

impl InteractionQueryStore for SqliteRuntimeStore {
    fn interaction_turn_heads(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<InteractionTurnHead>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let sql = format!(
            "SELECT i.interaction_id,{INBOX_STATE_EXPR},i.received_at_ms {INBOX_STATE_JOIN} \
             WHERE i.session_id=?1 ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT ?2"
        );
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        let rows = statement
            .query_map(params![session_id, i64::from(limit)], |row| {
                let interaction_id: String = row.get(0)?;
                let raw_status: String = row.get(1)?;
                let received_at_ms: i64 = row.get(2)?;
                Ok(InteractionTurnHead {
                    interaction_id,
                    status: parse_status(&raw_status)?,
                    received_at_ms: u64::try_from(received_at_ms).unwrap_or(0),
                })
            })
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }
    fn get_interaction(
        &self,
        interaction_id: &InteractionId,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                &select_inbox("WHERE i.interaction_id=?1"),
                params![interaction_id.as_str()],
                read_record,
            )
            .optional()
            .map_err(storage_error)
    }

    fn latest_external_interaction(
        &self,
        workspace_id: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let scope_json = workspace_scope_json(workspace_id)?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                &select_inbox(
                    "WHERE i.scope_json=?1 AND i.source_channel<>?2 AND i.source_channel<>?4 \
                       AND i.source_message_id NOT LIKE ?3 \
                     ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1",
                ),
                params![
                    scope_json,
                    SYSTEM_SOURCE_CHANNEL,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX),
                    AGENT_DISPATCH_SOURCE_CHANNEL
                ],
                read_record,
            )
            .optional()
            .map_err(storage_error)
    }

    fn latest_processing_external_interaction(
        &self,
        workspace_id: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let scope_json = workspace_scope_json(workspace_id)?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                &select_inbox(
                    "WHERE i.scope_json=?1 AND i.source_channel<>?2 AND i.source_channel<>?5 \
                       AND i.source_message_id NOT LIKE ?3 \
                       AND w.status=?4 \
                     ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1",
                ),
                params![
                    scope_json,
                    SYSTEM_SOURCE_CHANNEL,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX),
                    WORK_RUNNING,
                    AGENT_DISPATCH_SOURCE_CHANNEL,
                ],
                read_record,
            )
            .optional()
            .map_err(storage_error)
    }

    fn latest_processing_interaction_for_route(
        &self,
        workspace_id: &str,
        channel: &str,
        destination: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let scope_json = workspace_scope_json(workspace_id)?;
        let route = axon_domain::ReplyRoute::new(channel, destination)
            .map_err(|error| InteractionStoreError::InvalidState(error.to_string()))?;
        let route_json = serde_json::to_string(&route)
            .map_err(|error| InteractionStoreError::Storage(error.to_string()))?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                &select_inbox(
                    "WHERE i.scope_json=?1 AND i.source_channel=?2 AND i.reply_route_json=?3 \
                       AND i.source_message_id NOT LIKE ?4 AND w.status=?5 \
                     ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1",
                ),
                params![
                    scope_json,
                    channel,
                    route_json,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX),
                    WORK_RUNNING,
                ],
                read_record,
            )
            .optional()
            .map_err(storage_error)
    }

    fn latest_external_interaction_for_channel(
        &self,
        workspace_id: &str,
        channel: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        latest_interaction_for_channel(self, workspace_id, channel, false)
    }

    fn latest_processing_interaction_for_channel(
        &self,
        workspace_id: &str,
        channel: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        latest_interaction_for_channel(self, workspace_id, channel, true)
    }

    fn latest_external_interaction_any(
        &self,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                &select_inbox(
                    "WHERE i.source_channel<>?1 AND i.source_channel<>?3 \
                       AND i.source_message_id NOT LIKE ?2 \
                     ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1",
                ),
                params![
                    SYSTEM_SOURCE_CHANNEL,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX),
                    AGENT_DISPATCH_SOURCE_CHANNEL
                ],
                read_record,
            )
            .optional()
            .map_err(storage_error)
    }

    fn interactions_for_session(
        &self,
        session_id: &str,
        limit: u32,
        before: Option<&InteractionCursor>,
    ) -> Result<Vec<InteractionRecord>, InteractionStoreError> {
        if session_id.trim().is_empty() {
            return Err(InteractionStoreError::InvalidState(
                "车道标识不能为空".to_owned(),
            ));
        }
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        // 游标为空取最近一页，给出游标取它之前一页；行值比较与排序同键，故不漏不重。
        let mut statement = connection
            .prepare(&select_inbox(
                "WHERE i.session_id=?1 AND (?2 IS NULL OR (i.received_at_ms, i.interaction_id) < (?2, ?3)) \
                 ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT ?4",
            ))
            .map_err(storage_error)?;
        let mut records = statement
            .query_map(
                params![
                    session_id,
                    // SQLite 整数是 i64；毫秒时间戳远在其范围内，越界按最大值处理。
                    before.map(|cursor| i64::try_from(cursor.received_at_ms).unwrap_or(i64::MAX)),
                    before.map(|cursor| cursor.interaction_id.as_str()),
                    limit
                ],
                read_record,
            )
            .map_err(storage_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)?;
        records.reverse();
        Ok(records)
    }
}

fn workspace_scope_json(workspace_id: &str) -> Result<String, InteractionStoreError> {
    let scope = axon_domain::ScopeRef::workspace(workspace_id.to_owned())
        .map_err(|_| InteractionStoreError::InvalidState("Workspace ID 无效".to_owned()))?;
    serde_json::to_string(&scope).map_err(|error| InteractionStoreError::Storage(error.to_string()))
}

fn latest_interaction_for_channel(
    store: &SqliteRuntimeStore,
    workspace_id: &str,
    channel: &str,
    processing_only: bool,
) -> Result<Option<InteractionRecord>, InteractionStoreError> {
    store.initialize().map_err(runtime_error)?;
    let scope_json = workspace_scope_json(workspace_id)?;
    let state_clause = if processing_only {
        format!(" AND w.status='{WORK_RUNNING}'")
    } else {
        String::new()
    };
    let sql = select_inbox(&format!(
        "WHERE i.scope_json=?1 AND i.source_channel=?2 AND i.source_message_id NOT LIKE ?3{state_clause} \
         ORDER BY i.received_at_ms DESC, i.interaction_id DESC LIMIT 1"
    ));
    store
        .connection()
        .map_err(runtime_error)?
        .query_row(
            &sql,
            rusqlite::params![
                scope_json,
                channel,
                format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX)
            ],
            read_record,
        )
        .optional()
        .map_err(storage_error)
}
