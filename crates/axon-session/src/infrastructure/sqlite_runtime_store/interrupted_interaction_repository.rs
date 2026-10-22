//! 进程中断后的 processing Interaction 收敛。
//!
//! 主 turn lease 失效说明原 worker 已无法证明存活。为避免同一条用户输入被
//! 静默重跑，必须在事务内收敛为显式终态，并留下 durable 中断证据。

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage, u64_value};
use crate::application::session_lane_id;
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{
    AudienceContext, DaemonSessionId, InteractionId, InteractionSource, JarvisEventId, ReplyRoute,
    ScopeRef,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

const INTERRUPTED_STATUS: &str = "interrupted";
const PROCESSING_STATUS: &str = "processing";
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
            let Some(sequence) =
                append_session_event(&transaction, host, now, &session_id, &payload)?
            else {
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
    Ok(records)
}

fn read_candidates(
    transaction: &Transaction<'_>,
    now: i64,
    limit: u32,
) -> Result<Vec<(InteractionId, Option<DaemonSessionId>)>, RuntimeStoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT interaction_id,scope_json,audience_json,source_channel,source_message_id,\
                    reply_route_json \
             FROM jarvis_interactions \
             WHERE status=?1 AND lease_expires_at_ms<=?2 \
             ORDER BY lease_expires_at_ms ASC, received_at_ms ASC, interaction_id ASC LIMIT ?3",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map(
            params![PROCESSING_STATUS, now, i64::from(limit)],
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
    let interaction_id =
        InteractionId::new(row.get::<_, String>(0)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let session_id = candidate_session_id(row).ok();
    Ok((interaction_id, session_id))
}

fn candidate_session_id(row: &rusqlite::Row<'_>) -> Result<DaemonSessionId, String> {
    let scope_json: String = row.get(1).map_err(|error| error.to_string())?;
    let audience_json: String = row.get(2).map_err(|error| error.to_string())?;
    let source_channel: String = row.get(3).map_err(|error| error.to_string())?;
    let source_message_id: String = row.get(4).map_err(|error| error.to_string())?;
    let reply_route_json: String = row.get(5).map_err(|error| error.to_string())?;
    let scope = serde_json::from_str::<ScopeRef>(&scope_json).map_err(|error| error.to_string())?;
    let audience = serde_json::from_str::<AudienceContext>(&audience_json)
        .map_err(|error| error.to_string())?;
    let source = InteractionSource::new(source_channel, source_message_id)
        .map_err(|error| error.to_string())?;
    let reply_route =
        serde_json::from_str::<ReplyRoute>(&reply_route_json).map_err(|error| error.to_string())?;
    let workspace_id = scope
        .workspace_id()
        .ok_or_else(|| "未绑定 Workspace".to_owned())?;
    let lane_id = session_lane_id(
        workspace_id.as_str(),
        &audience,
        source.channel(),
        reply_route.destination(),
    );
    DaemonSessionId::new(lane_id).map_err(|error| error.to_string())
}

fn settle_interrupted(
    transaction: &Transaction<'_>,
    now: i64,
    interaction_id: &InteractionId,
) -> Result<bool, RuntimeStoreError> {
    let changed = transaction
        .execute(
            "UPDATE jarvis_interactions SET status=?1,lease_token=NULL,\
             lease_expires_at_ms=NULL,updated_at_ms=?2 \
             WHERE interaction_id=?3 AND status=?4",
            params![
                INTERRUPTED_STATUS,
                now,
                interaction_id.as_str(),
                PROCESSING_STATUS
            ],
        )
        .map_err(storage)?;
    if changed == 0 {
        return Ok(false);
    }
    let event_id = JarvisEventId::new(format!(
        "event-{LEDGER_EVENT_TYPE}-{}",
        interaction_id.as_str()
    ))
    .map_err(|error| RuntimeStoreError::InvalidId(error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,\
             cause_event_id,payload_json,occurred_at_ms) \
             VALUES(?1,(SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events),?2,?3,NULL,?4,?5)",
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

fn update_session_tail(
    transaction: &Transaction<'_>,
    now: i64,
    session_id: &DaemonSessionId,
    sequence: u64,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "UPDATE session_runtimes SET last_event_sequence=?1,last_activity_at_ms=?2,\
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

    #[test]
    fn interrupted_payload_carries_interaction_evidence() {
        let interaction_id = InteractionId::new("interaction-1").expect("Interaction ID");
        let payload = interrupted_payload(&interaction_id);
        assert!(payload.contains("\"interactionId\":\"interaction-1\""));
    }
}
