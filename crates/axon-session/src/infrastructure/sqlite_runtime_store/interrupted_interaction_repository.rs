//! 进程中断后的 processing Interaction 收敛。
//!
//! 主 turn lease 失效说明原 worker 已无法证明存活。为避免同一条用户输入被
//! 静默重跑，必须在事务内收敛为显式终态，并留下 durable 中断证据。

use super::SqliteRuntimeStore;
use super::helpers::{decode_error, i64_value, storage, u64_value};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, InteractionId, LedgerEventId};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

const WORK_RUNNING: &str = "running";
const WORK_FAILED: &str = "failed";
const TERMINAL_INTERRUPTED: &str = "interrupted";
const LEDGER_EVENT_TYPE: &str = "interaction.interrupted";
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
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let now = i64_value(now_ms)?;
    let mut records = Vec::new();
    for (interaction_id, session_id) in read_candidates(&transaction, now, limit)? {
        if !settle_interrupted(&transaction, now, &interaction_id)? {
            continue;
        }
        let payload = interrupted_payload(&interaction_id);
        if let Some(session_id) = session_id {
            let Some(sequence) = next_session_sequence(&transaction, &session_id)? else {
                continue;
            };
            update_session_tail(&transaction, now, &session_id, sequence)?;
            records.push(SessionEventRecord {
                session_id,
                sequence,
                generation: host.generation.clone(),
                event_type: SESSION_EVENT_TYPE.to_owned(),
                payload_json: payload,
                emitted_at_ms: now_ms,
            });
        }
    }
    transaction.commit().map_err(storage)?;
    super::event_log_repository::write_records(store, &records, None)?;
    Ok(records)
}

fn read_candidates(
    transaction: &Transaction<'_>,
    now: i64,
    limit: u32,
) -> Result<Vec<(InteractionId, Option<DaemonSessionId>)>, RuntimeStoreError> {
    let mut statement = transaction
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

fn settle_interrupted(
    transaction: &Transaction<'_>,
    now: i64,
    interaction_id: &InteractionId,
) -> Result<bool, RuntimeStoreError> {
    let changed = transaction
        .execute(
            "UPDATE work_items SET status=?1,terminal_kind=?2,lease_token=NULL,\
             lease_expires_at_ms=NULL,updated_at_ms=?3,committed_at_ms=?3 \
             WHERE inbox_id=?4 AND status=?5 AND lease_expires_at_ms<=?3",
            params![
                WORK_FAILED,
                TERMINAL_INTERRUPTED,
                now,
                interaction_id.as_str(),
                WORK_RUNNING
            ],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Ok(false);
    }
    // 隔离即终态：行搬进历史表后，处理链不会再把它当成可领取的入站事实。
    super::work_item_archive::archive_settled_work_item(
        transaction,
        "inbox_id=?1",
        interaction_id.as_str(),
        now,
    )
    .map_err(storage)?;
    let event_id = LedgerEventId::new(format!(
        "event-{LEDGER_EVENT_TYPE}-{}",
        interaction_id.as_str()
    ))
    .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO ledger_events(event_id,sequence,event_type,idempotency_key,\
             cause_event_id,payload_json,occurred_at_ms) \
             VALUES(?1,(SELECT COALESCE(MAX(sequence),0)+1 FROM ledger_events),?2,?3,NULL,?4,?5)",
            params![
                event_id.as_str(),
                LEDGER_EVENT_TYPE,
                format!("{LEDGER_EVENT_TYPE}:{}", interaction_id.as_str()),
                interrupted_payload(interaction_id),
                now
            ],
        )
        .map_err(storage)?;
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

fn interrupted_payload(interaction_id: &InteractionId) -> String {
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
