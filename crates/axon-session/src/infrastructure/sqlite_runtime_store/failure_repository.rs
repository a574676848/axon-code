//! Session Action 执行失败的原子结算。

use super::SqliteRuntimeStore;
use crate::{SessionEventRecord, domain::runtime::DaemonHostRecord, ports::RuntimeStoreError};
use axon_domain::{DaemonSessionId, SessionActionId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

const FAILURE_EVENT_TYPE: &str = "turn_failed";
const RETRY_EVENT_TYPE: &str = "turn_retry_scheduled";

pub(super) fn fail_action(
    store: &SqliteRuntimeStore,
    action_id: &SessionActionId,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    error: &str,
    max_attempts: u32,
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    /*
     * 锁序固定为「先会话锁、后数据库写事务」：本函数在事务里推进水位，提交后再落盘，
     * 两步必须在同一把会话锁内，否则与逐帧追加交错会写出乱序尾并触发「序号与文件尾
     * 不符」的重排。
     */
    let lock = store.event_log_write_lock(session_id);
    let guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(super::helpers::storage)?;
    super::SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let attempt: u32 = transaction
        .query_row(
            "SELECT attempt FROM work_items WHERE execution_id=?1 AND session_id=?2 AND claimed_by=?3 AND claimed_generation=?4",
            params![action_id.as_str(), session_id.as_str(), host.host_id.as_str(), host.generation],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(super::helpers::storage)?
        .ok_or(RuntimeStoreError::Fenced)
        .and_then(|value| u32::try_from(value).map_err(|_| RuntimeStoreError::Storage("Action attempt 超出范围".to_owned())))?;
    let terminal = attempt >= max_attempts;
    let status = if terminal { "failed" } else { "queued" };
    let event_type = if terminal {
        FAILURE_EVENT_TYPE
    } else {
        RETRY_EVENT_TYPE
    };
    let retry_at_ms = (!terminal).then(|| {
        let exponent = attempt.saturating_sub(1).min(6);
        let backoff_ms = 1_000_u64.saturating_mul(1_u64 << exponent);
        now_ms.saturating_add(backoff_ms)
    });
    transaction
        .execute(
            "UPDATE work_items SET status=?1,claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL,lease_json=NULL,retry_at_ms=?2 WHERE execution_id=?3 AND session_id=?4 AND claimed_by=?5 AND claimed_generation=?6",
            params![status, retry_at_ms.map(super::helpers::i64_value).transpose()?, action_id.as_str(), session_id.as_str(), host.host_id.as_str(), host.generation],
        )
        .map_err(super::helpers::storage)?;
    let previous: i64 = transaction
        .query_row(
            "SELECT last_event_sequence FROM sessions WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .map_err(super::helpers::storage)?;
    let sequence = u64::try_from(previous)
        .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?
        .checked_add(1)
        .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
    transaction
        .execute(
            "UPDATE sessions SET status='idle',owner_host_id=?1,owner_generation=?2,last_event_sequence=?3,last_activity_at_ms=?4,updated_at_ms=?4 WHERE session_id=?5",
            params![host.host_id.as_str(), host.generation, i64::try_from(sequence).map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?, i64::try_from(now_ms).map_err(|_| RuntimeStoreError::Storage("时间戳超出范围".to_owned()))?, session_id.as_str()],
        )
        .map_err(super::helpers::storage)?;
    // 终态失败搬进历史表；重试排队态不满足终态谓词，这里是空操作。
    super::work_item_archive::archive_settled_work_item(
        &transaction,
        "execution_id=?1",
        action_id.as_str(),
        super::helpers::i64_value(now_ms)?,
    )
    .map_err(super::helpers::storage)?;
    transaction.commit().map_err(super::helpers::storage)?;
    let written = super::event_log_repository::write_events_at_locked(
        store,
        session_id,
        &host.generation,
        sequence,
        &[(event_type.to_owned(), error.to_owned())],
        now_ms,
        None,
    );
    drop(guard);
    written
}
