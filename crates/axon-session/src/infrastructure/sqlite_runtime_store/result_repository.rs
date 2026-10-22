//! `jarvis_results` 的 SQLite 查询与读取确认持久化。

use super::{SqliteRuntimeStore, helpers::u64_value};
use crate::ports::{
    RESULT_ACK_SOURCE_EXPIRED, RESULT_ACK_SOURCE_READ, ResultStore, ResultStoreError, RuntimeStore,
};
use axon_domain::{ExecutionId, GoalId, ResultErrorKind, ResultId, ResultRecord, ScopeRef};
use rusqlite::{OptionalExtension, Row, TransactionBehavior, params};

impl ResultStore for SqliteRuntimeStore {
    fn list_unread_results(
        &self,
        scope: &ScopeRef,
        limit: u32,
    ) -> Result<Vec<ResultRecord>, ResultStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        let scope_json = serde_json::to_string(scope).map_err(storage_error)?;
        let mut statement = connection
            .prepare(
                "SELECT r.result_id,r.goal_id,r.execution_id,r.succeeded,r.summary,
                    r.receipt_refs_json,r.artifact_refs_json,r.error_kind,r.created_at_ms
                 FROM jarvis_results AS r
                 INNER JOIN jarvis_goals AS t ON t.goal_id=r.goal_id
                 LEFT JOIN jarvis_result_reads AS rr ON rr.result_id=r.result_id
                 WHERE t.scope_json=?1 AND rr.acknowledged_at_ms IS NULL
                 ORDER BY r.created_at_ms ASC, r.result_id ASC LIMIT ?2",
            )
            .map_err(storage_error)?;
        statement
            .query_map(params![scope_json, i64::from(limit)], read_result)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn acknowledge_results(
        &self,
        scope: &ScopeRef,
        result_ids: &[ResultId],
        acknowledged_at_ms: u64,
    ) -> Result<u32, ResultStoreError> {
        self.confirm_results(
            scope,
            result_ids,
            acknowledged_at_ms,
            RESULT_ACK_SOURCE_READ,
        )
    }

    fn expire_results(
        &self,
        scope: &ScopeRef,
        result_ids: &[ResultId],
        expired_at_ms: u64,
    ) -> Result<u32, ResultStoreError> {
        self.confirm_results(scope, result_ids, expired_at_ms, RESULT_ACK_SOURCE_EXPIRED)
    }

    fn result_for_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<Option<ResultRecord>, ResultStoreError> {
        self.initialize().map_err(storage_error)?;
        let connection = self.connection().map_err(storage_error)?;
        connection
            .query_row(
                "SELECT r.result_id,r.goal_id,r.execution_id,r.succeeded,r.summary,
                    r.receipt_refs_json,r.artifact_refs_json,r.error_kind,r.created_at_ms
                 FROM jarvis_results AS r
                 INDEXED BY jarvis_results_execution
                 WHERE r.execution_id=?1
                 ORDER BY r.created_at_ms DESC, r.result_id ASC LIMIT 1",
                params![execution_id.as_str()],
                read_result,
            )
            .optional()
            .map_err(storage_error)
    }
}

fn read_result(row: &Row<'_>) -> rusqlite::Result<ResultRecord> {
    Ok(ResultRecord {
        result_id: ResultId::new(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        goal_id: GoalId::new(row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        execution_id: ExecutionId::new(row.get::<_, String>(2)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        succeeded: row.get::<_, bool>(3)?,
        summary: row.get(4)?,
        receipt_refs: serde_json::from_str(&row.get::<_, String>(5)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        artifact_refs: serde_json::from_str(&row.get::<_, String>(6)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        error_kind: row
            .get::<_, Option<String>>(7)?
            .map(|value| match value.as_str() {
                "provider" => Ok(ResultErrorKind::Provider),
                "permission" => Ok(ResultErrorKind::Permission),
                "uncertain" => Ok(ResultErrorKind::Uncertain),
                "runtime" => Ok(ResultErrorKind::Runtime),
                _ => Err(rusqlite::Error::InvalidQuery),
            })
            .transpose()?,
        created_at_ms: u64_value(row.get(8)?)?,
    })
}

impl SqliteRuntimeStore {
    /// 按确认来源写入 Result 读取确认；重复确认与批量重复 ID 都幂等。
    fn confirm_results(
        &self,
        scope: &ScopeRef,
        result_ids: &[ResultId],
        confirmed_at_ms: u64,
        source: &str,
    ) -> Result<u32, ResultStoreError> {
        if result_ids.is_empty() {
            return Ok(0);
        }
        self.initialize().map_err(storage_error)?;
        let scope_json = serde_json::to_string(scope).map_err(storage_error)?;
        let confirmed_at_ms = super::helpers::i64_value(confirmed_at_ms).map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let mut confirmed = 0_u32;
        for result_id in result_ids {
            let changed = transaction
                .execute(
                    "INSERT OR IGNORE INTO jarvis_result_reads(result_id,acknowledged_at_ms,acknowledged_source)
                     SELECT r.result_id,?1,?2
                     FROM jarvis_results AS r
                     INNER JOIN jarvis_goals AS t ON t.goal_id=r.goal_id
                     WHERE r.result_id=?3 AND t.scope_json=?4",
                    params![confirmed_at_ms, source, result_id.as_str(), scope_json],
                )
                .map_err(storage_error)?;
            confirmed = confirmed.saturating_add(u32::try_from(changed).unwrap_or(u32::MAX));
        }
        transaction.commit().map_err(storage_error)?;
        Ok(confirmed)
    }
}

fn storage_error(error: impl std::fmt::Display) -> ResultStoreError {
    ResultStoreError::Storage(error.to_string())
}
