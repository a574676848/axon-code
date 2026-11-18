//! `inbox` 的 SQLite 持久化实现。
//!
//! `inbox` 是**只读入站事实**：写入后不再改状态。处理状态机整体搬到 `work_items`：
//! 一条消息对应一个工作项（`work_items.inbox_id` 唯一），领取、续租、终态都只更新
//! 工作项那一行。读取侧把两者 JOIN 起来，把工作项状态派生回领域状态，
//! 因此上层看到的 `InteractionRecord` 语义不变。

use super::SqliteRuntimeStore;
use super::helpers::i64_value;
use super::interaction_query_repository::{WORK_RUNNING, select_inbox};
use super::interaction_repository_row::{
    read_record, runtime_error, serialize_error, storage_error,
};
use crate::ports::{
    InteractionAppendReceipt, InteractionProcessingLease, InteractionStore, InteractionStoreError,
    RuntimeStore,
};
use axon_domain::{InteractionId, InteractionRecord, InteractionStatus};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

/// 工作项的排队状态：已释放、等待重新领取。
const WORK_QUEUED: &str = "queued";
/// 终态：正常结束。
const TERMINAL_UNDERSTOOD: &str = "understood";
/// 终态：被拒绝。
const TERMINAL_REJECTED: &str = "rejected";
/// 终态：进程中断。
const TERMINAL_INTERRUPTED: &str = "interrupted";
const SELECT_INTERACTION_ID_EXISTS: &str = "SELECT 1 FROM inbox WHERE interaction_id=?1";
const INSERT_INTERACTION: &str = "INSERT INTO inbox(interaction_id,source_channel,source_message_id,scope_json,audience_json,reply_route_json,content,received_at_ms,updated_at_ms,attachments_json,model,reasoning_effort,session_id,client) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8,?9,?10,?11,?12,?13)";
/// session 排他领取候选：没有工作项、或工作项已释放的那些；所属 session 当前不得有在途工作项。
const SELECT_CLAIMABLE_INTERACTIONS: &str = "SELECT queued.interaction_id,queued.received_at_ms \
     FROM inbox AS queued \
     LEFT JOIN work_items AS current ON current.inbox_id=queued.interaction_id \
     WHERE (current.execution_id IS NULL OR current.status=?1) \
       AND NOT EXISTS(SELECT 1 FROM work_item_history AS done \
                      WHERE done.inbox_id=queued.interaction_id) \
       AND NOT EXISTS(SELECT 1 FROM work_items AS active \
                      JOIN inbox AS session ON session.interaction_id=active.inbox_id \
                      WHERE session.session_id=queued.session_id AND active.status=?2 \
                        AND active.lease_expires_at_ms>?3) \
     ORDER BY queued.received_at_ms ASC, queued.interaction_id ASC LIMIT ?4";
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
            if !start_work_item(&transaction, &id, &token, now, expires)? {
                continue;
            }
            let interaction = transaction
                .query_row(
                    &select_inbox("WHERE i.interaction_id=?1"),
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
        settle_work_item(
            self,
            interaction_id,
            lease_token,
            now_ms,
            "completed",
            TERMINAL_UNDERSTOOD,
        )
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
        if !start_work_item(&transaction, interaction_id.as_str(), &token, now, expires)? {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        }
        let interaction = transaction
            .query_row(
                &select_inbox("WHERE i.interaction_id=?1"),
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
                "UPDATE work_items SET lease_expires_at_ms=?1,updated_at_ms=?2 \
                 WHERE inbox_id=?3 AND lease_token=?4 AND status=?5",
                params![
                    i64_value(expires_at_ms).map_err(runtime_error)?,
                    i64_value(now_ms).map_err(runtime_error)?,
                    interaction_id.as_str(),
                    lease_token,
                    WORK_RUNNING
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
        release_work_item(self, interaction_id, lease_token, now_ms)
    }

    fn mark_rejected(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        settle_work_item(
            self,
            interaction_id,
            lease_token,
            now_ms,
            "cancelled",
            TERMINAL_REJECTED,
        )
    }

    fn mark_interrupted(
        &self,
        interaction_id: &InteractionId,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), InteractionStoreError> {
        settle_work_item(
            self,
            interaction_id,
            lease_token,
            now_ms,
            "failed",
            TERMINAL_INTERRUPTED,
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
            &select_inbox("WHERE i.source_channel=?1 AND i.source_message_id=?2"),
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
                i64_value(interaction.received_at_ms).map_err(runtime_error)?,
                serde_json::to_string(&interaction.attachments).map_err(serialize_error)?,
                // 显式模型只跟随本轮 turn，不写进 Workspace 或 session 身份。
                interaction.model.as_deref(),
                // 推理强度与模型同源：随本轮下发，不写进 Workspace 或 session 身份。
                interaction.reasoning_effort.as_deref(),
                // session 是主 turn 排他领取的事实源：入站时必须由调用方显式给出会话标识。
                interaction.session_id.as_str(),
                // 客户端来源：与车道渠道分开记，逐轮标注与跨端混合统计读它。
                interaction.source.client().unwrap_or_default(),
            ],
        )
        .map(|_| ())
        .map_err(storage_error)
}

/// 领取候选集合：只包含没有工作项、或工作项已释放的入站事实。
///
/// session 内串行是主 turn 的硬约束：同一 session 已有未过期的在途工作项时，该 session 的
/// 后续记录必须留在队列里，否则协调线程会把同 session 的两条输入并发交给 Provider。
/// 过期租约不再盲目重领；由中断恢复路径先证明原 worker 已不可达，再收敛为 interrupted。
fn claimable_ids(
    transaction: &Transaction<'_>,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<String>, InteractionStoreError> {
    let candidates = ordered_candidates(
        transaction,
        SELECT_CLAIMABLE_INTERACTIONS,
        params![
            WORK_QUEUED,
            WORK_RUNNING,
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

/// 建立或接管一个工作项的租约。
///
/// 每个入站事实最多一个工作项：首次领取创建行，释放后的再领取复用同一行并换新的
/// `lease_token`，因此旧持有者的令牌在结算与续租时都会失效。已进入终态的工作项不可再领。
fn start_work_item(
    transaction: &Transaction<'_>,
    interaction_id: &str,
    lease_token: &str,
    now: i64,
    expires: i64,
) -> Result<bool, InteractionStoreError> {
    super::work_item_claim::start_interaction_item(
        transaction,
        interaction_id,
        Uuid::new_v4().to_string(),
        WORK_RUNNING,
        WORK_QUEUED,
        lease_token,
        now,
        expires,
    )
    .map_err(storage_error)
}

/// 释放租约：工作项回到排队态，令牌立即失效。
fn release_work_item(
    store: &SqliteRuntimeStore,
    interaction_id: &InteractionId,
    lease_token: &str,
    now_ms: u64,
) -> Result<(), InteractionStoreError> {
    if lease_token.is_empty() {
        return Err(InteractionStoreError::InvalidState(
            "lease token 不能为空".to_owned(),
        ));
    }
    let connection = store.connection().map_err(runtime_error)?;
    let changed = connection
        .execute(
            "UPDATE work_items SET status=?1,lease_token=NULL,lease_expires_at_ms=NULL,updated_at_ms=?2 \
             WHERE inbox_id=?3 AND lease_token=?4 AND status=?5",
            params![
                WORK_QUEUED,
                i64_value(now_ms).map_err(runtime_error)?,
                interaction_id.as_str(),
                lease_token,
                WORK_RUNNING
            ],
        )
        .map_err(storage_error)?;
    if changed == 1 {
        return Ok(());
    }
    Err(inbox_state_error(&connection, interaction_id)?)
}

/// 结算工作项：写入终态与终态类别，令牌立即失效。
fn settle_work_item(
    store: &SqliteRuntimeStore,
    interaction_id: &InteractionId,
    lease_token: &str,
    now_ms: u64,
    status: &str,
    terminal_kind: &str,
) -> Result<(), InteractionStoreError> {
    if lease_token.is_empty() {
        return Err(InteractionStoreError::InvalidState(
            "lease token 不能为空".to_owned(),
        ));
    }
    let connection = store.connection().map_err(runtime_error)?;
    let changed = connection
        .execute(
            "UPDATE work_items SET status=?1,terminal_kind=?2,lease_token=NULL,lease_expires_at_ms=NULL,updated_at_ms=?3,committed_at_ms=?3 \
             WHERE inbox_id=?4 AND lease_token=?5 AND status=?6",
            params![
                status,
                terminal_kind,
                i64_value(now_ms).map_err(runtime_error)?,
                interaction_id.as_str(),
                lease_token,
                WORK_RUNNING
            ],
        )
        .map_err(storage_error)?;
    if changed == 1 {
        // 终态行搬进历史表：在途表只留排队与运行中的行。
        super::work_item_archive::archive_work_item_by_inbox(
            &connection,
            interaction_id.as_str(),
            i64_value(now_ms).map_err(runtime_error)?,
        )
        .map_err(storage_error)?;
        return Ok(());
    }
    Err(inbox_state_error(&connection, interaction_id)?)
}

/// 结算失败时的错误归因：入站事实存在即为租约失效，否则是真不存在。
fn inbox_state_error(
    connection: &rusqlite::Connection,
    interaction_id: &InteractionId,
) -> Result<InteractionStoreError, InteractionStoreError> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM inbox WHERE interaction_id=?1",
            params![interaction_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_error)?;
    Ok(match exists {
        Some(_) => InteractionStoreError::InvalidState(
            "Interaction 不处于当前 lease 的 processing 状态".to_owned(),
        ),
        None => InteractionStoreError::NotFound(interaction_id.clone()),
    })
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
