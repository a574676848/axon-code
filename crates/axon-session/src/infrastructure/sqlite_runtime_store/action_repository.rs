//! Session Action repository 边界。
use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage, u64_value};
use crate::domain::{DaemonHostRecord, SessionActionRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, SessionActionId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

const ACTION_COLUMNS: &str = "execution_id,goal_id,session_id,kind,priority,status,dedupe_key,payload_json,goal_revision,context_version,claimed_by,claimed_generation,lease_expires_at_ms,attempt";

/// 领取下一个可执行 Action。
pub(super) fn claim_next_action(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    now_ms: u64,
    lease_duration_ms: u64,
) -> Result<Option<SessionActionRecord>, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let now_val = i64_value(now_ms)?;
    let queued_candidate: Option<(String, i32, u64)> = transaction
        .query_row(
            "SELECT execution_id, priority, created_at_ms FROM jarvis_executions WHERE session_id=?1 AND status='queued' AND (retry_at_ms IS NULL OR retry_at_ms <= ?2) ORDER BY priority DESC, created_at_ms ASC, execution_id ASC LIMIT 1",
            params![session_id.as_str(), now_val],
            |row| Ok((row.get(0)?, row.get(1)?, u64_value(row.get(2)?)?)),
        )
        .optional()
        .map_err(storage)?;

    // 租约失效的 running Execution 由 `quarantine_interrupted_executions` 收敛为 uncertain，
    // 不在 claim 路径重跑，避免重复外部副作用。
    let stale_waiting_candidate: Option<(String, i32, u64)> = transaction
        .query_row(
            "SELECT execution_id, priority, created_at_ms FROM jarvis_executions WHERE session_id=?1 AND status='waiting' AND lease_expires_at_ms <= ?2 ORDER BY priority DESC, created_at_ms ASC, execution_id ASC LIMIT 1",
            params![session_id.as_str(), now_val],
            |row| Ok((row.get(0)?, row.get(1)?, u64_value(row.get(2)?)?)),
        )
        .optional()
        .map_err(storage)?;

    // 租约失效的 running Execution 由 `quarantine_interrupted_executions` 收敛为 uncertain，
    // 不在 claim 路径重跑，避免重复外部副作用。
    let action_id = [queued_candidate, stale_waiting_candidate]
        .into_iter()
        .flatten()
        .max_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| b.0.cmp(&a.0))
        })
        .map(|c| c.0);
    let Some(action_id) = action_id else {
        transaction.commit().map_err(storage)?;
        return Ok(None);
    };
    let expires = i64_value(now_ms.saturating_add(lease_duration_ms))?;
    let changed = transaction
        .execute(
            "UPDATE jarvis_executions SET status='running',claimed_by=?1,claimed_generation=?2,lease_expires_at_ms=?3,attempt=attempt+1,started_at_ms=COALESCE(started_at_ms,?4),retry_at_ms=NULL,lease_json=json_object('owner',?1,'token',?5,'expires_at_ms',?3) WHERE execution_id=?6 AND (status='queued' OR (status='waiting' AND lease_expires_at_ms <= ?4))",
            params![
                host.host_id.as_str(),
                host.generation,
                expires,
                i64_value(now_ms)?,
                Uuid::new_v4().to_string(),
                action_id
            ],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    let action = transaction
        .query_row(
            &format!("SELECT {ACTION_COLUMNS} FROM jarvis_executions WHERE execution_id=?1"),
            params![action_id],
            SqliteRuntimeStore::read_action,
        )
        .map_err(storage)?;
    transaction.commit().map_err(storage)?;
    Ok(Some(action))
}

/// 续租当前 Host 正在执行的 Action 租约。
///
/// 只在 `claimed_by` 与 `claimed_generation` 都匹配、租约尚未过期、且没有取消请求时
/// 生效；`lease_json` 里的 token 原样保留，因此续租不改变 claim 身份，只是把到期时间
/// 推后一个租约时长。条件不成立返回 [`RuntimeStoreError::Fenced`]：调用方必须停止续租。
pub(super) fn renew_action_lease(
    store: &SqliteRuntimeStore,
    action_id: &SessionActionId,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    now_ms: u64,
    lease_duration_ms: u64,
) -> Result<(), RuntimeStoreError> {
    if lease_duration_ms == 0 {
        return Err(RuntimeStoreError::Storage(
            "lease 时长必须大于 0".to_owned(),
        ));
    }
    let connection = store.connection()?;
    let now = i64_value(now_ms)?;
    let expires = i64_value(now_ms.saturating_add(lease_duration_ms))?;
    let changed = connection
        .execute(
            "UPDATE jarvis_executions SET lease_expires_at_ms=?1,\
             lease_json=json_object('owner',claimed_by,'token',json_extract(lease_json,'$.token'),'expires_at_ms',?1),\
             updated_at_ms=?2 \
             WHERE execution_id=?3 AND session_id=?4 AND status='running' \
             AND cancellation_requested_at_ms IS NULL \
             AND claimed_by=?5 AND claimed_generation=?6 AND lease_expires_at_ms>?2",
            params![
                expires,
                now,
                action_id.as_str(),
                session_id.as_str(),
                host.host_id.as_str(),
                host.generation,
            ],
        )
        .map_err(storage)?;
    if changed == 1 {
        return Ok(());
    }
    Err(RuntimeStoreError::Fenced)
}

/// 取消 Action。
pub(super) fn cancel_action(
    store: &SqliteRuntimeStore,
    action_id: &SessionActionId,
    session_id: &DaemonSessionId,
    now_ms: u64,
) -> Result<SessionActionRecord, RuntimeStoreError> {
    let connection = store.connection()?;
    let changed = connection
        .execute(
            "UPDATE jarvis_executions SET status='cancelled',cancelled_at_ms=?1,lease_json=NULL,claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL WHERE execution_id=?2 AND session_id=?3 AND status IN ('queued','waiting','running')",
            params![i64_value(now_ms)?, action_id.as_str(), session_id.as_str()],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Conflict(
            "Action 不在可取消状态".to_owned(),
        ));
    }
    connection
        .query_row(
            &format!("SELECT {ACTION_COLUMNS} FROM jarvis_executions WHERE execution_id=?1"),
            params![action_id.as_str()],
            SqliteRuntimeStore::read_action,
        )
        .map_err(storage)
}

/// 以当前 Session epoch 原子入队 Action。
pub(super) fn enqueue_action(
    store: &SqliteRuntimeStore,
    action: &SessionActionRecord,
    now_ms: u64,
) -> Result<SessionActionRecord, RuntimeStoreError> {
    store.initialize()?;
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    if let Some(existing) = find_by_action_id(&transaction, action)? {
        transaction.commit().map_err(storage)?;
        return Ok(existing);
    }
    if let Some(existing) = find_by_dedupe_key(&transaction, action)? {
        transaction.commit().map_err(storage)?;
        return Ok(existing);
    }
    let context_version = transaction
        .query_row(
            "SELECT context_version FROM session_runtimes WHERE session_id=?1",
            params![action.session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| RuntimeStoreError::NotFound(action.session_id.to_string()))
        .and_then(|value| u64_value(value).map_err(storage))?;
    let goal_id = action
        .goal_id
        .as_ref()
        .map(|goal_id| goal_id.as_str())
        .unwrap_or(action.action_id.as_str());
    transaction
        .execute(
            "INSERT INTO jarvis_executions(execution_id,goal_id,session_id,kind,priority,status,dedupe_key,payload_json,goal_revision,context_version,attempt,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",
            params![
                action.action_id.as_str(),
                goal_id,
                action.session_id.as_str(),
                action.kind,
                action.priority,
                action.status.as_str(),
                action.dedupe_key,
                action.payload_json,
                action.goal_revision.map(i64_value).transpose()?,
                i64_value(context_version)?,
                i64::from(action.attempt),
                i64_value(now_ms)?
            ],
        )
        .map_err(storage)?;
    transaction.commit().map_err(storage)?;
    Ok(SessionActionRecord {
        context_version: Some(context_version),
        ..action.clone()
    })
}

fn find_by_action_id(
    transaction: &rusqlite::Transaction<'_>,
    action: &SessionActionRecord,
) -> Result<Option<SessionActionRecord>, RuntimeStoreError> {
    let existing = transaction
        .query_row(
            &format!("SELECT {ACTION_COLUMNS} FROM jarvis_executions WHERE execution_id=?1"),
            params![action.action_id.as_str()],
            SqliteRuntimeStore::read_action,
        )
        .optional()
        .map_err(storage)?;
    if existing
        .as_ref()
        .is_some_and(|record| record.session_id != action.session_id)
    {
        return Err(RuntimeStoreError::Conflict(
            "Action ID 已绑定其他 Session".to_owned(),
        ));
    }
    Ok(existing)
}

fn find_by_dedupe_key(
    transaction: &rusqlite::Transaction<'_>,
    action: &SessionActionRecord,
) -> Result<Option<SessionActionRecord>, RuntimeStoreError> {
    let Some(dedupe_key) = &action.dedupe_key else {
        return Ok(None);
    };
    transaction
        .query_row(
            &format!(
                "SELECT {ACTION_COLUMNS} FROM jarvis_executions WHERE session_id=?1 AND dedupe_key=?2 AND status NOT IN ('completed','cancelled','failed')"
            ),
            params![action.session_id.as_str(), dedupe_key],
            SqliteRuntimeStore::read_action,
        )
        .optional()
        .map_err(storage)
}

/// Action 查询 repository 视图。
#[derive(Debug, Clone, Copy)]
pub struct ActionRepository<'a, S> {
    /// 底层运行时事实源。
    pub store: &'a S,
}
impl<'a, S: RuntimeStore> ActionRepository<'a, S> {
    /// 创建 repository 视图。
    #[must_use]
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
    /// 读取 Session 的待处理 Action。
    pub fn pending(
        &self,
        session_id: &DaemonSessionId,
        limit: u32,
    ) -> Result<Vec<crate::domain::SessionActionRecord>, RuntimeStoreError> {
        self.store.list_pending_actions(session_id, limit)
    }
}
