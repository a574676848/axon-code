//! `jarvis_agent_messages` 的 SQLite 实现。
//!
//! 一条消息一行，投递进度写在同行：
//!
//! - `state='pending'`：尚未成功交付给目标代理；`attempt_count` 与 `retry_at_ms`
//!   驱动有界重试，越过保留窗口或达到尝试上限后转 `expired`；
//! - `state='delivered'`：已交付目标代理 mailbox，是正常终态。
//!
//! 幂等由 `jarvis_agent_messages_idempotency` 唯一索引承担：同键重复写入
//! `ON CONFLICT DO NOTHING`，受影响行数为 `0`，调用方据此判断「本条是不是首次入队」。
//! 状态迁移一律用条件 `UPDATE` 的受影响行数裁决，因此不存在重复迁移。

use super::{
    SqliteRuntimeStore,
    helpers::{i64_value, optional_u64, u64_value},
};
use crate::ports::{
    AgentMailboxAppendOutcome, AgentMailboxMessageKind, AgentMailboxMessageRecord,
    AgentMailboxMessageState, AgentMailboxQuery, AgentMailboxRetryPolicy, AgentMailboxStore,
    AgentMailboxStoreError, RuntimeStore,
};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params, params_from_iter};

/// 消息读取列：全部读取路径共用，避免列顺序与列集合漂移。
const MESSAGE_COLUMNS: &str = "message_id,idempotency_key,owner_id,author,recipient,kind,content,\
     trigger_turn,continuation_id,state,attempt_count,last_attempt_at_ms,retry_at_ms,last_error,\
     delivered_at_ms,created_at_ms,expires_at_ms";

/// 待投递消息的排序键：先到期的重试优先，其次按入队顺序，保证推进可预测。
const PENDING_ORDER: &str = "ORDER BY retry_at_ms ASC, created_at_ms ASC, message_id ASC";

impl AgentMailboxStore for SqliteRuntimeStore {
    fn append_agent_message(
        &self,
        message: &AgentMailboxMessageRecord,
    ) -> Result<AgentMailboxAppendOutcome, AgentMailboxStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "INSERT INTO jarvis_agent_messages(message_id,idempotency_key,owner_id,author,\
                 recipient,kind,content,trigger_turn,continuation_id,state,attempt_count,\
                 last_attempt_at_ms,retry_at_ms,last_error,delivered_at_ms,created_at_ms,\
                 expires_at_ms) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) \
                 ON CONFLICT(idempotency_key) DO NOTHING",
                params![
                    message.message_id,
                    message.idempotency_key,
                    message.owner_id,
                    message.author,
                    message.recipient,
                    message.kind.as_str(),
                    message.content,
                    message.trigger_turn,
                    message.continuation_id,
                    message.state.as_str(),
                    i64::from(message.attempt_count),
                    optional_i64(message.last_attempt_at_ms).map_err(storage_error)?,
                    optional_i64(message.retry_at_ms).map_err(storage_error)?,
                    message.last_error,
                    optional_i64(message.delivered_at_ms).map_err(storage_error)?,
                    i64_value(message.created_at_ms).map_err(storage_error)?,
                    i64_value(message.expires_at_ms).map_err(storage_error)?,
                ],
            )
            .map_err(storage_error)?;
        if changed > 0 {
            return Ok(AgentMailboxAppendOutcome::Inserted);
        }
        // 幂等命中：读回既有记录的状态，让调用方按事实回执而不是猜测结局。
        let existing: Option<String> = connection
            .query_row(
                "SELECT state FROM jarvis_agent_messages WHERE idempotency_key=?1",
                params![message.idempotency_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let state = existing
            .as_deref()
            .and_then(AgentMailboxMessageState::parse)
            .ok_or_else(|| {
                AgentMailboxStoreError::Storage("幂等键冲突但既有记录不可读".to_owned())
            })?;
        Ok(AgentMailboxAppendOutcome::Duplicate(state))
    }

    fn agent_message(
        &self,
        message_id: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentMailboxStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                &format!("SELECT {MESSAGE_COLUMNS} FROM jarvis_agent_messages WHERE message_id=?1"),
                params![message_id],
                read_message,
            )
            .optional()
            .map_err(storage_error)
    }

    fn agent_message_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentMailboxStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                &format!(
                    "SELECT {MESSAGE_COLUMNS} FROM jarvis_agent_messages \
                     WHERE idempotency_key=?1"
                ),
                params![idempotency_key],
                read_message,
            )
            .optional()
            .map_err(storage_error)
    }

    fn pending_agent_messages(
        &self,
        owner_id: &str,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM jarvis_agent_messages \
                 INDEXED BY jarvis_agent_messages_recipient \
                 WHERE owner_id=?1 AND recipient=?2 AND state='pending' AND expires_at_ms>?3 \
                 AND (retry_at_ms IS NULL OR retry_at_ms<=?3) {PENDING_ORDER} LIMIT ?4"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    owner_id,
                    recipient,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit),
                ],
                read_message,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn due_agent_messages(
        &self,
        owner_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM jarvis_agent_messages \
                 INDEXED BY jarvis_agent_messages_retry \
                 WHERE owner_id=?1 AND state='pending' AND expires_at_ms>?2 \
                 AND (retry_at_ms IS NULL OR retry_at_ms<=?2) {PENDING_ORDER} LIMIT ?3"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    owner_id,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit)
                ],
                read_message,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn deliver_agent_message(
        &self,
        message_id: &str,
        now_ms: u64,
    ) -> Result<bool, AgentMailboxStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_agent_messages \
                 SET state='delivered', delivered_at_ms=?2, last_attempt_at_ms=?2, retry_at_ms=NULL, \
                     last_error=NULL \
                 WHERE message_id=?1 AND state='pending'",
                params![
                    message_id,
                    i64_value(now_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
    }

    fn read_agent_message(
        &self,
        message_id: &str,
        now_ms: u64,
    ) -> Result<bool, AgentMailboxStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        // 读确认以「投递确认已经发生」为前置：只有 delivered 能推进到 read，
        // 因此不存在「还没送达就被标记已读」的路径。
        let changed = connection
            .execute(
                "UPDATE jarvis_agent_messages \
                 SET state='read', last_attempt_at_ms=?2 \
                 WHERE message_id=?1 AND state='delivered'",
                params![message_id, i64_value(now_ms).map_err(storage_error)?],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
    }

    fn delivered_unread_agent_messages(
        &self,
        owner_id: &str,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM jarvis_agent_messages \
                 INDEXED BY jarvis_agent_messages_recipient \
                 WHERE owner_id=?1 AND recipient=?2 AND state='delivered' AND expires_at_ms>?3 \
                 ORDER BY delivered_at_ms ASC, message_id ASC LIMIT ?4"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    owner_id,
                    recipient,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit),
                ],
                read_message,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn record_agent_message_failure(
        &self,
        message_id: &str,
        error: &str,
        now_ms: u64,
        policy: AgentMailboxRetryPolicy,
    ) -> Result<bool, AgentMailboxStoreError> {
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        // 先读当前尝试次数：上限判定必须与自增在同一 IMMEDIATE 事务内完成，
        // 否则并发重试会把同一消息推过上限两次。
        let current: Option<i64> = transaction
            .query_row(
                "SELECT attempt_count FROM jarvis_agent_messages \
                 WHERE message_id=?1 AND state='pending'",
                params![message_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let Some(current) = current else {
            // 不存在或已进入终态：不做任何变更，也无需调用方继续重试。
            return Ok(true);
        };
        let next = u64::try_from(current).unwrap_or(u64::MAX).saturating_add(1);
        let exhausted = next >= u64::from(policy.max_attempts);
        transaction
            .execute(
                "UPDATE jarvis_agent_messages \
                 SET attempt_count=?2, last_attempt_at_ms=?3, last_error=?4, retry_at_ms=?5, \
                     state=?6 \
                 WHERE message_id=?1 AND state='pending'",
                params![
                    message_id,
                    i64::try_from(next).unwrap_or(i64::MAX),
                    i64_value(now_ms).map_err(storage_error)?,
                    error,
                    if exhausted {
                        None
                    } else {
                        Some(i64_value(policy.retry_at_ms(now_ms, next)).map_err(storage_error)?)
                    },
                    if exhausted { "expired" } else { "pending" },
                ],
            )
            .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(exhausted)
    }

    fn expire_agent_messages(
        &self,
        owner_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<u32, AgentMailboxStoreError> {
        if limit == 0 {
            return Ok(0);
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_agent_messages SET state='expired' \
                 WHERE message_id IN (\
                     SELECT message_id FROM jarvis_agent_messages \
                     INDEXED BY jarvis_agent_messages_retention \
                     WHERE owner_id=?1 AND state='pending' AND expires_at_ms<=?2 \
                     ORDER BY expires_at_ms ASC, message_id ASC LIMIT ?3\
                 )",
                params![
                    owner_id,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit)
                ],
            )
            .map_err(storage_error)?;
        Ok(u32::try_from(changed).unwrap_or(u32::MAX))
    }

    fn list_agent_messages(
        &self,
        query: &AgentMailboxQuery,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError> {
        if query.limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut conditions: Vec<&str> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        if let Some(owner_id) = query.owner_id.as_deref() {
            conditions.push("owner_id=?");
            values.push(Value::Text(owner_id.to_owned()));
        }
        if let Some(recipient) = query.recipient.as_deref() {
            conditions.push("recipient=?");
            values.push(Value::Text(recipient.to_owned()));
        }
        if let Some(state) = query.state {
            conditions.push("state=?");
            values.push(Value::Text(state.as_str().to_owned()));
        }
        let mut sql = format!("SELECT {MESSAGE_COLUMNS} FROM jarvis_agent_messages");
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at_ms ASC, message_id ASC LIMIT ?");
        values.push(Value::Integer(i64::from(query.limit)));
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        statement
            .query_map(params_from_iter(values), read_message)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }
}

fn optional_i64(value: Option<u64>) -> Result<Option<i64>, crate::ports::RuntimeStoreError> {
    value.map(i64_value).transpose()
}

fn read_message(row: &Row<'_>) -> rusqlite::Result<AgentMailboxMessageRecord> {
    let kind: String = row.get(5)?;
    let state: String = row.get(9)?;
    let message_id: String = row.get(0)?;
    Ok(AgentMailboxMessageRecord {
        message_id,
        idempotency_key: row.get(1)?,
        owner_id: row.get(2)?,
        author: row.get(3)?,
        recipient: row.get(4)?,
        kind: AgentMailboxMessageKind::parse(&kind).ok_or(rusqlite::Error::InvalidQuery)?,
        content: row.get(6)?,
        trigger_turn: row.get::<_, i64>(7)? != 0,
        continuation_id: row.get(8)?,
        state: AgentMailboxMessageState::parse(&state).ok_or(rusqlite::Error::InvalidQuery)?,
        attempt_count: u32::try_from(u64_value(row.get(10)?)?).unwrap_or(u32::MAX),
        last_attempt_at_ms: optional_u64(row.get(11)?)?,
        retry_at_ms: optional_u64(row.get(12)?)?,
        last_error: row.get(13)?,
        delivered_at_ms: optional_u64(row.get(14)?)?,
        created_at_ms: u64_value(row.get(15)?)?,
        expires_at_ms: u64_value(row.get(16)?)?,
    })
}

fn storage_error(error: impl std::fmt::Display) -> AgentMailboxStoreError {
    AgentMailboxStoreError::Storage(error.to_string())
}
