//! `jarvis_user_questions` 的 SQLite 实现。

use super::SqliteRuntimeStore;
use crate::ports::{RuntimeStore, RuntimeStoreError, UserQuestionStore, UserQuestionStoreError};
use axon_domain::{UserQuestionRecord, UserQuestionState};
use rusqlite::{OptionalExtension, Row, params};

const USER_QUESTION_COLUMNS: &str = "\
    question_id, idempotency_key, lane_id, session_id, interaction_id, turn_id, tool_call_id, \
    questions_json, handoff_text, state, deadline_at_ms, answer_json, answered_at_ms, \
    answered_channel, delivered, delivery_mode, created_at_ms, expires_at_ms";

impl UserQuestionStore for SqliteRuntimeStore {
    fn insert_question(
        &self,
        record: &UserQuestionRecord,
    ) -> Result<UserQuestionRecord, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;

        let changed = connection
            .execute(
                "INSERT INTO jarvis_user_questions(\
                    question_id, idempotency_key, lane_id, session_id, interaction_id, turn_id, \
                    tool_call_id, questions_json, handoff_text, state, deadline_at_ms, \
                    answer_json, answered_at_ms, answered_channel, delivered, delivery_mode, \
                    created_at_ms, expires_at_ms\
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18) \
                 ON CONFLICT(idempotency_key) DO NOTHING",
                params![
                    record.question_id,
                    record.idempotency_key,
                    record.lane_id,
                    record.session_id,
                    record.interaction_id,
                    record.turn_id,
                    record.tool_call_id,
                    record.questions_json,
                    record.handoff_text,
                    record.state.as_str(),
                    record.deadline_at_ms,
                    record.answer_json,
                    record.answered_at_ms,
                    record.answered_channel,
                    if record.delivered { 1 } else { 0 },
                    record.delivery_mode,
                    record.created_at_ms,
                    record.expires_at_ms,
                ],
            )
            .map_err(storage_error)?;

        if changed > 0 {
            return Ok(record.clone());
        }

        // 幂等冲突时，查询并返回已存在的记录
        let sql = format!(
            "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions WHERE idempotency_key = ?1 LIMIT 1"
        );
        connection
            .query_row(&sql, params![record.idempotency_key], read_user_question)
            .map_err(storage_error)
    }

    fn get_question_by_id(
        &self,
        question_id: &str,
    ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let sql = format!(
            "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions WHERE question_id = ?1 LIMIT 1"
        );
        connection
            .query_row(&sql, params![question_id], read_user_question)
            .optional()
            .map_err(storage_error)
    }

    fn get_question_by_call_id(
        &self,
        call_id: &str,
    ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let sql = format!(
            "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions WHERE tool_call_id = ?1 LIMIT 1"
        );
        connection
            .query_row(&sql, params![call_id], read_user_question)
            .optional()
            .map_err(storage_error)
    }

    fn list_questions_by_lane(
        &self,
        lane_id: &str,
        state: Option<UserQuestionState>,
        limit: usize,
    ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;

        let limit_i64 = i64::try_from(limit).unwrap_or(50);
        let mut results = Vec::new();

        if let Some(st) = state {
            let sql = format!(
                "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions \
                 WHERE lane_id = ?1 AND state = ?2 \
                 ORDER BY created_at_ms DESC LIMIT ?3"
            );
            let mut stmt = connection.prepare(&sql).map_err(storage_error)?;
            let rows = stmt
                .query_map(params![lane_id, st.as_str(), limit_i64], read_user_question)
                .map_err(storage_error)?;
            for row in rows {
                results.push(row.map_err(storage_error)?);
            }
        } else {
            let sql = format!(
                "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions \
                 WHERE lane_id = ?1 \
                 ORDER BY created_at_ms DESC LIMIT ?2"
            );
            let mut stmt = connection.prepare(&sql).map_err(storage_error)?;
            let rows = stmt
                .query_map(params![lane_id, limit_i64], read_user_question)
                .map_err(storage_error)?;
            for row in rows {
                results.push(row.map_err(storage_error)?);
            }
        }

        Ok(results)
    }

    fn cas_settle_answer(
        &self,
        call_id: &str,
        answer_json: &str,
        answered_at_ms: i64,
        channel: Option<&str>,
        delivery_mode: Option<&str>,
    ) -> Result<UserQuestionRecord, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;

        let changed = connection
            .execute(
                "UPDATE jarvis_user_questions \
                 SET state = 'answered', \
                     answer_json = ?1, \
                     answered_at_ms = ?2, \
                     answered_channel = ?3, \
                     delivery_mode = ?4 \
                 WHERE (tool_call_id = ?5 OR question_id = ?5) \
                   AND state IN ('pending', 'suspended')",
                params![answer_json, answered_at_ms, channel, delivery_mode, call_id],
            )
            .map_err(storage_error)?;

        if changed > 0 {
            let sql = format!(
                "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions \
                 WHERE tool_call_id = ?1 OR question_id = ?1 LIMIT 1"
            );
            return connection
                .query_row(&sql, params![call_id], read_user_question)
                .map_err(storage_error);
        }

        // 没更新到行，检查是否存在以及当前状态
        let check_sql = "SELECT state FROM jarvis_user_questions WHERE tool_call_id = ?1 OR question_id = ?1 LIMIT 1";
        let existing_state: Option<String> = connection
            .query_row(check_sql, params![call_id], |row| row.get(0))
            .optional()
            .map_err(storage_error)?;

        match existing_state {
            None => Err(UserQuestionStoreError::NotFound(call_id.to_string())),
            Some(st) => Err(UserQuestionStoreError::AlreadySettled(format!(
                "问题 {call_id} 处于状态 '{st}'，无法再次结算"
            ))),
        }
    }

    fn cas_update_state(
        &self,
        question_id: &str,
        from_state: UserQuestionState,
        to_state: UserQuestionState,
        deadline_at_ms: Option<i64>,
        handoff_text: Option<&str>,
    ) -> Result<bool, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;

        let changed = connection
            .execute(
                "UPDATE jarvis_user_questions \
                 SET state = ?1, \
                     deadline_at_ms = COALESCE(?2, deadline_at_ms), \
                     handoff_text = COALESCE(?3, handoff_text) \
                 WHERE question_id = ?4 AND state = ?5",
                params![
                    to_state.as_str(),
                    deadline_at_ms,
                    handoff_text,
                    question_id,
                    from_state.as_str()
                ],
            )
            .map_err(storage_error)?;

        Ok(changed > 0)
    }

    fn list_unreconciled_answered(
        &self,
        limit: usize,
    ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let limit_i64 = i64::try_from(limit).unwrap_or(50);

        let sql = format!(
            "SELECT {USER_QUESTION_COLUMNS} FROM jarvis_user_questions \
             WHERE state = 'answered' AND delivered = 0 \
             ORDER BY created_at_ms ASC LIMIT ?1"
        );
        let mut stmt = connection.prepare(&sql).map_err(storage_error)?;
        let rows = stmt
            .query_map(params![limit_i64], read_user_question)
            .map_err(storage_error)?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(storage_error)?);
        }
        Ok(results)
    }

    fn mark_delivered(
        &self,
        question_id: &str,
        delivery_mode: &str,
    ) -> Result<bool, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;

        let changed = connection
            .execute(
                "UPDATE jarvis_user_questions \
                 SET delivered = 1, delivery_mode = ?2 \
                 WHERE question_id = ?1",
                params![question_id, delivery_mode],
            )
            .map_err(storage_error)?;

        Ok(changed > 0)
    }

    fn mark_expired_questions(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<usize, UserQuestionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        let limit_i64 = i64::try_from(limit).unwrap_or(100);

        let expired = connection
            .execute(
                "UPDATE jarvis_user_questions \
                 SET state = ?1 \
                 WHERE question_id IN ( \
                     SELECT question_id FROM jarvis_user_questions \
                     WHERE expires_at_ms <= ?2 AND state IN ('pending', 'suspended') \
                     LIMIT ?3 \
                 )",
                params![UserQuestionState::Expired.as_str(), now_ms, limit_i64],
            )
            .map_err(storage_error)?;

        Ok(expired)
    }
}

fn read_user_question(row: &Row<'_>) -> rusqlite::Result<UserQuestionRecord> {
    let state_str: String = row.get(9)?;
    let state = UserQuestionState::parse_state(&state_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            9,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("未知 UserQuestionState: {state_str}"),
            )),
        )
    })?;

    let delivered_int: i64 = row.get(14)?;

    Ok(UserQuestionRecord {
        question_id: row.get(0)?,
        idempotency_key: row.get(1)?,
        lane_id: row.get(2)?,
        session_id: row.get(3)?,
        interaction_id: row.get(4)?,
        turn_id: row.get(5)?,
        tool_call_id: row.get(6)?,
        questions_json: row.get(7)?,
        handoff_text: row.get(8)?,
        state,
        deadline_at_ms: row.get(10)?,
        answer_json: row.get(11)?,
        answered_at_ms: row.get(12)?,
        answered_channel: row.get(13)?,
        delivered: delivered_int != 0,
        delivery_mode: row.get(15)?,
        created_at_ms: row.get(16)?,
        expires_at_ms: row.get(17)?,
    })
}

fn storage_error(error: rusqlite::Error) -> UserQuestionStoreError {
    UserQuestionStoreError::Storage(error.to_string())
}

fn runtime_error(error: RuntimeStoreError) -> UserQuestionStoreError {
    UserQuestionStoreError::Storage(error.to_string())
}
