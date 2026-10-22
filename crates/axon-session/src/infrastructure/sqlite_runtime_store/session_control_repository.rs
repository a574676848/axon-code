//! Session 控制事务的 SQLite 持久化实现。

use axon_domain::DaemonSessionId;
use rusqlite::{TransactionBehavior, params};
use uuid::Uuid;

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::domain::runtime::{DaemonHostRecord, SessionRuntimeRecord};
use crate::ports::RuntimeStoreError;

/// 停止当前 Session，并让正在执行的 Worker 在提交边界被 fencing。
pub(super) fn stop_session(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    now_ms: u64,
) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    cancel_open_actions(&transaction, session_id, now_ms)?;
    let changed = transaction
        .execute(
            "UPDATE session_runtimes
             SET status='idle',owner_host_id=?1,owner_generation=?2,
                 last_activity_at_ms=?3,updated_at_ms=?3
             WHERE session_id=?4",
            params![
                host.host_id.as_str(),
                host.generation,
                i64_value(now_ms)?,
                session_id.as_str()
            ],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::NotFound(session_id.to_string()));
    }
    transaction.commit().map_err(storage)?;
    store.get_session_impl(session_id)
}

/// 停止旧 rollout 并复用 transport Session ID 创建新的 rollout 身份。
pub(super) fn reset_session(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    cwd: &str,
    host: &DaemonHostRecord,
    now_ms: u64,
) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
    let run_session_id = Uuid::new_v4().to_string();
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    cancel_open_actions(&transaction, session_id, now_ms)?;
    let changed = transaction
        .execute(
            "UPDATE session_runtimes
             SET run_session_id=?1,cwd=?2,status='created',
                 owner_host_id=?3,owner_generation=?4,worker_attempt=worker_attempt+1,
                 context_version=context_version+1,last_activity_at_ms=?5,updated_at_ms=?5
             WHERE session_id=?6",
            params![
                run_session_id,
                cwd,
                host.host_id.as_str(),
                host.generation,
                i64_value(now_ms)?,
                session_id.as_str()
            ],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::NotFound(session_id.to_string()));
    }
    transaction.commit().map_err(storage)?;
    store.get_session_impl(session_id)
}

fn cancel_open_actions(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &DaemonSessionId,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE jarvis_executions
             SET status='cancelled',cancelled_at_ms=?1,
                 claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL,lease_json=NULL
             WHERE session_id=?2 AND status IN ('queued','waiting','running')",
            params![i64_value(now_ms)?, session_id.as_str()],
        )
        .map_err(storage)?;
    Ok(())
}
