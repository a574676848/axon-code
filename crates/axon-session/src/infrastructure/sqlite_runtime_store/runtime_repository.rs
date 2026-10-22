//! Session Runtime repository 边界。
use super::HOST_STATE_RUNNING;
use super::SqliteRuntimeStore;
use super::helpers::{
    i64_value, parse_host_id, parse_runtime_status, parse_session_id, storage, u64_value,
};
use crate::domain::runtime::{DaemonHostRecord, SessionRuntimeRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::RuntimeHostId;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

/// 回收仍标记为 running、但对应进程已经退出的 Host lease。
pub(super) fn reap_dead_hosts(
    store: &SqliteRuntimeStore,
    now_ms: u64,
) -> Result<u32, RuntimeStoreError> {
    store.initialize()?;
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let mut statement = transaction
        .prepare("SELECT host_id,pid FROM daemon_hosts WHERE state=?1")
        .map_err(storage)?;
    let hosts = statement
        .query_map(params![HOST_STATE_RUNNING], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);

    let mut reaped = 0_u32;
    for (host_id, pid) in hosts {
        let lease_active: bool = transaction
            .query_row(
                "SELECT lease_expires_at_ms>?1 FROM daemon_hosts
                 WHERE host_id=?2 AND state=?3",
                params![i64_value(now_ms)?, host_id, HOST_STATE_RUNNING],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if lease_active && super::host_process::host_process_alive(pid) {
            continue;
        }
        reaped = reaped.saturating_add(
            transaction
                .execute(
                    "UPDATE daemon_hosts SET state='stopped',updated_at_ms=?1
                     WHERE host_id=?2 AND state=?3",
                    params![i64_value(now_ms)?, host_id, HOST_STATE_RUNNING],
                )
                .map_err(storage)?
                .try_into()
                .unwrap_or(u32::MAX),
        );
    }
    transaction.commit().map_err(storage)?;
    Ok(reaped)
}

/// 直接扫描 `jarvis_executions` 并返回需要唤醒的内部运行上下文。
pub(super) fn list_runnable_execution_sessions(
    store: &SqliteRuntimeStore,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError> {
    let connection = store.connection()?;
    let mut statement = connection
        .prepare("SELECT r.session_id,r.run_session_id,r.root_session_id,r.cwd,r.status,r.owner_host_id,r.owner_generation,r.worker_attempt,r.context_version,r.last_checkpoint_seq,r.last_event_sequence,r.last_activity_at_ms FROM session_runtimes r WHERE r.status IN ('created','idle','running','recovering') AND EXISTS (SELECT 1 FROM jarvis_executions a WHERE a.session_id=r.session_id AND (a.status='queued' OR (a.status='running' AND a.cancellation_requested_at_ms IS NULL AND a.lease_expires_at_ms <= ?1))) ORDER BY r.last_activity_at_ms ASC LIMIT ?2")
        .map_err(storage)?;
    let rows = statement
        .query_map(params![i64_value(now_ms)?, i64::from(limit)], |row| {
            Ok(SessionRuntimeRecord {
                session_id: parse_session_id(row.get(0)?)?,
                run_session_id: row.get(1)?,
                root_session_id: parse_session_id(row.get(2)?)?,
                cwd: row.get(3)?,
                status: parse_runtime_status(&row.get::<_, String>(4)?),
                owner_host_id: row
                    .get::<_, Option<String>>(5)?
                    .map(parse_host_id)
                    .transpose()?,
                owner_generation: row.get(6)?,
                worker_attempt: u32::try_from(row.get::<_, i64>(7)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                context_version: u64_value(row.get(8)?)?,
                last_checkpoint_seq: u64_value(row.get(9)?)?,
                last_event_sequence: u64_value(row.get(10)?)?,
                last_activity_at_ms: u64_value(row.get(11)?)?,
            })
        })
        .map_err(storage)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(storage)
}

pub(super) fn claim_host(
    store: &SqliteRuntimeStore,
    host_id: &RuntimeHostId,
    pid: u32,
    process_start_id: Option<&str>,
    now_ms: u64,
    lease_duration_ms: u64,
) -> Result<DaemonHostRecord, RuntimeStoreError> {
    store.initialize()?;
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let active: Option<String> = transaction
        .query_row(
            "SELECT host_id FROM daemon_hosts \
             WHERE state = ?1 AND lease_expires_at_ms > ?2 LIMIT 1",
            params![HOST_STATE_RUNNING, i64_value(now_ms)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if active
        .as_deref()
        .is_some_and(|value| value != host_id.as_str())
    {
        return Err(RuntimeStoreError::Conflict(
            "已有 Daemon Host 持有 lease".to_owned(),
        ));
    }
    let generation = format!("{}-{}-{}", now_ms, pid, Uuid::new_v4());
    let expires = now_ms.saturating_add(lease_duration_ms);
    transaction
        .execute(
            "INSERT INTO daemon_hosts(host_id,generation,pid,process_start_id,state, \
                lease_expires_at_ms,created_at_ms,updated_at_ms) \
             VALUES(?1,?2,?3,?4,?5,?6,?7,?7) \
             ON CONFLICT(host_id) DO UPDATE SET generation=excluded.generation, \
                pid=excluded.pid,process_start_id=excluded.process_start_id, \
                state=excluded.state,lease_expires_at_ms=excluded.lease_expires_at_ms, \
                updated_at_ms=excluded.updated_at_ms",
            params![
                host_id.as_str(),
                generation,
                i64::from(pid),
                process_start_id,
                HOST_STATE_RUNNING,
                i64_value(expires)?,
                i64_value(now_ms)?
            ],
        )
        .map_err(storage)?;
    transaction.commit().map_err(storage)?;
    Ok(DaemonHostRecord {
        host_id: host_id.clone(),
        generation,
        pid,
        process_start_id: process_start_id.map(str::to_owned),
        state: HOST_STATE_RUNNING.to_owned(),
        lease_expires_at_ms: expires,
    })
}

pub(super) fn renew_host(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    lease_duration_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let connection = store.connection()?;
    let changed = connection
        .execute(
            "UPDATE daemon_hosts SET lease_expires_at_ms=?1,updated_at_ms=?2 \
             WHERE host_id=?3 AND generation=?4 AND state=?5 \
               AND lease_expires_at_ms > ?2",
            params![
                i64_value(now_ms.saturating_add(lease_duration_ms))?,
                i64_value(now_ms)?,
                host.host_id.as_str(),
                host.generation,
                HOST_STATE_RUNNING
            ],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    Ok(())
}

pub(super) fn request_upgrade(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    handoff_token: &str,
    target_version: &str,
    deadline_at_ms: u64,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let changed = transaction
        .execute(
            "UPDATE daemon_hosts SET state='upgrade_requested',updated_at_ms=?1 \
         WHERE host_id=?2 AND generation=?3 AND state='running'",
            params![i64_value(now_ms)?, host.host_id.as_str(), host.generation],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    let admission_id = Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO daemon_upgrade_admissions(admission_id,handoff_token,target_version,old_host_id,old_generation,deadline_at_ms,created_at_ms,updated_at_ms,state) \
         VALUES(?1,?2,?3,?4,?5,?6,?7,?7,'requested')",
        params![
            admission_id,
            handoff_token,
            target_version,
            host.host_id.as_str(),
            host.generation,
            i64_value(deadline_at_ms)?,
            i64_value(now_ms)?
        ],
    ).map_err(storage)?;
    transaction.commit().map_err(storage)
}

pub(super) fn mark_handoff_ready(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    handoff_token: &str,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let active_actions: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM jarvis_executions WHERE claimed_by=?1 AND claimed_generation=?2 AND status='running' AND lease_expires_at_ms > ?3",
            params![host.host_id.as_str(), host.generation, i64_value(now_ms)?],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if active_actions > 0 {
        return Err(RuntimeStoreError::Conflict(
            "仍有 Provider Action 执行中，不能完成升级 handoff".to_owned(),
        ));
    }
    let changed = transaction
        .execute(
            "UPDATE daemon_hosts SET state='handoff_ready',updated_at_ms=?1 \
         WHERE host_id=?2 AND generation=?3 AND state='upgrade_requested'",
            params![i64_value(now_ms)?, host.host_id.as_str(), host.generation],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    let admission = transaction
        .execute(
            "UPDATE daemon_upgrade_admissions SET state='ready',updated_at_ms=?4 \
         WHERE handoff_token=?1 AND old_host_id=?2 AND old_generation=?3 AND state='requested'",
            params![
                handoff_token,
                host.host_id.as_str(),
                host.generation,
                i64_value(now_ms)?
            ],
        )
        .map_err(storage)?;
    if admission == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    transaction.commit().map_err(storage)
}

pub(super) fn claim_upgrade_replacement(
    store: &SqliteRuntimeStore,
    host_id: &RuntimeHostId,
    pid: u32,
    process_start_id: Option<&str>,
    handoff_token: &str,
    now_ms: u64,
    lease_duration_ms: u64,
) -> Result<DaemonHostRecord, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let admission: Option<(String, String)> = transaction
        .query_row(
            "SELECT old_host_id,old_generation FROM daemon_upgrade_admissions WHERE handoff_token=?1 AND state='ready' AND deadline_at_ms>=?2",
            params![handoff_token, i64_value(now_ms)?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    let Some((old_host_id, old_generation)) = admission else {
        return Err(RuntimeStoreError::Fenced);
    };
    let generation = format!("{}-{}-{}", now_ms, pid, Uuid::new_v4());
    let expires = now_ms.saturating_add(lease_duration_ms);
    transaction.execute(
        "INSERT INTO daemon_hosts(host_id,generation,pid,process_start_id,state,lease_expires_at_ms,created_at_ms,updated_at_ms) \
         VALUES(?1,?2,?3,?4,'running',?5,?6,?6)",
        params![
            host_id.as_str(),
            generation,
            i64::from(pid),
            process_start_id,
            i64_value(expires)?,
            i64_value(now_ms)?
        ],
    ).map_err(storage)?;
    let stopped = transaction
        .execute(
            "UPDATE daemon_hosts SET state='stopped',updated_at_ms=?1 \
         WHERE host_id=?2 AND generation=?3 AND state='handoff_ready'",
            params![i64_value(now_ms)?, old_host_id, old_generation],
        )
        .map_err(storage)?;
    if stopped == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    transaction.execute(
        "UPDATE daemon_upgrade_admissions SET state='claimed',claimed_by=?1,claimed_at_ms=?2,updated_at_ms=?2 \
         WHERE handoff_token=?3 AND state='ready'",
        params![host_id.as_str(), i64_value(now_ms)?, handoff_token],
    ).map_err(storage)?;
    transaction.commit().map_err(storage)?;
    Ok(DaemonHostRecord {
        host_id: host_id.clone(),
        generation,
        pid,
        process_start_id: process_start_id.map(str::to_owned),
        state: HOST_STATE_RUNNING.to_owned(),
        lease_expires_at_ms: expires,
    })
}

pub(super) fn recover_expired_sessions(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let mut statement = transaction.prepare(
        "SELECT runtime.session_id FROM session_runtimes AS runtime \
         LEFT JOIN daemon_hosts AS owner ON owner.host_id = runtime.owner_host_id AND owner.generation = runtime.owner_generation \
         WHERE runtime.status IN ('running','compacting','waiting') AND (runtime.owner_host_id IS NULL OR owner.host_id IS NULL OR owner.lease_expires_at_ms <= ?1) \
         ORDER BY runtime.last_activity_at_ms ASC LIMIT ?2"
    ).map_err(storage)?;
    let ids = statement
        .query_map(params![i64_value(now_ms)?, i64::from(limit)], |row| {
            row.get::<_, String>(0)
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);
    let mut recovered = Vec::with_capacity(ids.len());
    for id in ids {
        let changed = transaction.execute(
            "UPDATE session_runtimes SET status='recovering',owner_host_id=?1,owner_generation=?2,worker_attempt=worker_attempt+1,last_activity_at_ms=?3,updated_at_ms=?3 WHERE session_id=?4",
            params![host.host_id.as_str(), host.generation, i64_value(now_ms)?, id],
        ).map_err(storage)?;
        if changed == 0 {
            continue;
        }
        let next_event: i64 = transaction
            .query_row(
                "SELECT last_event_sequence + 1 FROM session_runtimes WHERE session_id=?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        transaction.execute(
            "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms) \
             VALUES(?1,?2,?3,'session_recovered',?4,?5)",
            params![
                id,
                next_event,
                host.generation,
                serde_json::json!({"ownerGeneration": host.generation, "checkpointRecovered": true}).to_string(),
                i64_value(now_ms)?
            ],
        ).map_err(storage)?;
        transaction
            .execute(
                "UPDATE session_runtimes SET last_event_sequence=?1 WHERE session_id=?2",
                params![next_event, id],
            )
            .map_err(storage)?;
        recovered.push(transaction.query_row(
            "SELECT session_id,run_session_id,root_session_id,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms \
             FROM session_runtimes WHERE session_id=?1",
            params![id],
            |row| Ok(SessionRuntimeRecord {
                session_id: parse_session_id(row.get(0)?)?,
                run_session_id: row.get(1)?,
                root_session_id: parse_session_id(row.get(2)?)?,
                cwd: row.get(3)?,
                status: parse_runtime_status(&row.get::<_, String>(4)?),
                owner_host_id: row.get::<_, Option<String>>(5)?.map(parse_host_id).transpose()?,
                owner_generation: row.get(6)?,
                worker_attempt: u32::try_from(row.get::<_, i64>(7)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
                context_version: u64_value(row.get(8)?)?,
                last_checkpoint_seq: u64_value(row.get(9)?)?,
                last_event_sequence: u64_value(row.get(10)?)?,
                last_activity_at_ms: u64_value(row.get(11)?)?,
            }),
        ).map_err(storage)?);
    }
    transaction.commit().map_err(storage)?;
    Ok(recovered)
}

/// Runtime 聚合 repository 视图。
#[derive(Debug, Clone, Copy)]
pub struct RuntimeRepository<'a, S> {
    /// 底层运行时事实源。
    pub store: &'a S,
}
impl<'a, S: RuntimeStore> RuntimeRepository<'a, S> {
    /// 创建 repository 视图。
    #[must_use]
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
    /// 初始化事实源。
    pub fn initialize(&self) -> Result<(), RuntimeStoreError> {
        self.store.initialize()
    }
}
