//! 进程中断后的 processing Interaction 收敛。
//!
//! 主 turn lease 失效说明原 worker 已无法证明存活。为避免同一条用户输入被
//! 静默重跑，必须在事务内收敛为显式终态，并留下 durable 中断证据。

use super::SqliteRuntimeStore;
use super::decode::decode_error;
use super::helpers::{i64_value, storage, u64_value};
use super::work_item_settlement::{
    LedgerTrace, Settlement, TERMINAL_INTERRUPTED, WorkItemLocator, settle_work_item,
};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, ExecutionStatus, InteractionId};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

const WORK_RUNNING: &str = "running";
/// 既有流水账事件名：升级期核对脚本依赖它，不得改名。
pub(super) const LEDGER_EVENT_TYPE: &str = "interaction.interrupted";
const SESSION_EVENT_TYPE: &str = "interaction_interrupted";
pub(super) fn quarantine_interrupted_interactions(
    store: &SqliteRuntimeStore,
    host: &DaemonHostRecord,
    now_ms: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    store.initialize()?;
    let now = i64_value(now_ms)?;
    /*
     * 只读探路：fence 与候选一起在一个读事务里判定，没有候选就不开写事务。
     *
     * 这条路径挂在每秒一次的 supervisor tick 上，而运行态库是单写者：集合为空也去拿一次
     * 写锁，等于每秒白占一次别的写者（逐帧追加、主轮收口）必须排队的时机。fence 走
     * `check_fence`（只读口径，判据相同）：空集合下租约失效仍按 fail-closed 上报。
     */
    let candidates = {
        let mut connection = store.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage)?;
        SqliteRuntimeStore::check_fence(&transaction, host, now_ms)?;
        let candidates = read_candidates(&transaction, now, limit)?;
        transaction.commit().map_err(storage)?;
        candidates
    };
    let mut records = Vec::new();
    for (interaction_id, session_id) in candidates {
        /*
         * 每条一个事务：持锁时间只与这一条的语句数有关，不随批量线性增长。
         *
         * 探路与写入之间可能有别的收敛方先落定同一条，因此终态判定仍以事务内的守卫结果
         * 为准（`settle_interrupted` 返回它是否真的改了这一行）：重复收敛只会空转一次。
         */
        /*
         * 锁序固定为「先会话锁、后数据库写事务」：水位推进与文件尾追加必须落在同一把
         * 会话锁里，否则与逐帧追加交错就会写出乱序尾并触发「序号与文件尾不符」的重排。
         * 没有会话行就没有日志可写，那一支不需要锁。
         */
        let session_lock = session_id
            .as_ref()
            .map(|session_id| store.event_log_write_lock(session_id));
        let _session_guard = session_lock.as_ref().map(|lock| {
            lock.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        let mut connection = store.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
        /*
         * 事务内重校验租约：探路是在读事务里做的，它与这一次写入之间可能有别的执行者刚
         * 完成续租。不重校验就会把一条仍在跑的轮次收口成中断（`settle_interrupted` 的
         * 守卫只看 `status='running'`，不看租约是否真的过期）。
         */
        if !lease_expired(&transaction, &interaction_id, now)? {
            transaction.commit().map_err(storage)?;
            continue;
        }
        if !settle_interrupted(&transaction, now, &interaction_id)? {
            transaction.commit().map_err(storage)?;
            continue;
        }
        let event = match session_id {
            Some(session_id) => match next_session_sequence(&transaction, &session_id)? {
                Some(sequence) => {
                    update_session_tail(&transaction, now, &session_id, sequence)?;
                    Some(SessionEventRecord {
                        session_id,
                        sequence,
                        generation: host.generation.clone(),
                        event_type: SESSION_EVENT_TYPE.to_owned(),
                        payload_json: interrupted_payload(&interaction_id),
                        emitted_at_ms: now_ms,
                    })
                }
                // 会话行不存在：控制面终态照样已落定，只是没有可归属的事件日志。
                None => None,
            },
            None => None,
        };
        transaction.commit().map_err(storage)?;
        // 连接在落盘之前还池：补偿分支会自己再取连接，这里不必占着句柄。
        drop(connection);
        /*
         * 控制面已经落定，证据帧必须紧跟着写进事件面。
         *
         * 逐条摊销而不是攒到批末：中途某条失败时，前面已经提交的收敛不能因此丢掉证据帧
         * （否则那一轮在界面上就是「收口了但没有过程」）。
         */
        if let Some(mut record) = event {
            match super::event_log_repository::write_records_locked(
                store,
                std::slice::from_mut(&mut record),
                None,
            ) {
                Ok(()) => records.push(record),
                Err(error) => {
                    /*
                     * 控制面已收口、证据帧没落成：如实留痕并停在这一条。
                     *
                     * 已落成的前序记录照常返回，让调用方把它们发布出去，不用一个错误把
                     * 整轮的实时发布一起吞掉。复核口径：按 session_id 与 sequence 对照
                     * 事件面文件里的 `interaction_interrupted` 帧。
                     */
                    axon_logger::error(&format!(
                        "控制面已收口但证据帧未落盘（该轮缺证据帧）: session={} sequence={} error={error:?}",
                        record.session_id.as_str(),
                        record.sequence
                    ));
                    break;
                }
            }
        }
    }
    Ok(records)
}

fn read_candidates(
    connection: &Connection,
    now: i64,
    limit: u32,
) -> Result<Vec<(InteractionId, Option<DaemonSessionId>)>, RuntimeStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT i.interaction_id,i.scope_json,i.audience_json,i.source_channel,i.source_message_id,\
                    i.reply_route_json,i.session_id \
             FROM work_items AS w JOIN inbox AS i ON i.interaction_id=w.inbox_id \
             WHERE w.status=?1 AND w.lease_expires_at_ms<=?2 \
             ORDER BY w.lease_expires_at_ms ASC, i.received_at_ms ASC, i.interaction_id ASC LIMIT ?3",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map(
            params![WORK_RUNNING, now, i64::from(limit)],
            read_candidate_row,
        )
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    Ok(rows)
}

fn read_candidate_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(InteractionId, Option<DaemonSessionId>)> {
    let raw_interaction_id = row.get::<_, String>(0)?;
    let interaction_id = InteractionId::new(raw_interaction_id.clone())
        .map_err(|_| decode_error("inbox.interaction_id", &raw_interaction_id))?;
    let session_id = candidate_session_id(row).ok();
    Ok((interaction_id, session_id))
}

fn candidate_session_id(row: &rusqlite::Row<'_>) -> Result<DaemonSessionId, String> {
    let session_id: String = row.get(6).map_err(|error| error.to_string())?;
    DaemonSessionId::new(session_id).map_err(|error| error.to_string())
}

/// 该轮次的工作项是否确实已经租约过期。
///
/// `settle_interrupted` 的守卫只认 `status='running'`，不含租约判定。探路搬到读事务之后，
/// 探路与写入之间可能有别的执行者刚完成续租，因此必须在写事务里补一次到期复核。
fn lease_expired(
    transaction: &Transaction<'_>,
    interaction_id: &InteractionId,
    now: i64,
) -> Result<bool, RuntimeStoreError> {
    let expired = transaction
        .query_row(
            "SELECT 1 FROM work_items WHERE inbox_id=?1 AND status=?2 AND lease_expires_at_ms<=?3",
            params![interaction_id.as_str(), WORK_RUNNING, now],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage)?;
    Ok(expired.is_some())
}

fn settle_interrupted(
    transaction: &Transaction<'_>,
    now: i64,
    interaction_id: &InteractionId,
) -> Result<bool, RuntimeStoreError> {
    // 隔离即终态：四件事（状态、终态类别、归档、流水账）由存储层的唯一出口一次做完。
    let payload = interrupted_payload(interaction_id);
    let settlement = Settlement::concluded(ExecutionStatus::Failed, TERMINAL_INTERRUPTED)
        .with_ledger(LedgerTrace {
            event_type: LEDGER_EVENT_TYPE,
            subject_id: interaction_id.as_str(),
            payload: &payload,
        });
    settle_work_item(
        transaction,
        WorkItemLocator::RunningInbox(interaction_id.as_str()),
        &settlement,
        now,
    )
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
    /*
     * 中断证据帧属于目标 Interaction：归属列必须与载荷里的 interactionId 一致。
     *
     * 尾窗读取按 interaction_id 列过滤，列留空就只剩「已中断」状态，
     * 中断原因与分类永远读不出来。
     */
    Ok(Some(sequence))
}

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

/// 中断证据载荷：显式收口与被动隔离共用同一份事实形状。
pub(super) fn interrupted_payload(interaction_id: &InteractionId) -> String {
    serde_json::json!({
        "interactionId": interaction_id.as_str(),
        "reason": "process_interrupted",
        "classification": "turn_outcome_unknown",
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InteractionStore;
    use crate::ports::EventLogStore as _;
    use axon_domain::RuntimeHostId;

    #[test]
    fn interrupted_payload_carries_interaction_evidence() {
        let interaction_id = InteractionId::new("interaction-1").expect("Interaction ID");
        let payload = interrupted_payload(&interaction_id);
        assert!(payload.contains("\"interactionId\":\"interaction-1\""));
    }

    #[test]
    #[ignore = "旧表列语义已废弃，集中清理阶段改写"]
    fn interrupted_session_event_writes_interaction_id_column() {
        let directory = axon_test_support::test_temp_dir();
        let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
        let host = store
            .claim_host(
                &RuntimeHostId::new("interrupted-column-host").expect("host"),
                1,
                None,
                1,
                1_000,
            )
            .expect("host lease");
        let interaction =
            crate::infrastructure::sqlite_runtime_store_interaction_tests::sample_interaction();
        store.append(&interaction).expect("追加 Interaction");
        let session_id = crate::interaction_session_id(&interaction);
        let session_id = DaemonSessionId::new(session_id).expect("Session ID");
        store.create_session(&session_id, ".", 1).expect("Session");
        store
            .claim_processing_by_id(&interaction.interaction_id, 20, 100)
            .expect("领取")
            .expect("lease");

        let records = quarantine_interrupted_interactions(&store, &host, 121, 10)
            .expect("收敛中断 Interaction");
        assert_eq!(records.len(), 1);
        let column: String = store
            .connection()
            .expect("connection")
            .query_row(
                "SELECT interaction_id FROM session_events WHERE event_type=?1",
                [SESSION_EVENT_TYPE],
                |row| row.get(0),
            )
            .expect("归属列");
        assert_eq!(column, interaction.interaction_id.as_str());
        // 尾窗按同一列过滤：列写对了，中断证据才会出现在该交互自身的历史里。
        let window = store
            .interaction_tail_window(&session_id, &interaction.interaction_id, 10)
            .expect("尾窗读取");
        assert_eq!(window.len(), 1);
        assert_eq!(window[0].event_type, SESSION_EVENT_TYPE);
    }
}
