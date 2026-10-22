//! Schedule repository 边界。
use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::domain::RuntimeScheduleRecord;
use crate::domain::runtime::DaemonHostRecord;
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::DaemonSessionId;
use rusqlite::{TransactionBehavior, params};
/// 持久调度 repository 视图。
#[derive(Debug, Clone, Copy)]
pub struct ScheduleRepository<'a, S> {
    /// 底层运行时事实源。
    pub store: &'a S,
}
impl<'a, S: RuntimeStore> ScheduleRepository<'a, S> {
    /// 创建 repository 视图。
    #[must_use]
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
    /// 写入调度计划。
    pub fn upsert(&self, schedule: &RuntimeScheduleRecord) -> Result<(), RuntimeStoreError> {
        self.store.upsert_schedule(schedule)
    }
}

/// 写入或整体覆盖一条调度计划。
pub(super) fn upsert_schedule(
    store: &SqliteRuntimeStore,
    schedule: &RuntimeScheduleRecord,
) -> Result<(), RuntimeStoreError> {
    store.initialize()?;
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    transaction.execute("INSERT INTO runtime_schedules(schedule_id,session_id,kind,expression,payload_json,delivery_mode,status,next_run_at_ms,lease_owner,lease_expires_at_ms,failure_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,NULL,?9) ON CONFLICT(schedule_id) DO UPDATE SET session_id=excluded.session_id,kind=excluded.kind,expression=excluded.expression,payload_json=excluded.payload_json,delivery_mode=excluded.delivery_mode,status=excluded.status,next_run_at_ms=excluded.next_run_at_ms,failure_count=excluded.failure_count", params![schedule.schedule_id, schedule.session_id.as_ref().map(DaemonSessionId::as_str), schedule.kind, schedule.expression, schedule.payload_json, schedule.delivery_mode, schedule.status, schedule.next_run_at_ms.map(i64_value).transpose()?, i64::from(schedule.failure_count)]).map_err(storage)?;
    transaction.commit().map_err(storage)
}

/// 以 Host lease 领取到期调度，返回本轮归属该 Host 的计划。
pub(super) fn claim_due_schedules(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    lease_duration_ms: u64,
    limit: u32,
) -> Result<Vec<RuntimeScheduleRecord>, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let expires = i64_value(now_ms.saturating_add(lease_duration_ms))?;
    let mut statement = transaction.prepare("SELECT schedule_id FROM runtime_schedules WHERE status='active' AND next_run_at_ms <= ?1 AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?1) ORDER BY next_run_at_ms ASC LIMIT ?2").map_err(storage)?;
    let ids = statement
        .query_map(params![i64_value(now_ms)?, i64::from(limit)], |row| {
            row.get::<_, String>(0)
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);
    let mut result = Vec::with_capacity(ids.len());
    for id in ids {
        transaction.execute("UPDATE runtime_schedules SET lease_owner=?1,lease_expires_at_ms=?2 WHERE schedule_id=?3 AND status='active' AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?4)", params![host.host_id.as_str(), expires, id, i64_value(now_ms)?]).map_err(storage)?;
        result.push(transaction.query_row("SELECT schedule_id,session_id,kind,expression,payload_json,delivery_mode,status,next_run_at_ms,lease_owner,lease_expires_at_ms,failure_count FROM runtime_schedules WHERE schedule_id=?1", params![id], SqliteRuntimeStore::read_schedule).map_err(storage)?);
    }
    transaction.commit().map_err(storage)?;
    Ok(result)
}

/// 结算已领取的调度；`failed` 时累加失败计数。
pub(super) fn settle_schedule(
    store: &SqliteRuntimeStore,
    schedule_id: &str,
    host: &DaemonHostRecord,
    next_run_at_ms: Option<u64>,
    now_ms: u64,
    failed: bool,
) -> Result<(), RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let changed = transaction.execute("UPDATE runtime_schedules SET next_run_at_ms=?1,last_run_at_ms=?2,lease_owner=NULL,lease_expires_at_ms=NULL,failure_count=CASE WHEN ?3 THEN failure_count+1 ELSE 0 END WHERE schedule_id=?4 AND lease_owner=?5", params![next_run_at_ms.map(i64_value).transpose()?, i64_value(now_ms)?, failed, schedule_id, host.host_id.as_str()]).map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    transaction.commit().map_err(storage)
}
