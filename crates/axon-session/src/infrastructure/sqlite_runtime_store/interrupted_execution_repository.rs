//! 进程中断后的 running Execution 隔离。
//!
//! 持有 lease 的进程消失后，执行可能已经发出外部副作用请求。
//! 目标态不允许盲目重跑，必须在单个事务内把 Execution 收敛为 `uncertain`：
//! 释放 lease、提交 uncertain Result、把 Goal 置为 `waiting` 并追加幂等事件。

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage, u64_value};
use super::work_item_settlement::{LedgerTrace, Settlement, WorkItemLocator, settle_work_item};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, ExecutionStatus};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

const UNCERTAIN_ERROR_KIND: &str = "uncertain";
const WAITING_GOAL_STATUS: &str = "waiting";
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
    for candidate in read_candidates(&transaction, now, limit, selector)? {
        let payload = uncertain_payload(&candidate.execution_id, candidate.goal_id.as_deref());
        if !settle_uncertain(
            &transaction,
            now,
            &candidate.execution_id,
            candidate.goal_id.as_deref(),
            &payload,
        )? {
            continue;
        }
        let Some(session_id) = candidate.session_id else {
            continue;
        };
        let Ok(session_id) = DaemonSessionId::new(session_id) else {
            continue;
        };
        let Some(sequence) = next_session_sequence(&transaction, &session_id)? else {
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
    super::event_log_repository::write_records(store, &records, None)?;
    Ok(records)
}

/// 一条待隔离的 running 执行行；`goal_id` 为空表示它没有所属目标。
struct InterruptedCandidate {
    execution_id: String,
    goal_id: Option<String>,
    session_id: Option<String>,
}

fn read_candidates(
    transaction: &Transaction<'_>,
    now: i64,
    limit: u32,
    selector: CandidateSelector<'_>,
) -> Result<Vec<InterruptedCandidate>, RuntimeStoreError> {
    let (sql, owner_host_id, owner_generation) = match selector {
        CandidateSelector::ExpiredLease => (
            "SELECT execution_id,goal_id,session_id FROM work_items \
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
            "SELECT execution_id,goal_id,session_id FROM work_items \
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

fn read_candidate_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InterruptedCandidate> {
    // `work_items.goal_id` 对会话动作恒为空（入站领取写的就是 NULL），按非空字符串
    // 读会让 supervisor 的隔离扫描每次都在这里失败，整个 tick 一并停摆。
    Ok(InterruptedCandidate {
        execution_id: row.get(0)?,
        goal_id: row.get::<_, Option<String>>(1)?,
        session_id: row.get(2)?,
    })
}

/// 把一条 running Execution 收敛为 `uncertain`；已被其他事务处理时返回 false。
///
/// `goal_id` 为 `None` 表示这条执行没有所属目标（会话动作）：结果账本与目标状态都
/// 没有可写入的归属，只留状态收敛与 ledger 事件这两处真实事实。
///
/// 状态、终态类别、归档与流水账由存储层的唯一出口一次做完：`terminal_kind` 必须与
/// `status` 同时落定，否则这一轮既不在在途表里可领，又在派生列里读不出结论。
fn settle_uncertain(
    transaction: &Transaction<'_>,
    now: i64,
    execution_id: &str,
    goal_id: Option<&str>,
    payload: &str,
) -> Result<bool, RuntimeStoreError> {
    let settlement = Settlement::concluded(
        ExecutionStatus::Uncertain,
        super::work_item_settlement::TERMINAL_INTERRUPTED,
    )
    .with_ledger(LedgerTrace {
        event_type: LEDGER_EVENT_TYPE,
        subject_id: execution_id,
        payload,
    });
    if !settle_work_item(
        transaction,
        WorkItemLocator::RunningExecution(execution_id),
        &settlement,
        now,
    )? {
        return Ok(false);
    }
    if let Some(goal_id) = goal_id {
        transaction
            .execute(
                "INSERT INTO goal_results(result_id,goal_id,execution_id,succeeded,summary,\
                 receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) \
                 SELECT ?1,?2,?3,0,?4,'[]','[]',?5,?6 \
                 WHERE NOT EXISTS(SELECT 1 FROM goal_results WHERE execution_id=?3)",
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
                "UPDATE goals SET status=?1,revision=revision+1,updated_at_ms=?2 \
                 WHERE goal_id=?3 AND status IN ('proposed','active')",
                params![WAITING_GOAL_STATUS, now, goal_id],
            )
            .map_err(storage)?;
    }
    Ok(true)
}

/// 取该会话的下一个事件序号；会话不存在时返回 None。
fn next_session_sequence(
    transaction: &Transaction<'_>,
    session_id: &DaemonSessionId,
) -> Result<Option<u64>, RuntimeStoreError> {
    let previous: Option<i64> = transaction
        .query_row(
            "SELECT last_event_sequence FROM sessions WHERE session_id=?1",
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
    // 会话级事件，不归属任何 Interaction，故 interaction_id 留空。
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
            "UPDATE sessions SET status='idle',owner_host_id=?1,owner_generation=?2,\
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

fn uncertain_payload(execution_id: &str, goal_id: Option<&str>) -> String {
    serde_json::json!({
        "executionId": execution_id,
        "goalId": goal_id,
        "reason": "process_interrupted",
        "classification": "external_outcome_unknown",
    })
    .to_string()
}
