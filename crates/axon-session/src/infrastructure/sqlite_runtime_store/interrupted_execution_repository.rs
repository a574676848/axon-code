//! 进程中断后的 running Execution 隔离。
//!
//! 持有 lease 的进程消失后，执行可能已经发出外部副作用请求。
//! 目标态不允许盲目重跑，必须在单个事务内把 Execution 收敛为 `uncertain`：
//! 释放 lease、提交 uncertain Result、把 Goal 置为 `waiting` 并追加幂等事件。

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage, u64_value};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, JarvisEventId};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

const UNCERTAIN_STATUS: &str = "uncertain";
const UNCERTAIN_ERROR_KIND: &str = "uncertain";
const WAITING_GOAL_STATUS: &str = "waiting";
const RUNNING_STATUS: &str = "running";
const LEDGER_EVENT_TYPE: &str = "execution.uncertain";
const SESSION_EVENT_TYPE: &str = "turn_uncertain";
const INTERRUPTED_SUMMARY: &str = "执行进程中断，外部副作用结果未知，禁止自动重试";
const DEFAULT_INTERRUPT_LIMIT: u32 = 64;

/// 待隔离 Execution 的读取条件。
#[derive(Clone, Copy)]
enum CandidateSelector<'a> {
    /// 进程崩溃恢复：lease 已失效的 running Execution。
    ExpiredLease,
    /// 升级 deadline：仍由指定 Host generation 持有的 running Execution。
    HostLease {
        /// 旧 Host ID。
        owner_host_id: &'a str,
        /// 旧 Host generation。
        owner_generation: &'a str,
    },
}

/// Session 在隔离时的归属处理方式。
#[derive(Clone, Copy)]
enum SessionSettlement {
    /// 由接管方回收为可再次调度。
    AdoptByCurrentHost,
    /// 保持旧 Host 的 recovering 归属，交由接管方恢复。
    KeepRecovering,
}

/// 隔离租约失效的 running Execution，并返回追加的 Session 事件。
pub(super) fn quarantine_interrupted_executions(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    quarantine_running(
        store,
        host,
        now_ms,
        limit,
        CandidateSelector::ExpiredLease,
        SessionSettlement::AdoptByCurrentHost,
    )
}

/// 隔离仍由当前 Host 持有、但必须在升级 deadline 前中断的 running Execution。
pub(super) fn quarantine_host_executions(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
) -> Result<u32, RuntimeStoreError> {
    let records = quarantine_running(
        store,
        host,
        now_ms,
        DEFAULT_INTERRUPT_LIMIT,
        CandidateSelector::HostLease {
            owner_host_id: host.host_id.as_str(),
            owner_generation: &host.generation,
        },
        SessionSettlement::KeepRecovering,
    )?;
    u32::try_from(records.len())
        .map_err(|_| RuntimeStoreError::Storage("中断 Execution 数量超出范围".to_owned()))
}

fn quarantine_running(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    limit: u32,
    selector: CandidateSelector<'_>,
    settlement: SessionSettlement,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    store.initialize()?;
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let now = i64_value(now_ms)?;
    let mut records = Vec::new();
    for (execution_id, goal_id, session_id) in read_candidates(&transaction, now, limit, selector)?
    {
        let payload = uncertain_payload(&execution_id, &goal_id);
        if !settle_uncertain(&transaction, now, &execution_id, &goal_id, &payload)? {
            continue;
        }
        let Some(session_id) = session_id else {
            continue;
        };
        let Ok(session_id) = DaemonSessionId::new(session_id) else {
            continue;
        };
        let Some(sequence) = append_session_event(&transaction, host, now, &session_id, &payload)?
        else {
            continue;
        };
        if matches!(settlement, SessionSettlement::AdoptByCurrentHost) {
            update_session_owner(&transaction, host, now, &session_id, sequence)?;
        }
        records.push(SessionEventRecord {
            session_id,
            sequence,
            generation: host.generation.clone(),
            event_type: SESSION_EVENT_TYPE.to_owned(),
            payload_json: payload,
            emitted_at_ms: now_ms,
        });
    }
    transaction.commit().map_err(storage)?;
    Ok(records)
}

fn read_candidates(
    transaction: &Transaction<'_>,
    now: i64,
    limit: u32,
    selector: CandidateSelector<'_>,
) -> Result<Vec<(String, String, Option<String>)>, RuntimeStoreError> {
    let (sql, owner_host_id, owner_generation) = match selector {
        CandidateSelector::ExpiredLease => (
            "SELECT execution_id,goal_id,session_id FROM jarvis_executions \
             WHERE status='running' AND lease_expires_at_ms<=?1 \
             ORDER BY lease_expires_at_ms ASC, created_at_ms ASC LIMIT ?2"
                .to_owned(),
            None,
            None,
        ),
        CandidateSelector::HostLease {
            owner_host_id,
            owner_generation,
        } => (
            "SELECT execution_id,goal_id,session_id FROM jarvis_executions \
             WHERE claimed_by=?1 AND claimed_generation=?2 AND status='running' \
             ORDER BY created_at_ms ASC, execution_id ASC LIMIT ?3"
                .to_owned(),
            Some(owner_host_id),
            Some(owner_generation),
        ),
    };
    let mut statement = transaction.prepare(&sql).map_err(storage)?;
    let rows = match (owner_host_id, owner_generation) {
        (Some(owner_host_id), Some(owner_generation)) => statement
            .query_map(
                params![owner_host_id, owner_generation, i64::from(limit)],
                read_candidate_row,
            )
            .map_err(storage)?,
        _ => statement
            .query_map(params![now, i64::from(limit)], read_candidate_row)
            .map_err(storage)?,
    };
    rows.collect::<Result<Vec<_>, _>>().map_err(storage)
}

fn read_candidate_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, Option<String>)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

/// 把一条 running Execution 收敛为 `uncertain`；已被其他事务处理时返回 false。
fn settle_uncertain(
    transaction: &Transaction<'_>,
    now: i64,
    execution_id: &str,
    goal_id: &str,
    payload: &str,
) -> Result<bool, RuntimeStoreError> {
    let changed = transaction
        .execute(
            "UPDATE jarvis_executions SET status=?1,claimed_by=NULL,claimed_generation=NULL,\
             lease_json=NULL,lease_expires_at_ms=NULL,retry_at_ms=NULL,updated_at_ms=?2 \
             WHERE execution_id=?3 AND status=?4",
            params![UNCERTAIN_STATUS, now, execution_id, RUNNING_STATUS],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Ok(false);
    }
    transaction
        .execute(
            "INSERT INTO jarvis_results(result_id,goal_id,execution_id,succeeded,summary,\
             receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) \
             SELECT ?1,?2,?3,0,?4,'[]','[]',?5,?6 \
             WHERE NOT EXISTS(SELECT 1 FROM jarvis_results WHERE execution_id=?3)",
            params![
                format!("result-uncertain-{execution_id}"),
                goal_id,
                execution_id,
                INTERRUPTED_SUMMARY,
                UNCERTAIN_ERROR_KIND,
                now
            ],
        )
        .map_err(storage)?;
    transaction
        .execute(
            "UPDATE jarvis_goals SET status=?1,revision=revision+1,updated_at_ms=?2 \
             WHERE goal_id=?3 AND status IN ('proposed','active')",
            params![WAITING_GOAL_STATUS, now, goal_id],
        )
        .map_err(storage)?;
    let event_id = JarvisEventId::new(format!("event-{LEDGER_EVENT_TYPE}-{execution_id}"))
        .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,\
             cause_event_id,payload_json,occurred_at_ms) \
             VALUES(?1,(SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events),?2,?3,NULL,?4,?5)",
            params![
                event_id.as_str(),
                LEDGER_EVENT_TYPE,
                format!("{LEDGER_EVENT_TYPE}:{execution_id}"),
                payload,
                now
            ],
        )
        .map_err(storage)?;
    Ok(true)
}

fn append_session_event(
    transaction: &Transaction<'_>,
    host: &DaemonHostRecord,
    now: i64,
    session_id: &DaemonSessionId,
    payload: &str,
) -> Result<Option<u64>, RuntimeStoreError> {
    let previous: Option<i64> = transaction
        .query_row(
            "SELECT last_event_sequence FROM session_runtimes WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    let Some(previous) = previous else {
        return Ok(None);
    };
    let sequence = u64_value(previous)
        .map_err(storage)?
        .checked_add(1)
        .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
    transaction
        .execute(
            "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,\
             emitted_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                session_id.as_str(),
                i64_value(sequence)?,
                host.generation,
                SESSION_EVENT_TYPE,
                payload,
                now
            ],
        )
        .map_err(storage)?;
    Ok(Some(sequence))
}

fn update_session_owner(
    transaction: &Transaction<'_>,
    host: &DaemonHostRecord,
    now: i64,
    session_id: &DaemonSessionId,
    sequence: u64,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE session_runtimes SET status='idle',owner_host_id=?1,owner_generation=?2,\
             last_event_sequence=?3,last_activity_at_ms=?4,updated_at_ms=?4 WHERE session_id=?5",
            params![
                host.host_id.as_str(),
                host.generation,
                i64_value(sequence)?,
                now,
                session_id.as_str()
            ],
        )
        .map_err(storage)?;
    Ok(())
}

fn uncertain_payload(execution_id: &str, goal_id: &str) -> String {
    serde_json::json!({
        "executionId": execution_id,
        "goalId": goal_id,
        "reason": "process_interrupted",
        "classification": "external_outcome_unknown",
    })
    .to_string()
}
