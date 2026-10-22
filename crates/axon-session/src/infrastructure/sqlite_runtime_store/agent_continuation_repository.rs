//! `jarvis_continuations` 的 SQLite 实现。
//!
//! 一条记录承载子代理终态的一次引用：摘要（落盘文本）与 artifact 路径（磁盘绝对路径）
//! 是恢复装配的解析凭据，`job_ref` 只作可读线索：它含进程代际，跨进程必然失效。
//!
//! **同一 continuation 最多产生一次有效执行**：产生侧靠幂等键唯一索引，
//! 消费侧靠条件 `UPDATE` 的受影响行数。并发或重复消费拿到 `0` 行，必须放弃。

use super::{
    SqliteRuntimeStore,
    helpers::{i64_value, optional_u64, u64_value},
};
use crate::ports::{
    AgentContinuationAppendOutcome, AgentContinuationQuery, AgentContinuationRecord,
    AgentContinuationState, AgentContinuationStore, AgentContinuationStoreError, RuntimeStore,
};
use rusqlite::types::Value;
use rusqlite::{OptionalExtension, Row, params, params_from_iter};

/// continuation 读取列：全部读取路径共用，避免列顺序与列集合漂移。
const CONTINUATION_COLUMNS: &str = "continuation_id,idempotency_key,owner_id,parent_session_id,\
     child_session_id,agent_path,job_ref,execution_ref,terminal_kind,exit_reason,summary,\
     artifact_path,token_input,token_output,token_reasoning,state,consumed_at_ms,consumed_by,\
     attempt_count,last_attempt_at_ms,retry_at_ms,last_error,created_at_ms,expires_at_ms";

impl AgentContinuationStore for SqliteRuntimeStore {
    fn append_continuation(
        &self,
        record: &AgentContinuationRecord,
    ) -> Result<AgentContinuationAppendOutcome, AgentContinuationStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "INSERT INTO jarvis_continuations(continuation_id,idempotency_key,owner_id,\
                 parent_session_id,child_session_id,agent_path,job_ref,execution_ref,terminal_kind,\
                 exit_reason,summary,artifact_path,token_input,token_output,token_reasoning,state,\
                 consumed_at_ms,consumed_by,attempt_count,last_attempt_at_ms,retry_at_ms,last_error,\
                 created_at_ms,expires_at_ms) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
                 ?21,?22,?23,?24) \
                 ON CONFLICT(idempotency_key) DO NOTHING",
                params![
                    record.continuation_id,
                    record.idempotency_key,
                    record.owner_id,
                    record.parent_session_id,
                    record.child_session_id,
                    record.agent_path,
                    record.job_ref,
                    record.execution_ref,
                    record.terminal_kind,
                    record.exit_reason,
                    record.summary,
                    record.artifact_path,
                    optional_i64(record.token_input).map_err(storage_error)?,
                    optional_i64(record.token_output).map_err(storage_error)?,
                    optional_i64(record.token_reasoning).map_err(storage_error)?,
                    record.state.as_str(),
                    optional_i64(record.consumed_at_ms).map_err(storage_error)?,
                    record.consumed_by,
                    i64::from(record.attempt_count),
                    optional_i64(record.last_attempt_at_ms).map_err(storage_error)?,
                    optional_i64(record.retry_at_ms).map_err(storage_error)?,
                    record.last_error,
                    i64_value(record.created_at_ms).map_err(storage_error)?,
                    i64_value(record.expires_at_ms).map_err(storage_error)?,
                ],
            )
            .map_err(storage_error)?;
        if changed > 0 {
            return Ok(AgentContinuationAppendOutcome::Inserted);
        }
        // 幂等命中：读回既有记录的状态，让调用方按事实回执而不是猜测结局。
        let existing: Option<String> = connection
            .query_row(
                "SELECT state FROM jarvis_continuations WHERE idempotency_key=?1",
                params![record.idempotency_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let state = existing
            .as_deref()
            .and_then(AgentContinuationState::parse)
            .ok_or_else(|| {
                AgentContinuationStoreError::Storage("幂等键冲突但既有记录不可读".to_owned())
            })?;
        Ok(AgentContinuationAppendOutcome::Duplicate(state))
    }

    fn continuation(
        &self,
        continuation_id: &str,
    ) -> Result<Option<AgentContinuationRecord>, AgentContinuationStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                &format!(
                    "SELECT {CONTINUATION_COLUMNS} FROM jarvis_continuations \
                     WHERE continuation_id=?1"
                ),
                params![continuation_id],
                read_continuation,
            )
            .optional()
            .map_err(storage_error)
    }

    fn continuation_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<AgentContinuationRecord>, AgentContinuationStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                &format!(
                    "SELECT {CONTINUATION_COLUMNS} FROM jarvis_continuations \
                     WHERE idempotency_key=?1"
                ),
                params![idempotency_key],
                read_continuation,
            )
            .optional()
            .map_err(storage_error)
    }

    fn pending_continuations(
        &self,
        owner_id: &str,
        parent_session_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentContinuationRecord>, AgentContinuationStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT {CONTINUATION_COLUMNS} FROM jarvis_continuations \
                 INDEXED BY jarvis_continuations_owner \
                 WHERE owner_id=?1 AND parent_session_id=?2 AND state='pending' \
                 AND expires_at_ms>?3 AND (retry_at_ms IS NULL OR retry_at_ms<=?3) \
                 ORDER BY created_at_ms ASC, continuation_id ASC LIMIT ?4"
            ))
            .map_err(storage_error)?;
        statement
            .query_map(
                params![
                    owner_id,
                    parent_session_id,
                    i64_value(now_ms).map_err(storage_error)?,
                    i64::from(limit)
                ],
                read_continuation,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn consume_continuation(
        &self,
        continuation_id: &str,
        consumed_by: &str,
        now_ms: u64,
    ) -> Result<bool, AgentContinuationStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_continuations \
                 SET state='consumed', consumed_at_ms=?2, consumed_by=?3, retry_at_ms=NULL, \
                     last_error=NULL \
                 WHERE continuation_id=?1 AND state='pending'",
                params![
                    continuation_id,
                    i64_value(now_ms).map_err(storage_error)?,
                    consumed_by
                ],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
    }

    fn record_continuation_failure(
        &self,
        continuation_id: &str,
        error: &str,
        now_ms: u64,
        retry_at_ms: u64,
    ) -> Result<bool, AgentContinuationStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        // continuation 的终止路径只有保留窗口：装配失败不该让终态事实被丢弃，
        // 因此这里只记账与延后，不设尝试上限。
        let changed = connection
            .execute(
                "UPDATE jarvis_continuations \
                 SET attempt_count=attempt_count+1, last_attempt_at_ms=?2, last_error=?3, \
                     retry_at_ms=?4 \
                 WHERE continuation_id=?1 AND state='pending'",
                params![
                    continuation_id,
                    i64_value(now_ms).map_err(storage_error)?,
                    error,
                    i64_value(retry_at_ms).map_err(storage_error)?
                ],
            )
            .map_err(storage_error)?;
        Ok(changed > 0)
    }

    fn expire_continuations(
        &self,
        owner_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<u32, AgentContinuationStoreError> {
        if limit == 0 {
            return Ok(0);
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let changed = connection
            .execute(
                "UPDATE jarvis_continuations SET state='expired' \
                 WHERE continuation_id IN (\
                     SELECT continuation_id FROM jarvis_continuations \
                     INDEXED BY jarvis_continuations_retention \
                     WHERE owner_id=?1 AND state='pending' AND expires_at_ms<=?2 \
                     ORDER BY expires_at_ms ASC, continuation_id ASC LIMIT ?3\
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

    fn list_continuations(
        &self,
        query: &AgentContinuationQuery,
    ) -> Result<Vec<AgentContinuationRecord>, AgentContinuationStoreError> {
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
        if let Some(parent_session_id) = query.parent_session_id.as_deref() {
            conditions.push("parent_session_id=?");
            values.push(Value::Text(parent_session_id.to_owned()));
        }
        if let Some(child_session_id) = query.child_session_id.as_deref() {
            conditions.push("child_session_id=?");
            values.push(Value::Text(child_session_id.to_owned()));
        }
        if let Some(state) = query.state {
            conditions.push("state=?");
            values.push(Value::Text(state.as_str().to_owned()));
        }
        let mut sql = format!("SELECT {CONTINUATION_COLUMNS} FROM jarvis_continuations");
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at_ms ASC, continuation_id ASC LIMIT ?");
        values.push(Value::Integer(i64::from(query.limit)));
        let mut statement = connection.prepare(&sql).map_err(storage_error)?;
        statement
            .query_map(params_from_iter(values), read_continuation)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }
}

fn optional_i64(value: Option<u64>) -> Result<Option<i64>, crate::ports::RuntimeStoreError> {
    value.map(i64_value).transpose()
}

fn read_continuation(row: &Row<'_>) -> rusqlite::Result<AgentContinuationRecord> {
    let state: String = row.get(15)?;
    Ok(AgentContinuationRecord {
        continuation_id: row.get(0)?,
        idempotency_key: row.get(1)?,
        owner_id: row.get(2)?,
        parent_session_id: row.get(3)?,
        child_session_id: row.get(4)?,
        agent_path: row.get(5)?,
        job_ref: row.get(6)?,
        execution_ref: row.get(7)?,
        terminal_kind: row.get(8)?,
        exit_reason: row.get(9)?,
        summary: row.get(10)?,
        artifact_path: row.get(11)?,
        token_input: optional_u64(row.get(12)?)?,
        token_output: optional_u64(row.get(13)?)?,
        token_reasoning: optional_u64(row.get(14)?)?,
        state: AgentContinuationState::parse(&state).ok_or(rusqlite::Error::InvalidQuery)?,
        consumed_at_ms: optional_u64(row.get(16)?)?,
        consumed_by: row.get(17)?,
        attempt_count: u32::try_from(u64_value(row.get(18)?)?).unwrap_or(u32::MAX),
        last_attempt_at_ms: optional_u64(row.get(19)?)?,
        retry_at_ms: optional_u64(row.get(20)?)?,
        last_error: row.get(21)?,
        created_at_ms: u64_value(row.get(22)?)?,
        expires_at_ms: u64_value(row.get(23)?)?,
    })
}

fn storage_error(error: impl std::fmt::Display) -> AgentContinuationStoreError {
    AgentContinuationStoreError::Storage(error.to_string())
}
