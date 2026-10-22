//! `jarvis_events` 的 SQLite repository。

use super::{SqliteRuntimeStore, helpers::i64_value};
use crate::ports::{LedgerEventStore, LedgerEventStoreError, RuntimeStore};
use axon_domain::{GoalId, JarvisEvent, JarvisEventId};
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};

impl LedgerEventStore for SqliteRuntimeStore {
    fn append_ledger_event(&self, event: &JarvisEvent) -> Result<bool, LedgerEventStoreError> {
        if event.event_type.trim().is_empty() || event.idempotency_key.trim().is_empty() {
            return Err(LedgerEventStoreError::Conflict(
                "Event envelope 无效".to_owned(),
            ));
        }
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let existing = transaction
            .query_row(
                "SELECT event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms FROM jarvis_events WHERE idempotency_key=?1",
                params![event.idempotency_key.as_str()],
                read_event,
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(existing) = existing {
            let is_match = if event.sequence == 0 {
                existing.event_id == event.event_id
                    && existing.event_type == event.event_type
                    && existing.payload_json == event.payload_json
            } else {
                existing == *event
            };
            if is_match {
                return Ok(false);
            }
            return Err(LedgerEventStoreError::Conflict(
                "idempotency_key 已绑定不同 Event".to_owned(),
            ));
        }
        let expected_sequence: i64 = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let sequence_val = i64_value(event.sequence).map_err(storage_error)?;
        if sequence_val != 0 && sequence_val != expected_sequence {
            return Err(LedgerEventStoreError::Conflict(format!(
                "Event sequence 不连续，期望 {}，收到 {}",
                expected_sequence, event.sequence
            )));
        }
        transaction
            .execute(
                "INSERT INTO jarvis_events(event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    event.event_id.as_str(),
                    expected_sequence,
                    event.event_type,
                    event.idempotency_key,
                    event.cause_event_id.as_ref().map(ToString::to_string),
                    event.payload_json,
                    i64_value(event.occurred_at_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(true)
    }

    fn replay_ledger_events(
        &self,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<JarvisEvent>, LedgerEventStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(
                "SELECT event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms FROM jarvis_events WHERE sequence>?1 ORDER BY sequence ASC LIMIT ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    i64_value(after_sequence).map_err(storage_error)?,
                    i64::from(limit)
                ],
                read_event,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn list_events_for_goal(
        &self,
        goal_id: &GoalId,
        limit: u32,
    ) -> Result<Vec<JarvisEvent>, LedgerEventStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(
                "SELECT event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms FROM jarvis_events WHERE COALESCE(json_extract(payload_json,'$.goal_id'),json_extract(payload_json,'$.goalId'))=?1 ORDER BY sequence DESC LIMIT ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(params![goal_id.as_str(), i64::from(limit)], read_event)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }
}

fn read_event(row: &Row<'_>) -> rusqlite::Result<JarvisEvent> {
    Ok(JarvisEvent {
        event_id: JarvisEventId::new(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        sequence: u64::try_from(row.get::<_, i64>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        event_type: row.get(2)?,
        idempotency_key: row.get(3)?,
        cause_event_id: row
            .get::<_, Option<String>>(4)?
            .map(|value| JarvisEventId::new(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        payload_json: row.get(5)?,
        occurred_at_ms: u64::try_from(row.get::<_, i64>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    })
}

fn storage_error(error: impl std::fmt::Display) -> LedgerEventStoreError {
    LedgerEventStoreError::Storage(error.to_string())
}
