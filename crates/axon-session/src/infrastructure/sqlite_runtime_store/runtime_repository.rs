//! Session Runtime repository 边界。
use super::HOST_STATE_RUNNING;
use super::SqliteRuntimeStore;
use super::helpers::{i64_value, parse_session_id, session_runtime_row, storage};
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
        .prepare("SELECT host_id,pid FROM daemon_instances WHERE state=?1")
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
                "SELECT lease_expires_at_ms>?1 FROM daemon_instances
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
                    "UPDATE daemon_instances SET state='stopped',updated_at_ms=?1
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

/// 直接扫描 `work_items` 并返回需要唤醒的内部运行上下文。
pub(super) fn list_runnable_execution_sessions(
    store: &SqliteRuntimeStore,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError> {
    let connection = store.connection()?;
    let mut statement = connection
        .prepare("SELECT r.session_id,r.current_run_id,r.root_session_id,r.kind,r.trust_scope,r.workspace_id,r.cwd,r.status,r.owner_host_id,r.owner_generation,r.worker_attempt,r.context_version,r.last_checkpoint_seq,r.last_event_sequence,r.last_activity_at_ms FROM sessions r WHERE r.status IN ('created','idle','running','recovering') AND EXISTS (SELECT 1 FROM work_items a WHERE a.session_id=r.session_id AND (a.status='queued' OR (a.status='running' AND a.cancellation_requested_at_ms IS NULL AND a.lease_expires_at_ms <= ?1))) ORDER BY r.last_activity_at_ms ASC LIMIT ?2")
        .map_err(storage)?;
    let rows = statement
        .query_map(
            params![i64_value(now_ms)?, i64::from(limit)],
            session_runtime_row,
        )
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
            "SELECT host_id FROM daemon_instances \
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
            "INSERT INTO daemon_instances(host_id,generation,pid,process_start_id,state, \
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
            "UPDATE daemon_instances SET lease_expires_at_ms=?1,updated_at_ms=?2 \
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
            "UPDATE daemon_instances SET state='upgrade_requested',updated_at_ms=?1 \
         WHERE host_id=?2 AND generation=?3 AND state='running'",
            params![i64_value(now_ms)?, host.host_id.as_str(), host.generation],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    let admission_id = Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO upgrade_handoffs(admission_id,handoff_token,target_version,old_host_id,old_generation,deadline_at_ms,created_at_ms,updated_at_ms,state) \
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
            "SELECT COUNT(*) FROM work_items WHERE claimed_by=?1 AND claimed_generation=?2 AND status='running' AND lease_expires_at_ms > ?3",
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
            "UPDATE daemon_instances SET state='handoff_ready',updated_at_ms=?1 \
         WHERE host_id=?2 AND generation=?3 AND state='upgrade_requested'",
            params![i64_value(now_ms)?, host.host_id.as_str(), host.generation],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    let admission = transaction
        .execute(
            "UPDATE upgrade_handoffs SET state='ready',updated_at_ms=?4 \
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
            "SELECT old_host_id,old_generation FROM upgrade_handoffs WHERE handoff_token=?1 AND state='ready' AND deadline_at_ms>=?2",
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
        "INSERT INTO daemon_instances(host_id,generation,pid,process_start_id,state,lease_expires_at_ms,created_at_ms,updated_at_ms) \
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
            "UPDATE daemon_instances SET state='stopped',updated_at_ms=?1 \
         WHERE host_id=?2 AND generation=?3 AND state='handoff_ready'",
            params![i64_value(now_ms)?, old_host_id, old_generation],
        )
        .map_err(storage)?;
    if stopped == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    transaction.execute(
        "UPDATE upgrade_handoffs SET state='claimed',claimed_by=?1,claimed_at_ms=?2,updated_at_ms=?2 \
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

/// 恢复租约失效的 Session，并把水位与事件面对齐。
///
/// 分三段执行，目的是**把文件 IO 移出写事务**：
/// 1. 只读取出候选会话与它们的水位（不拿写锁）；
/// 2. 逐会话读事件面末条（文件 IO，不持锁）；
/// 3. 一个短写事务里带水位 CAS 落账，水位在期间被别人改过就跳过该会话。
///
/// 直接在第一段事务里读日志会把写锁按住整个「会话数 × 单会话文件读」的时间：
/// 解锁前 WAL 无法回卷，其他写者全部在 `busy_timeout` 里排队，表现为
/// `database is locked` 风暴。
pub(super) fn recover_expired_sessions(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError> {
    let plan = plan_recovery(store, host, now_ms, limit)?;
    if plan.is_empty() {
        // 没有候选也要过一遍 fence：状态不成立时调用方必须看到 Fenced 而不是空结果。
        let mut connection = store.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
        transaction.commit().map_err(storage)?;
        return Ok(Vec::new());
    }
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let mut recovered = Vec::with_capacity(plan.len());
    let mut pending_events: Vec<(axon_domain::DaemonSessionId, u64, String)> = Vec::new();
    for step in plan {
        // 水位 CAS：第一段到现在之间若有别的写者推进过水位，这一步必须落空，
        // 否则会用旧水位算出的事件序号覆盖别人的进度。
        let changed = transaction
            .execute(
                "UPDATE sessions SET status='recovering',owner_host_id=?1,owner_generation=?2,\
                 worker_attempt=worker_attempt+1,last_activity_at_ms=?3,updated_at_ms=?3,\
                 last_event_sequence=?4 \
                 WHERE session_id=?5 AND last_event_sequence=?6 \
                 AND status IN ('running','compacting','waiting')",
                params![
                    host.host_id.as_str(),
                    host.generation,
                    i64_value(now_ms)?,
                    step.next_event,
                    step.session_id.as_str(),
                    i64_value(step.recorded_sequence)?,
                ],
            )
            .map_err(storage)?;
        if changed == 0 {
            continue;
        }
        pending_events.push((
            step.session_id.clone(),
            step.next_sequence,
            step.payload_json,
        ));
        recovered.push(transaction.query_row(
            "SELECT session_id,current_run_id,root_session_id,kind,trust_scope,workspace_id,cwd,status,owner_host_id,owner_generation,worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,last_activity_at_ms \
             FROM sessions WHERE session_id=?1",
            params![step.session_id.as_str()],
            session_runtime_row,
        ).map_err(storage)?);
    }
    transaction.commit().map_err(storage)?;
    for (session_id, sequence, payload_json) in pending_events {
        let record = crate::domain::runtime::SessionEventRecord {
            session_id,
            sequence,
            generation: host.generation.clone(),
            event_type: "session_recovered".to_owned(),
            payload_json,
            emitted_at_ms: now_ms,
        };
        super::event_log_repository::write_records(store, std::slice::from_ref(&record), None)?;
    }
    Ok(recovered)
}

/// 一段待恢复会话的落账计划：水位、要补的会话级事件与 CAS 条件都已算好。
struct RecoveryStep {
    session_id: axon_domain::DaemonSessionId,
    /// 计划依据的水位；落账时 CAS 比对它，期间被别人推进过就跳过该会话。
    recorded_sequence: u64,
    next_event: i64,
    next_sequence: u64,
    payload_json: String,
}

/// 只读取候选会话，并在**不持写锁**的情况下读完事件面末条，产出落账计划。
fn plan_recovery(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<RecoveryStep>, RuntimeStoreError> {
    let connection = store.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT runtime.session_id, runtime.last_event_sequence FROM sessions AS runtime \
             LEFT JOIN daemon_instances AS owner ON owner.host_id = runtime.owner_host_id AND owner.generation = runtime.owner_generation \
             WHERE runtime.status IN ('running','compacting','waiting') \
             AND (runtime.owner_host_id IS NULL OR owner.host_id IS NULL OR owner.lease_expires_at_ms <= ?1) \
             ORDER BY runtime.last_activity_at_ms ASC LIMIT ?2",
        )
        .map_err(storage)?;
    let candidates = statement
        .query_map(params![i64_value(now_ms)?, i64::from(limit)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);
    let mut plan = Vec::with_capacity(candidates.len());
    for (raw_id, recorded) in candidates {
        let recorded = u64::try_from(recorded)
            .map_err(|_| RuntimeStoreError::Storage("事件水位为负".to_owned()))?;
        let session_id = parse_session_id(raw_id)
            .map_err(|_| RuntimeStoreError::Storage("会话标识无效".to_owned()))?;
        // 水位与事件面对账：控制面记的末条序号必须等于真正落盘的末条序号。
        // 崩溃可能让两者差开（帧已落盘但水位没推进，或控制面先提交而帧没落盘），
        // 不对账就会写出序号不单调的日志。日志缺失时保持水位不变。
        let durable = super::event_log_repository::durable_tail(store, &session_id)?;
        let (watermark, reconciliation) = match durable {
            Some((tail, tail_generation)) if tail != recorded => (
                tail,
                serde_json::json!({
                    "watermarkBefore": recorded,
                    "watermarkAfter": tail,
                    "logTail": tail,
                    "tailGeneration": tail_generation,
                }),
            ),
            _ => (recorded, serde_json::Value::Null),
        };
        let next_event = i64::try_from(watermark.saturating_add(1))
            .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?;
        let next_sequence = u64::try_from(next_event)
            .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?;
        plan.push(RecoveryStep {
            session_id,
            recorded_sequence: recorded,
            next_event,
            next_sequence,
            // 会话级事件，不归属任何 Interaction，故 interaction_id 留空。
            payload_json: serde_json::json!({
                "ownerGeneration": host.generation,
                "checkpointRecovered": true,
                "watermarkReconciled": reconciliation,
            })
            .to_string(),
        });
    }
    Ok(plan)
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
