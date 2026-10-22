//! Session Event repository 边界。
use super::SqliteRuntimeStore;
use super::helpers::{i64_value, parse_session_id, storage, u64_value};
use crate::domain::runtime::{DaemonHostRecord, SessionEventRecord};
use crate::ports::{RuntimeStore, RuntimeStoreError};
use axon_domain::{DaemonSessionId, InteractionId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
/// Event cursor repository 视图。
#[derive(Debug, Clone, Copy)]
pub struct EventRepository<'a, S> {
    /// 底层运行时事实源。
    pub store: &'a S,
}
impl<'a, S: RuntimeStore> EventRepository<'a, S> {
    /// 创建 repository 视图。
    #[must_use]
    pub fn new(store: &'a S) -> Self {
        Self { store }
    }
    /// 读取 cursor 后的事件。
    pub fn replay(
        &self,
        session_id: &DaemonSessionId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<crate::domain::SessionEventRecord>, RuntimeStoreError> {
        self.store.replay_events(session_id, after, limit)
    }
}

/// 追加 Session 事件并推进事件序号。
pub(super) fn append_event(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    event_type: &str,
    payload_json: &str,
    now_ms: u64,
) -> Result<SessionEventRecord, RuntimeStoreError> {
    let event = (event_type, payload_json);
    let mut records = append_event_refs(
        store,
        session_id,
        host,
        std::iter::once(event),
        now_ms,
        None,
    )?;
    records
        .pop()
        .ok_or_else(|| RuntimeStoreError::Storage("单事件批次未返回持久化记录".to_owned()))
}

/// 在同一事务中按输入顺序追加一批 Session 事件。
pub(super) fn append_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    append_event_refs(
        store,
        session_id,
        host,
        events
            .iter()
            .map(|(event_type, payload_json)| (event_type.as_str(), payload_json.as_str())),
        now_ms,
        None,
    )
}

pub(super) fn append_interaction_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    interaction_id: &InteractionId,
    lease_token: &str,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    append_event_refs(
        store,
        session_id,
        host,
        events
            .iter()
            .map(|(event_type, payload_json)| (event_type.as_str(), payload_json.as_str())),
        now_ms,
        Some((interaction_id, lease_token)),
    )
}

fn append_event_refs<'a>(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: impl ExactSizeIterator<Item = (&'a str, &'a str)>,
    now_ms: u64,
    interaction_lease: Option<(&InteractionId, &str)>,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    if let Some((interaction_id, lease_token)) = interaction_lease {
        let active = transaction
            .query_row(
                "SELECT 1 FROM jarvis_interactions WHERE interaction_id=?1 AND status='processing' AND lease_token=?2 AND lease_expires_at_ms>?3",
                params![interaction_id.as_str(), lease_token, i64_value(now_ms)?],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage)?;
        if active.is_none() {
            return Err(RuntimeStoreError::Fenced);
        }
    }
    let previous = transaction
        .query_row(
            "SELECT last_event_sequence FROM session_runtimes WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| RuntimeStoreError::NotFound(session_id.to_string()))?;
    let mut next_sequence = u64::try_from(previous)
        .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?;
    let emitted_at_ms = i64_value(now_ms)?;
    let mut records = Vec::with_capacity(events.len());
    {
        let mut statement = transaction
            .prepare_cached(
                "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",
            )
            .map_err(storage)?;
        for (event_type, payload_json) in events {
            next_sequence = next_sequence
                .checked_add(1)
                .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
            statement
                .execute(params![
                    session_id.as_str(),
                    i64_value(next_sequence)?,
                    host.generation,
                    event_type,
                    payload_json,
                    emitted_at_ms
                ])
                .map_err(storage)?;
            records.push(SessionEventRecord {
                session_id: session_id.clone(),
                sequence: next_sequence,
                generation: host.generation.clone(),
                event_type: event_type.to_owned(),
                payload_json: payload_json.to_owned(),
                emitted_at_ms: now_ms,
            });
        }
    }
    transaction
        .execute(
            "UPDATE session_runtimes SET last_event_sequence=?1,last_activity_at_ms=?2,owner_host_id=?3,owner_generation=?4 WHERE session_id=?5",
            params![
                i64_value(next_sequence)?,
                emitted_at_ms,
                host.host_id.as_str(),
                host.generation,
                session_id.as_str()
            ],
        )
        .map_err(storage)?;
    transaction.commit().map_err(storage)?;
    Ok(records)
}

/// 读取 cursor 之后按序号升序排列的 Session 事件。
pub(super) fn replay_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    after_sequence: u64,
    limit: u32,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let connection = store.connection()?;
    let mut statement = connection.prepare("SELECT session_id,sequence,generation,event_type,payload_json,emitted_at_ms FROM session_events WHERE session_id=?1 AND sequence>?2 ORDER BY sequence ASC LIMIT ?3").map_err(storage)?;
    let rows = statement
        .query_map(
            params![
                session_id.as_str(),
                i64_value(after_sequence)?,
                i64::from(limit)
            ],
            |row| {
                Ok(SessionEventRecord {
                    session_id: parse_session_id(row.get(0)?)?,
                    sequence: u64_value(row.get(1)?)?,
                    generation: row.get(2)?,
                    event_type: row.get(3)?,
                    payload_json: row.get(4)?,
                    emitted_at_ms: u64_value(row.get(5)?)?,
                })
            },
        )
        .map_err(storage)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(storage)
}
