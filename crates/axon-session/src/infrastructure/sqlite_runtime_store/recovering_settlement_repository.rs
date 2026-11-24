//! `recovering` 会话的收敛：这个状态必须有归宿（INV-7）。
//!
//! `recover_expired_sessions` 把失控会话写成 `recovering` 之后，旧实现只取它的条数：
//! 既没有把会话送回 `idle`，也没有重新派发。`recovering` 于是在库里长期停留，
//! 会话清理把它当成「仍在活动」，读侧也拿不到任何收口事实。本模块挂在 supervisor 的
//! 维护路径上，补齐两条收敛规则：
//!
//! 1. 没有在途工作项（`queued`/`running`），或它们已全部落定终态 → 会话回 `idle`；
//! 2. 停留超过 TTL、工作项仍停在排队态（从未派发）→ 工作项收口为 `interrupted`
//!    并追加对应会话事件，会话随即回 `idle`。
//!
//! 运行中且租约仍然有效的工作项不动：那说明还有执行者，不属于「没有归宿」。
//! 这类行由 `quarantine_interrupted_executions` / `quarantine_interrupted_interactions`
//! 在租约过期后收口，收口完就落回本模块的第 1 条。

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, parse_session_id, storage, u64_value};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RecoveringSettlement, RuntimeStore, RuntimeStoreError};
use axon_domain::DaemonSessionId;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

/// 在途工作项的状态集合：与 `list_runnable_execution_sessions` 同一口径。
///
/// 只有这两态代表「还有人在等它跑完」。`uncertain` 不在其中：外部结果未知的执行
/// 不会再次派发，会话因此可以回 `idle`。
const IN_FLIGHT_WORK_STATUSES: &str = "'queued','running'";
/// 从未派发的排队态。
const QUEUED_WORK_STATUS: &str = "queued";
/// 会话的恢复中状态。
const RECOVERING: &str = "recovering";
/// 会话的空闲状态。
const IDLE: &str = "idle";
/// 归属明确的中断事件类型：复用「本轮被中断」这一个既有语义，实时桥据此收口。
const INTERACTION_INTERRUPTED_EVENT: &str = "interaction_interrupted";
/// 无归属工作项的中断事件类型：没有 Interaction 可归属时，仍要留下可读的中断证据。
const WORK_ITEM_INTERRUPTED_EVENT: &str = "work_item_interrupted";
/// 中断原因：停留在恢复态超过 TTL。
const RECOVERY_TTL_REASON: &str = "recovery_ttl_expired";
/// 中断分类：这一轮从未派发过，可以断定没有外部副作用。
const NEVER_DISPATCHED: &str = "never_dispatched";

/// 一条停在 `recovering` 的候选会话。
struct RecoveringSession {
    session_id: DaemonSessionId,
    last_activity_at_ms: u64,
}

/// 一条在途工作项。
struct InFlightWorkItem {
    execution_id: String,
    /// 归属的入站事实；目标执行与会话动作的工作项没有归属。
    inbox_id: Option<String>,
    /// 是否仍停在排队态（从未派发）。
    queued: bool,
}

/// 收敛一批停在 `recovering` 的会话，返回收敛条数与待追加的会话事件。
///
/// 控制面按会话各自一个写事务落定；事件在事务提交后由调用方写入事件面（与
/// `quarantine_interrupted_interactions` 同一顺序：先控制面，再日志）。
///
/// 单事务的批量形态会让持锁时间随批量线性增长，与每秒 tick 的节拍叠在一起时，逐帧追加
/// 与主轮收口都要排在它后面等同一个单写者锁。这里把「批量」只当调度窗口，不当锁窗口。
pub(super) fn settle_recovering_sessions(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    ttl_ms: u64,
    limit: u32,
) -> Result<RecoveringSettlement, RuntimeStoreError> {
    if limit == 0 {
        return Ok(RecoveringSettlement::default());
    }
    store.initialize()?;
    // 只读探路：fence 与候选一起在一个读事务里判定，没有候选就不开写事务（理由同中断收敛）。
    let candidates = {
        let mut connection = store.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage)?;
        SqliteRuntimeStore::check_fence(&transaction, host, now_ms)?;
        let candidates = read_candidates(&transaction, limit)?;
        transaction.commit().map_err(storage)?;
        candidates
    };
    let mut settlement = RecoveringSettlement::default();
    for session in candidates {
        /*
         * 锁序固定为「先会话锁、后数据库写事务」：水位推进与文件尾追加必须落在同一把
         * 会话锁里。连接在块内取还，落盘时不再占着池里的句柄。
         */
        let lock = store.event_log_write_lock(&session.session_id);
        let guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 每条会话一个事务：持锁时间只与这条会话自己的在途工作项有关。
        let mut settled = {
            let mut connection = store.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
            let settled = settle_one_session(&transaction, host, now_ms, ttl_ms, &session)?;
            transaction.commit().map_err(storage)?;
            settled
        };
        /*
         * 先控制面、再日志：落盘失败时如实留痕并停止本轮，已落成的前序会话照常返回，
         * 不用一个错误把整轮发布一起吞掉。复核口径见日志里的 session 与 sequence。
         */
        if let Err(error) =
            super::event_log_repository::write_records_locked(store, &mut settled.events, None)
        {
            axon_logger::error(&format!(
                "控制面已收口但证据帧未落盘（该会话缺证据帧）: session={} events={} error={error:?}",
                session.session_id.as_str(),
                settled.events.len()
            ));
            drop(guard);
            break;
        }
        drop(guard);
        settlement.settled_to_idle = settlement
            .settled_to_idle
            .saturating_add(settled.settled_to_idle);
        settlement.interrupted = settlement.interrupted.saturating_add(settled.interrupted);
        settlement.events.extend(settled.events);
    }
    Ok(settlement)
}

/// 收敛一条会话：调用方必须已经为这条会话开好写事务。
///
/// 内层只遍历「这一条会话自己的在途工作项」，量级由单条会话决定，与外层批量无关。
fn settle_one_session(
    transaction: &Transaction<'_>,
    host: &DaemonHostRecord,
    now_ms: u64,
    ttl_ms: u64,
    session: &RecoveringSession,
) -> Result<RecoveringSettlement, RuntimeStoreError> {
    let mut settlement = RecoveringSettlement::default();
    let now = i64_value(now_ms)?;
    let in_flight = read_in_flight_items(transaction, &session.session_id)?;
    // 规则一：没有在途工作项就没有可等待的执行，直接回 idle。
    if in_flight.is_empty() {
        settlement.settled_to_idle = settlement.settled_to_idle.saturating_add(reclaim_idle(
            transaction,
            now,
            &session.session_id,
        )?);
        return Ok(settlement);
    }
    // 还没到 TTL：给重新派发留出窗口。
    if session.last_activity_at_ms.saturating_add(ttl_ms) > now_ms {
        return Ok(settlement);
    }
    // 规则二：TTL 到期仍停在排队态的工作项从未派发，收口为中断。
    let mut settled_any = false;
    for item in in_flight.iter().filter(|item| item.queued) {
        let settled = super::interaction_repository::settle_undispatched_work_item(
            transaction,
            &item.execution_id,
            now,
        )
        .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?;
        if !settled {
            continue;
        }
        settled_any = true;
        settlement.interrupted = settlement.interrupted.saturating_add(1);
        let Some(sequence) = next_session_sequence(transaction, &session.session_id)? else {
            continue;
        };
        update_session_tail(transaction, now, &session.session_id, sequence)?;
        settlement.events.push(SessionEventRecord {
            session_id: session.session_id.clone(),
            sequence,
            generation: host.generation.clone(),
            event_type: interrupted_event_type(item).to_owned(),
            payload_json: interrupted_payload(item),
            emitted_at_ms: now_ms,
        });
    }
    if settled_any {
        // 收口后本会话可能已经腾空：同一条带守卫的收敛把状态也落定。
        settlement.settled_to_idle = settlement.settled_to_idle.saturating_add(reclaim_idle(
            transaction,
            now,
            &session.session_id,
        )?);
    }
    Ok(settlement)
}

/// 读一批候选会话：最久没有活动的先收敛，避免积压把新会话挤在窗口外。
fn read_candidates(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<RecoveringSession>, RuntimeStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT session_id,last_activity_at_ms FROM sessions \
             WHERE status=?1 ORDER BY last_activity_at_ms ASC,session_id ASC LIMIT ?2",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map(params![RECOVERING, i64::from(limit)], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(storage)?;
    let mut candidates = Vec::new();
    for row in rows {
        let (raw_id, last_activity_at_ms) = row.map_err(storage)?;
        let session_id = parse_session_id(raw_id)
            .map_err(|_| RuntimeStoreError::Storage("会话标识无效".to_owned()))?;
        candidates.push(RecoveringSession {
            session_id,
            last_activity_at_ms: u64_value(last_activity_at_ms).map_err(storage)?,
        });
    }
    Ok(candidates)
}

/// 读该会话的在途工作项：排队与运行中的都读，由调用方按状态分流。
fn read_in_flight_items(
    transaction: &Transaction<'_>,
    session_id: &DaemonSessionId,
) -> Result<Vec<InFlightWorkItem>, RuntimeStoreError> {
    let sql = format!(
        "SELECT execution_id,inbox_id,status FROM work_items \
         WHERE session_id=?1 AND status IN ({IN_FLIGHT_WORK_STATUSES}) \
         ORDER BY created_at_ms ASC,execution_id ASC"
    );
    let mut statement = transaction.prepare(&sql).map_err(storage)?;
    let rows = statement
        .query_map(params![session_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage)?;
    let mut items = Vec::new();
    for row in rows {
        let (execution_id, inbox_id, status) = row.map_err(storage)?;
        items.push(InFlightWorkItem {
            execution_id,
            inbox_id,
            queued: status == QUEUED_WORK_STATUS,
        });
    }
    Ok(items)
}

/// 把会话送回 `idle`，守卫是「这一刻真的没有在途工作项」。
///
/// 守卫写在 UPDATE 里而不是先读后写：同一事务里刚收口的行与别处新派发的行都不会
/// 被误判，收敛结论与状态更新永远自洽。返回是否真的改了这一行。
fn reclaim_idle(
    transaction: &Transaction<'_>,
    now: i64,
    session_id: &DaemonSessionId,
) -> Result<u32, RuntimeStoreError> {
    let sql = format!(
        "UPDATE sessions SET status=?1,updated_at_ms=?2 \
         WHERE session_id=?3 AND status=?4 \
           AND NOT EXISTS(SELECT 1 FROM work_items WHERE session_id=?3 \
                          AND status IN ({IN_FLIGHT_WORK_STATUSES}))"
    );
    let changed = transaction
        .execute(&sql, params![IDLE, now, session_id.as_str(), RECOVERING])
        .map_err(storage)?;
    Ok(u32::try_from(changed).unwrap_or(u32::MAX))
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
    Ok(Some(sequence))
}

/// 推进会话事件水位与活动时间：中断证据帧的序号必须与控制面一致。
fn update_session_tail(
    transaction: &Transaction<'_>,
    now: i64,
    session_id: &DaemonSessionId,
    sequence: u64,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE sessions SET last_event_sequence=?1,last_activity_at_ms=?2,\
             updated_at_ms=?2 WHERE session_id=?3",
            params![i64_value(sequence)?, now, session_id.as_str()],
        )
        .map_err(storage)?;
    Ok(())
}

/// 中断事件类型：有归属就是这一轮的中断，没有归属就是这一条工作项的中断。
fn interrupted_event_type(item: &InFlightWorkItem) -> &'static str {
    if item.inbox_id.is_some() {
        INTERACTION_INTERRUPTED_EVENT
    } else {
        WORK_ITEM_INTERRUPTED_EVENT
    }
}

/// 中断证据载荷：归属列与载荷里的标识必须一致，尾窗才读得到这一条。
fn interrupted_payload(item: &InFlightWorkItem) -> String {
    let mut payload = serde_json::Map::new();
    match item.inbox_id.as_deref() {
        Some(interaction_id) => {
            payload.insert(
                "interactionId".to_owned(),
                serde_json::Value::String(interaction_id.to_owned()),
            );
        }
        None => {
            payload.insert(
                "executionId".to_owned(),
                serde_json::Value::String(item.execution_id.clone()),
            );
        }
    }
    payload.insert(
        "reason".to_owned(),
        serde_json::Value::String(RECOVERY_TTL_REASON.to_owned()),
    );
    payload.insert(
        "classification".to_owned(),
        serde_json::Value::String(NEVER_DISPATCHED.to_owned()),
    );
    serde_json::Value::Object(payload).to_string()
}
