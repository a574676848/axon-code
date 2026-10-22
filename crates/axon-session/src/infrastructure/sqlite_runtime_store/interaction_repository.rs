//! `jarvis_interactions` 的 SQLite 持久化实现。

use super::{
    SqliteRuntimeStore,
    helpers::{i64_value, u64_value},
};
use crate::application::interaction_session_lane_id;
use crate::ports::{
    InteractionAppendReceipt, InteractionCursor, InteractionProcessingLease, InteractionQueryStore,
    InteractionStore, InteractionStoreError, RuntimeStore,
};
use axon_domain::{InteractionId, InteractionRecord, InteractionSource, InteractionStatus};
use rusqlite::{OptionalExtension, Row, Transaction, TransactionBehavior, params};
use uuid::Uuid;

const STATUS_RECEIVED: &str = "received";
const STATUS_PROCESSING: &str = "processing";
const STATUS_UNDERSTOOD: &str = "understood";
const STATUS_REJECTED: &str = "rejected";
const STATUS_INTERRUPTED: &str = "interrupted";
const SELECT_INTERACTION_BY_SOURCE: &str = "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json FROM jarvis_interactions WHERE source_channel=?1 AND source_message_id=?2";
const SELECT_INTERACTION_ID_EXISTS: &str =
    "SELECT 1 FROM jarvis_interactions WHERE interaction_id=?1";
const INSERT_INTERACTION: &str = "INSERT INTO jarvis_interactions(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,updated_at_ms,attachments_json,lane_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9,?10,?11)";
/// lane 排他领取候选：排队记录中，所属 lane 当前没有在途 `processing` 的那些。
const SELECT_CLAIMABLE_INTERACTIONS: &str = "SELECT queued.interaction_id,queued.received_at_ms \
     FROM jarvis_interactions queued \
     WHERE queued.status=?1 AND queued.lease_expires_at_ms IS NULL \
       AND NOT EXISTS(SELECT 1 FROM jarvis_interactions active \
                      WHERE active.lane_id=queued.lane_id AND active.status=?2 \
                        AND active.lease_expires_at_ms>?3) \
     ORDER BY queued.received_at_ms ASC, queued.interaction_id ASC LIMIT ?4";
/// 系统自身派发的来源渠道；与领域常量保持单一事实源。
const SYSTEM_SOURCE_CHANNEL: &str = axon_domain::SYSTEM_SOURCE_CHANNEL;

impl InteractionQueryStore for SqliteRuntimeStore {
    fn get_interaction(
        &self,
        interaction_id: &InteractionId,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json FROM jarvis_interactions WHERE interaction_id=?1",
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
        let scope = axon_domain::ScopeRef::workspace(workspace_id.to_owned())
            .map_err(|_| InteractionStoreError::InvalidState("Workspace ID 无效".to_owned()))?;
        let scope_json = serde_json::to_string(&scope)
            .map_err(|error| InteractionStoreError::Storage(error.to_string()))?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json
                 FROM jarvis_interactions
                 WHERE scope_json=?1 AND source_channel<>?2 AND source_message_id NOT LIKE ?3
                 ORDER BY received_at_ms DESC, interaction_id DESC LIMIT 1",
                params![
                    scope_json,
                    SYSTEM_SOURCE_CHANNEL,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX)
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
        let scope = axon_domain::ScopeRef::workspace(workspace_id.to_owned())
            .map_err(|_| InteractionStoreError::InvalidState("Workspace ID 无效".to_owned()))?;
        let scope_json = serde_json::to_string(&scope)
            .map_err(|error| InteractionStoreError::Storage(error.to_string()))?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json
                 FROM jarvis_interactions
                 WHERE scope_json=?1 AND source_channel<>?2 AND source_message_id NOT LIKE ?3 AND status=?4
                 ORDER BY received_at_ms DESC, interaction_id DESC LIMIT 1",
                params![
                    scope_json,
                    SYSTEM_SOURCE_CHANNEL,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX),
                    STATUS_PROCESSING,
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
        let scope = axon_domain::ScopeRef::workspace(workspace_id.to_owned())
            .map_err(|_| InteractionStoreError::InvalidState("Workspace ID 无效".to_owned()))?;
        let route = axon_domain::ReplyRoute::new(channel, destination)
            .map_err(|error| InteractionStoreError::InvalidState(error.to_string()))?;
        let scope_json = serde_json::to_string(&scope)
            .map_err(|error| InteractionStoreError::Storage(error.to_string()))?;
        let route_json = serde_json::to_string(&route)
            .map_err(|error| InteractionStoreError::Storage(error.to_string()))?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json
                 FROM jarvis_interactions
                 WHERE scope_json=?1 AND source_channel=?2 AND reply_route_json=?3 AND source_message_id NOT LIKE ?4 AND status=?5
                 ORDER BY received_at_ms DESC, interaction_id DESC LIMIT 1",
                params![
                    scope_json,
                    channel,
                    route_json,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX),
                    STATUS_PROCESSING,
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
        latest_interaction_for_channel(self, workspace_id, channel, None)
    }

    fn latest_processing_interaction_for_channel(
        &self,
        workspace_id: &str,
        channel: &str,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        latest_interaction_for_channel(self, workspace_id, channel, Some(STATUS_PROCESSING))
    }

    fn latest_external_interaction_any(
        &self,
    ) -> Result<Option<InteractionRecord>, InteractionStoreError> {
        self.initialize().map_err(runtime_error)?;
        self.connection()
            .map_err(runtime_error)?
            .query_row(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json
                 FROM jarvis_interactions
                 WHERE source_channel<>?1 AND source_message_id NOT LIKE ?2
                 ORDER BY received_at_ms DESC, interaction_id DESC LIMIT 1",
                params![
                    SYSTEM_SOURCE_CHANNEL,
                    format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX)
                ],
                read_record,
            )
            .optional()
            .map_err(storage_error)
    }

    fn interactions_for_lane(
        &self,
        lane_id: &str,
        limit: u32,
        before: Option<&InteractionCursor>,
    ) -> Result<Vec<InteractionRecord>, InteractionStoreError> {
        if lane_id.trim().is_empty() {
            return Err(InteractionStoreError::InvalidState(
                "车道标识不能为空".to_owned(),
            ));
        }
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        // 游标为空取最近一页，给出游标取它之前一页；行值比较与排序同键，故不漏不重。
        let mut statement = connection
            .prepare(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json
                 FROM jarvis_interactions
                 WHERE lane_id=?1 AND (?2 IS NULL OR (received_at_ms, interaction_id) < (?2, ?3))
                 ORDER BY received_at_ms DESC, interaction_id DESC LIMIT ?4",
            )
            .map_err(storage_error)?;
        let mut records = statement
            .query_map(
                params![
                    lane_id,
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

fn latest_interaction_for_channel(
    store: &SqliteRuntimeStore,
    workspace_id: &str,
    channel: &str,
    status: Option<&str>,
) -> Result<Option<InteractionRecord>, InteractionStoreError> {
    store.initialize().map_err(runtime_error)?;
    let scope = axon_domain::ScopeRef::workspace(workspace_id.to_owned())
        .map_err(|_| InteractionStoreError::InvalidState("Workspace ID 无效".to_owned()))?;
    let scope_json = serde_json::to_string(&scope)
        .map_err(|error| InteractionStoreError::Storage(error.to_string()))?;
    let status_clause = status.map_or(String::new(), |_| " AND status=?4".to_owned());
    let sql = format!(
        "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json
         FROM jarvis_interactions
         WHERE scope_json=?1 AND source_channel=?2 AND source_message_id NOT LIKE ?3{status_clause}
         ORDER BY received_at_ms DESC, interaction_id DESC LIMIT 1"
    );
    let connection = store.connection().map_err(runtime_error)?;
    let steer_prefix = format!("{}%", axon_domain::COMPLETION_STEER_SOURCE_PREFIX);
    let result = match status {
        Some(status) => connection
            .query_row(
                &sql,
                rusqlite::params![scope_json, channel, steer_prefix, status],
                read_record,
            )
            .optional()
            .map_err(storage_error)?,
        None => connection
            .query_row(
                &sql,
                rusqlite::params![scope_json, channel, steer_prefix],
                read_record,
            )
            .optional()
            .map_err(storage_error)?,
    };
    Ok(result)
}

impl InteractionStore for SqliteRuntimeStore {
    fn append(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionAppendReceipt, InteractionStoreError> {
        ensure_received(interaction)?;
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        if let Some(record) = find_by_source(&transaction, interaction)? {
            ensure_same_interaction(&record, interaction)?;
            return Ok(InteractionAppendReceipt {
                interaction_id: record.interaction_id,
                inserted: false,
            });
        }
        ensure_id_available(&transaction, &interaction.interaction_id)?;
        insert_interaction(&transaction, interaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(InteractionAppendReceipt {
            interaction_id: interaction.interaction_id.clone(),
            inserted: true,
        })
    }

    fn claim_processing(
        &self,
        limit: u32,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Vec<InteractionProcessingLease>, InteractionStoreError> {
        if limit == 0 || lease_duration_ms == 0 {
            return Err(InteractionStoreError::InvalidState(
                "claim 参数必须为正数".to_owned(),
            ));
        }
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let now = i64_value(now_ms).map_err(runtime_error)?;
        let expires = i64_value(now_ms.saturating_add(lease_duration_ms)).map_err(runtime_error)?;
        let ids = claimable_ids(&transaction, now_ms, limit)?;
        let mut leases = Vec::with_capacity(ids.len());
        for id in ids {
            let token = Uuid::new_v4().to_string();
            let changed = transaction
                .execute(
                    "UPDATE jarvis_interactions SET status=?1,lease_token=?2,lease_expires_at_ms=?3,updated_at_ms=?4 WHERE interaction_id=?5 AND (status=?6 OR (status=?7 AND lease_expires_at_ms<=?4))",
                    params![STATUS_PROCESSING, token, expires, now, id, STATUS_RECEIVED, STATUS_PROCESSING],
                )
                .map_err(storage_error)?;
            if changed == 0 {
                continue;
            }
            let interaction = transaction
                .query_row(
                    "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json FROM jarvis_interactions WHERE interaction_id=?1",
                    params![id],
                    read_record,
                )
                .map_err(storage_error)?;
            leases.push(InteractionProcessingLease {
                interaction,
                lease_token: token,
                lease_expires_at_ms: u64::try_from(expires)
                    .map_err(|_| InteractionStoreError::Storage("lease 到期时间无效".to_owned()))?,
            });
        }
        transaction.commit().map_err(storage_error)?;
        Ok(leases)
    }

    fn mark_understood(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        update_status(self, interaction_id, lease_token, now_ms, STATUS_UNDERSTOOD)
    }

    fn claim_processing_by_id(
        &self,
        interaction_id: &InteractionId,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<InteractionProcessingLease>, InteractionStoreError> {
        ensure_positive_lease_duration(lease_duration_ms)?;
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let now = i64_value(now_ms).map_err(runtime_error)?;
        let expires = i64_value(now_ms.saturating_add(lease_duration_ms)).map_err(runtime_error)?;
        let token = Uuid::new_v4().to_string();
        let changed = transaction
            .execute(
                "UPDATE jarvis_interactions SET status=?1,lease_token=?2,lease_expires_at_ms=?3,updated_at_ms=?4 WHERE interaction_id=?5 AND (status=?6 OR (status=?1 AND lease_expires_at_ms<=?4))",
                params![
                    STATUS_PROCESSING,
                    token,
                    expires,
                    now,
                    interaction_id.as_str(),
                    STATUS_RECEIVED
                ],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        }
        let interaction = transaction
            .query_row(
                "SELECT interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,status,received_at_ms,attachments_json FROM jarvis_interactions WHERE interaction_id=?1",
                params![interaction_id.as_str()],
                read_record,
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(Some(InteractionProcessingLease {
            interaction,
            lease_token: token,
            lease_expires_at_ms: u64::try_from(expires)
                .map_err(|_| InteractionStoreError::Storage("lease 到期时间无效".to_owned()))?,
        }))
    }

    fn renew_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<u64, InteractionStoreError> {
        if lease_token.is_empty() || lease_duration_ms == 0 {
            return Err(InteractionStoreError::InvalidState(
                "续租参数无效".to_owned(),
            ));
        }
        let expires_at_ms = now_ms.saturating_add(lease_duration_ms);
        let connection = self.connection().map_err(runtime_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_interactions SET lease_expires_at_ms=?1,updated_at_ms=?2 WHERE interaction_id=?3 AND status=?4 AND lease_token=?5",
                params![
                    i64_value(expires_at_ms).map_err(runtime_error)?,
                    i64_value(now_ms).map_err(runtime_error)?,
                    interaction_id.as_str(),
                    STATUS_PROCESSING,
                    lease_token
                ],
            )
            .map_err(storage_error)?;
        if changed == 1 {
            Ok(expires_at_ms)
        } else {
            Err(InteractionStoreError::InvalidState(
                "Interaction processing lease 已失效".to_owned(),
            ))
        }
    }

    fn release_processing(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        update_status(self, interaction_id, lease_token, now_ms, STATUS_RECEIVED)
    }

    fn mark_rejected(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        update_status(self, interaction_id, lease_token, now_ms, STATUS_REJECTED)
    }

    fn mark_interrupted(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        update_status(
            self,
            interaction_id,
            lease_token,
            now_ms,
            STATUS_INTERRUPTED,
        )
    }
}

fn ensure_positive_lease_duration(lease_duration_ms: u64) -> Result<(), InteractionStoreError> {
    if lease_duration_ms == 0 {
        return Err(InteractionStoreError::InvalidState(
            "lease duration 必须为正数".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_received(interaction: &InteractionRecord) -> Result<(), InteractionStoreError> {
    if interaction.status != InteractionStatus::Received {
        return Err(InteractionStoreError::InvalidState(
            "Interaction 入站状态必须为 received".to_owned(),
        ));
    }
    Ok(())
}

fn find_by_source(
    transaction: &Transaction<'_>,
    interaction: &InteractionRecord,
) -> Result<Option<InteractionRecord>, InteractionStoreError> {
    transaction
        .query_row(
            SELECT_INTERACTION_BY_SOURCE,
            params![
                interaction.source.channel(),
                interaction.source.source_message_id()
            ],
            read_record,
        )
        .optional()
        .map_err(storage_error)
}

fn ensure_id_available(
    transaction: &Transaction<'_>,
    interaction_id: &InteractionId,
) -> Result<(), InteractionStoreError> {
    let duplicate = transaction
        .query_row(
            SELECT_INTERACTION_ID_EXISTS,
            params![interaction_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_error)?;
    if duplicate.is_some() {
        return Err(InteractionStoreError::Conflict(
            "interaction_id 已绑定其他来源".to_owned(),
        ));
    }
    Ok(())
}

fn insert_interaction(
    transaction: &Transaction<'_>,
    interaction: &InteractionRecord,
) -> Result<(), InteractionStoreError> {
    transaction
        .execute(
            INSERT_INTERACTION,
            params![
                interaction.interaction_id.as_str(),
                interaction.source.channel(),
                interaction.source.source_message_id(),
                serde_json::to_string(&interaction.scope).map_err(serialize_error)?,
                serde_json::to_string(&interaction.audience).map_err(serialize_error)?,
                serde_json::to_string(&interaction.reply_route).map_err(serialize_error)?,
                interaction.content,
                status_code(interaction.status),
                i64_value(interaction.received_at_ms).map_err(runtime_error)?,
                serde_json::to_string(&interaction.attachments).map_err(serialize_error)?,
                // lane 是主 turn 排他领取的事实源；未绑定 Workspace 的输入统一落在空 lane。
                interaction_session_lane_id(interaction).unwrap_or_default(),
            ],
        )
        .map(|_| ())
        .map_err(storage_error)
}

/// 领取候选集合：只包含未持有 lease、且所属 lane 当前没有在途执行的排队记录。
///
/// lane 内串行是主 turn 的硬约束：同一 lane 已有未过期 `processing` 时，该 lane 的
/// 后续记录必须留在队列里，否则协调线程会把同 lane 的两条输入并发交给 Provider。
/// 一次领取内的多条同 lane 记录仍然合法：它们会作为同一条 lane 投递、顺序执行。
/// 过期 `processing` 不再盲目重领；由中断恢复路径先证明原 worker 已不可达，
/// 再收敛为 interrupted 终态。排队侧走 claim 索引等值前缀，lane 侧走 lane 索引。
fn claimable_ids(
    transaction: &Transaction<'_>,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<String>, InteractionStoreError> {
    let candidates = ordered_candidates(
        transaction,
        SELECT_CLAIMABLE_INTERACTIONS,
        params![
            STATUS_RECEIVED,
            STATUS_PROCESSING,
            i64_value(now_ms).map_err(runtime_error)?,
            i64::from(limit)
        ],
    )?;
    Ok(candidates.into_iter().map(|(_, id)| id).collect())
}

/// 按到达顺序读取单个分支的前 `limit` 条候选，返回 `(received_at_ms, interaction_id)`。
fn ordered_candidates(
    transaction: &Transaction<'_>,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<Vec<(i64, String)>, InteractionStoreError> {
    let mut statement = transaction.prepare(sql).map_err(storage_error)?;
    let rows = statement
        .query_map(parameters, |row| {
            Ok((row.get::<_, i64>(1)?, row.get::<_, String>(0)?))
        })
        .map_err(storage_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(storage_error)
}

fn update_status(
    store: &SqliteRuntimeStore,
    interaction_id: &InteractionId,
    lease_token: &str,
    now_ms: u64,
    status: &str,
) -> Result<(), InteractionStoreError> {
    if lease_token.is_empty() {
        return Err(InteractionStoreError::InvalidState(
            "lease token 不能为空".to_owned(),
        ));
    }
    let connection = store.connection().map_err(runtime_error)?;
    let changed = connection
        .execute(
            "UPDATE jarvis_interactions SET status=?1,lease_token=NULL,lease_expires_at_ms=NULL,updated_at_ms=?2 WHERE interaction_id=?3 AND status=?4 AND lease_token=?5",
            params![status, i64_value(now_ms).map_err(runtime_error)?, interaction_id.as_str(), STATUS_PROCESSING, lease_token],
        )
        .map_err(storage_error)?;
    if changed == 1 {
        return Ok(());
    }
    let exists = connection
        .query_row(
            "SELECT 1 FROM jarvis_interactions WHERE interaction_id=?1",
            params![interaction_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_error)?;
    match exists {
        Some(_) => Err(InteractionStoreError::InvalidState(
            "Interaction 不处于当前 lease 的 processing 状态".to_owned(),
        )),
        None => Err(InteractionStoreError::NotFound(interaction_id.clone())),
    }
}

fn read_record(row: &Row<'_>) -> rusqlite::Result<InteractionRecord> {
    let interaction_id =
        InteractionId::new(row.get::<_, String>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let source = InteractionSource::new(row.get::<_, String>(1)?, row.get::<_, String>(2)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let scope = serde_json::from_str(&row.get::<_, String>(3)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let audience = serde_json::from_str(&row.get::<_, String>(4)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let reply_route = serde_json::from_str(&row.get::<_, String>(5)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let status = parse_status(&row.get::<_, String>(7)?)?;
    Ok(InteractionRecord {
        interaction_id,
        source,
        scope,
        audience,
        reply_route,
        content: row.get(6)?,
        attachments: row
            .get::<_, Option<String>>(9)?
            .map_or_else(Vec::new, |json| {
                serde_json::from_str(&json).unwrap_or_default()
            }),
        status,
        received_at_ms: u64_value(row.get(8)?)?,
    })
}

fn parse_status(value: &str) -> rusqlite::Result<InteractionStatus> {
    match value {
        STATUS_RECEIVED => Ok(InteractionStatus::Received),
        STATUS_PROCESSING => Ok(InteractionStatus::Processing),
        STATUS_UNDERSTOOD => Ok(InteractionStatus::Understood),
        STATUS_REJECTED => Ok(InteractionStatus::Rejected),
        STATUS_INTERRUPTED => Ok(InteractionStatus::Interrupted),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn status_code(status: InteractionStatus) -> &'static str {
    match status {
        InteractionStatus::Received => STATUS_RECEIVED,
        InteractionStatus::Processing => STATUS_PROCESSING,
        InteractionStatus::Understood => STATUS_UNDERSTOOD,
        InteractionStatus::Rejected => STATUS_REJECTED,
        InteractionStatus::Interrupted => STATUS_INTERRUPTED,
    }
}

fn ensure_same_interaction(
    existing: &InteractionRecord,
    requested: &InteractionRecord,
) -> Result<(), InteractionStoreError> {
    if existing.interaction_id != requested.interaction_id
        || existing.content != requested.content
        || existing.scope != requested.scope
        || existing.audience != requested.audience
        || existing.reply_route != requested.reply_route
    {
        return Err(InteractionStoreError::Conflict(
            "来源幂等键对应的 Interaction 内容不一致".to_owned(),
        ));
    }
    Ok(())
}

fn runtime_error(error: impl std::fmt::Display) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

fn storage_error(error: rusqlite::Error) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> InteractionStoreError {
    InteractionStoreError::Storage(error.to_string())
}
