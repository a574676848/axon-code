use axon_session::ThreadCompaction;
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    CURRENT_SESSION_SINGLETON_KEY, RolloutEventRecord, RolloutEventType, RolloutHistoryWindow,
    RolloutSqliteStore, RunSessionRecord, events_for_session,
};

#[cfg(test)]
use super::RolloutEventAppendInput;

const ROLLOUT_EVENTS_PER_TURN: u64 = 32;
const ROLLOUT_EXTERNAL_EVENT_SEQUENCE_BASE: u32 = 1 << 30;

impl RolloutSqliteStore {
    pub(super) fn commit_session(
        &self,
        record: &RunSessionRecord,
        title: Option<&str>,
    ) -> Result<(), String> {
        let record_json = serialize(record, "会话")?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| "rollout SQLite 事务启动失败".to_owned())?;
        upsert_session(&transaction, record, title)?;
        if record.parent_session_id.is_none() {
            select_current_session(&transaction, &record.session_id)?;
        }
        let turn_sequence = upsert_turn(&transaction, record, &record_json)?;
        replace_turn_events(&transaction, record, turn_sequence)?;
        transaction
            .commit()
            .map_err(|_| "rollout SQLite 事务提交失败".to_owned())
    }

    pub(super) fn install_compaction(
        &self,
        session_id: &str,
        compaction: &ThreadCompaction,
    ) -> Result<(), String> {
        self.replace_compaction(session_id, Some(compaction))
    }

    pub(super) fn install_active_window(
        &self,
        session_id: &str,
        assistant_part_id: &str,
        mut compaction: ThreadCompaction,
    ) -> Result<(ThreadCompaction, Option<ThreadCompaction>), String> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| "活动窗口事务启动失败".to_owned())?;
        let previous_json = transaction
            .query_row(
                "SELECT history_checkpoint_json FROM rollout_sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|_| "读取活动窗口回滚投影失败".to_owned())?;
        let previous = previous_json
            .map(|json| deserialize::<ThreadCompaction>(&json, "活动窗口回滚投影"))
            .transpose()?;
        let sequence = transaction.query_row(
            "SELECT sequence FROM rollout_turns WHERE session_id = ?1 AND assistant_part_id = ?2",
            params![session_id, assistant_part_id],
            |row| row.get::<_, i64>(0),
        ).map_err(|_| "活动窗口缺少当前运行轮次".to_owned())?;
        compaction.through_sequence =
            u64::try_from(sequence).map_err(|_| "活动窗口轮次不能为负数".to_owned())?;
        let json = serialize(&compaction, "活动窗口 checkpoint")?;
        transaction
            .execute(
                "UPDATE rollout_sessions SET history_checkpoint_json = ?2 WHERE session_id = ?1",
                params![session_id, json],
            )
            .map_err(|_| "活动窗口 checkpoint 写入失败".to_owned())?;
        transaction
            .commit()
            .map_err(|_| "活动窗口事务提交失败".to_owned())?;
        Ok((compaction, previous))
    }

    pub(super) fn replace_compaction(
        &self,
        session_id: &str,
        compaction: Option<&ThreadCompaction>,
    ) -> Result<(), String> {
        let json = compaction
            .map(|value| serialize(value, "compaction checkpoint"))
            .transpose()?;
        let changed = self
            .connection
            .execute(
                "UPDATE rollout_sessions SET history_checkpoint_json = ?2 WHERE session_id = ?1",
                params![session_id, json],
            )
            .map_err(|_| "rollout compaction checkpoint 写入失败".to_owned())?;
        if changed != 1 {
            return Err(format!("未找到待压缩 session：{session_id}"));
        }
        Ok(())
    }

    pub(super) fn load_history_window(
        &self,
        session_id: &str,
    ) -> Result<RolloutHistoryWindow, String> {
        let checkpoint_json = self
            .connection
            .query_row(
                "SELECT history_checkpoint_json FROM rollout_sessions WHERE session_id = ?1",
                params![session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| "rollout compaction checkpoint 读取失败".to_owned())?
            .flatten();
        let compaction = checkpoint_json
            .map(|json| deserialize::<ThreadCompaction>(&json, "compaction checkpoint"))
            .transpose()?;
        let after_sequence = compaction.as_ref().map(|value| value.through_sequence);
        let turns =
            load_sequenced_turns_from_connection(&self.connection, session_id, after_sequence)?;
        Ok(RolloutHistoryWindow { compaction, turns })
    }

    #[cfg(test)]
    pub(super) fn append_event(
        &self,
        input: RolloutEventAppendInput,
    ) -> Result<RolloutEventRecord, String> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| "rollout event 事务启动失败".to_owned())?;
        let sequence = transaction
            .query_row(
                "SELECT COALESCE(MAX(sequence) + 1, ?2) FROM rollout_events WHERE session_id = ?1 AND sequence >= ?2",
                params![input.session_id, ROLLOUT_EXTERNAL_EVENT_SEQUENCE_BASE],
                |row| row.get::<_, u32>(0),
            )
            .map_err(|_| "rollout event sequence 读取失败".to_owned())?;
        let event = RolloutEventRecord {
            schema_version: 1,
            session_id: input.session_id,
            parent_session_id: input.parent_session_id,
            sequence,
            event_type: input.event_type,
            recorded_at_ms: input.recorded_at_ms,
            payload: input.payload,
        };
        insert_event(&transaction, &event, sequence)?;
        transaction
            .commit()
            .map_err(|_| "rollout event 事务提交失败".to_owned())?;
        Ok(event)
    }

    pub(super) fn load_events(&self, session_id: &str) -> Result<Vec<RolloutEventRecord>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT event_json FROM rollout_events WHERE session_id = ?1 ORDER BY sequence",
            )
            .map_err(|_| "rollout events 查询准备失败".to_owned())?;
        let rows = statement
            .query_map(params![session_id], |row| row.get::<_, String>(0))
            .map_err(|_| "rollout events 查询失败".to_owned())?;
        let json = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| "rollout events 读取失败".to_owned())?;
        json.into_iter()
            .enumerate()
            .map(|(sequence, value)| {
                let mut event = deserialize::<RolloutEventRecord>(&value, "event")?;
                event.sequence = u32::try_from(sequence).unwrap_or(u32::MAX);
                Ok(event)
            })
            .collect()
    }
}

fn upsert_session(
    connection: &Connection,
    record: &RunSessionRecord,
    title: Option<&str>,
) -> Result<(), String> {
    connection
        .execute(
            r"
            INSERT INTO rollout_sessions (
                session_id, parent_session_id, created_at_ms, updated_at_ms,
                provider, model, title
            ) VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)
            ON CONFLICT(session_id) DO UPDATE SET
                parent_session_id = excluded.parent_session_id,
                updated_at_ms = excluded.updated_at_ms,
                provider = excluded.provider,
                model = excluded.model,
                title = COALESCE(excluded.title, rollout_sessions.title)
            ",
            params![
                record.session_id,
                record.parent_session_id,
                timestamp(record.created_at_ms),
                record.provider,
                record.model,
                title,
            ],
        )
        .map_err(|_| "rollout 会话事务写入失败".to_owned())?;
    Ok(())
}

fn select_current_session(connection: &Connection, session_id: &str) -> Result<(), String> {
    let candidate_timestamp = connection
        .query_row(
            "SELECT updated_at_ms FROM rollout_sessions WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| "rollout 当前根会话时间读取失败".to_owned())?;
    let current_timestamp = connection
        .query_row(
            r"
            SELECT sessions.updated_at_ms
            FROM rollout_current_session AS current
            INNER JOIN rollout_sessions AS sessions
                ON sessions.session_id = current.session_id
            WHERE current.singleton_key = ?1
            ",
            params![CURRENT_SESSION_SINGLETON_KEY],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| "rollout 当前会话时间读取失败".to_owned())?;
    if current_timestamp.is_some_and(|timestamp| timestamp > candidate_timestamp) {
        return Ok(());
    }
    connection
        .execute(
            r"
            INSERT INTO rollout_current_session (singleton_key, session_id)
            VALUES (?1, ?2)
            ON CONFLICT(singleton_key) DO UPDATE SET session_id = excluded.session_id
            ",
            params![CURRENT_SESSION_SINGLETON_KEY, session_id],
        )
        .map_err(|_| "rollout 当前根会话写入失败".to_owned())?;
    Ok(())
}

fn upsert_turn(
    connection: &Connection,
    record: &RunSessionRecord,
    record_json: &str,
) -> Result<u64, String> {
    let existing_sequence = connection
        .query_row(
            "SELECT sequence FROM rollout_turns WHERE session_id = ?1 AND assistant_part_id = ?2",
            params![record.session_id, record.assistant_part_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| "rollout turn 幂等键查询失败".to_owned())?;
    let sequence = existing_sequence.map_or_else(
        || {
            connection
                .query_row(
                    "SELECT COALESCE(MAX(sequence) + 1, 0) FROM rollout_turns WHERE session_id = ?1",
                    params![record.session_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|_| "rollout turn sequence 读取失败".to_owned())
        },
        Ok,
    )?;
    connection
        .execute(
            r"
            INSERT INTO rollout_turns (
                session_id, sequence, assistant_part_id, recorded_at_ms, record_json
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(session_id, assistant_part_id) DO UPDATE SET
                recorded_at_ms = excluded.recorded_at_ms,
                record_json = excluded.record_json
            ",
            params![
                record.session_id,
                sequence,
                record.assistant_part_id,
                timestamp(record.created_at_ms),
                record_json,
            ],
        )
        .map_err(|_| "rollout turn 事务写入失败".to_owned())?;
    u64::try_from(sequence).map_err(|_| "rollout turn sequence 不能为负数".to_owned())
}

fn replace_turn_events(
    connection: &Connection,
    record: &RunSessionRecord,
    turn_sequence: u64,
) -> Result<(), String> {
    let storage_start = turn_sequence
        .checked_mul(ROLLOUT_EVENTS_PER_TURN)
        .ok_or_else(|| "rollout turn event sequence 溢出".to_owned())?;
    let storage_end = storage_start
        .checked_add(ROLLOUT_EVENTS_PER_TURN)
        .ok_or_else(|| "rollout turn event sequence 溢出".to_owned())?;
    if storage_end > u64::from(ROLLOUT_EXTERNAL_EVENT_SEQUENCE_BASE) {
        return Err("rollout turn event sequence 已耗尽".to_owned());
    }
    let events = events_for_session(record)?;
    if events.len() > usize::try_from(ROLLOUT_EVENTS_PER_TURN).unwrap_or(usize::MAX) {
        return Err("单个 rollout turn 事件数超过稳定槽位上限".to_owned());
    }
    connection
        .execute(
            "DELETE FROM rollout_events WHERE session_id = ?1 AND sequence >= ?2 AND sequence < ?3",
            params![
                record.session_id,
                timestamp(storage_start),
                timestamp(storage_end)
            ],
        )
        .map_err(|_| "rollout turn events 事务清理失败".to_owned())?;
    for (index, mut event) in events.into_iter().enumerate() {
        let index = u64::try_from(index).unwrap_or(u64::MAX);
        let storage_sequence = storage_start
            .checked_add(index)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| "rollout turn event sequence 溢出".to_owned())?;
        event.sequence = storage_sequence;
        insert_event(connection, &event, storage_sequence)?;
    }
    Ok(())
}

fn insert_event(
    connection: &Connection,
    event: &RolloutEventRecord,
    storage_sequence: u32,
) -> Result<(), String> {
    let event_json = serialize(event, "event")?;
    connection
        .execute(
            r"
            INSERT INTO rollout_events (
                session_id, sequence, event_type, recorded_at_ms, event_json
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![
                event.session_id,
                storage_sequence,
                event_type_name(event.event_type),
                timestamp(event.recorded_at_ms),
                event_json,
            ],
        )
        .map_err(|_| "rollout event 事务写入失败".to_owned())?;
    Ok(())
}

fn load_sequenced_turns_from_connection(
    connection: &Connection,
    session_id: &str,
    after_sequence: Option<u64>,
) -> Result<Vec<(u64, RunSessionRecord)>, String> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, record_json FROM rollout_turns \
             WHERE session_id = ?1 AND (?2 IS NULL OR sequence > ?2) ORDER BY sequence",
        )
        .map_err(|_| "rollout turns 查询准备失败".to_owned())?;
    let after_sequence = after_sequence.map(|value| i64::try_from(value).unwrap_or(i64::MAX));
    let mut rows = statement
        .query(params![session_id, after_sequence])
        .map_err(|_| "rollout turns 查询失败".to_owned())?;
    let mut turns = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|_| "rollout turns 读取失败".to_owned())?
    {
        let sequence = row
            .get::<_, i64>(0)
            .map_err(|_| "rollout turn sequence 无效".to_owned())?;
        let sequence =
            u64::try_from(sequence).map_err(|_| "rollout turn sequence 不能为负数".to_owned())?;
        let json = row
            .get::<_, String>(1)
            .map_err(|_| "rollout turn JSON 读取失败".to_owned())?;
        turns.push((sequence, deserialize(&json, "turn")?));
    }
    Ok(turns)
}

fn event_type_name(event_type: RolloutEventType) -> &'static str {
    match event_type {
        RolloutEventType::SessionStarted => "session_started",
        RolloutEventType::TurnEvents => "turn_events",
        RolloutEventType::ToolReceipts => "tool_receipts",
        RolloutEventType::ReasoningMessages => "reasoning_messages",
        RolloutEventType::SessionCompleted => "session_completed",
    }
}

fn serialize(value: &impl serde::Serialize, label: &str) -> Result<String, String> {
    serde_json::to_string(value).map_err(|_| format!("rollout {label} 序列化失败"))
}

fn deserialize<T: serde::de::DeserializeOwned>(value: &str, label: &str) -> Result<T, String> {
    serde_json::from_str(value).map_err(|_| format!("rollout {label} JSON 无效"))
}

fn timestamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
