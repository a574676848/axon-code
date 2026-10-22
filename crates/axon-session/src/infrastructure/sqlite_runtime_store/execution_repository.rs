//! `jarvis_executions` 的 SQLite repository。

use super::{
    SqliteRuntimeStore,
    helpers::{i64_value, u64_value},
};
use crate::ports::ExecutionTrigger;
use crate::ports::{ExecutionStore, ExecutionStoreError, RuntimeStore};
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionLease, ExecutionRecord, ExecutionStatus, GoalId,
};
use rusqlite::{OptionalExtension, Row, ToSql, TransactionBehavior, params, params_from_iter};
use uuid::Uuid;

const RETRY_BACKOFF_BASE_MS: u64 = 1_000;
const RETRY_BACKOFF_MAX_EXPONENT: u32 = 6;
const MAX_EXECUTION_CLAIM_BATCH: u32 = 64;
const EXECUTION_COLUMNS: &str = "execution_id,goal_id,session_id,attempt,priority,dedupe_key,status,lease_json,retry_at_ms,checkpoint_json,created_at_ms,updated_at_ms,payload_json";

impl ExecutionStore for SqliteRuntimeStore {
    fn enqueue_execution_trigger(
        &self,
        trigger: &ExecutionTrigger,
        now_ms: u64,
    ) -> Result<bool, ExecutionStoreError> {
        self.initialize().map_err(runtime_error)?;
        let connection = self.connection().map_err(runtime_error)?;
        if trigger.dedupe_key.trim().is_empty() || trigger.priority < 0 {
            return Err(ExecutionStoreError::Conflict(
                "Execution trigger 幂等键或优先级无效".to_owned(),
            ));
        }
        let inserted = connection
            .execute(
                "INSERT INTO jarvis_executions(execution_id,goal_id,session_id,kind,payload_json,attempt,priority,dedupe_key,status,created_at_ms,updated_at_ms) VALUES(?1,?1,?2,?3,?4,0,?5,?6,'queued',?7,?7) ON CONFLICT(execution_id) DO NOTHING",
                params![
                    trigger.execution_id.as_str(),
                    trigger.session_id.as_str(),
                    trigger.kind,
                    trigger.payload_json,
                    trigger.priority,
                    trigger.dedupe_key,
                    i64_value(now_ms).map_err(runtime_error)?,
                ],
            )
            .map_err(enqueue_error)?;
        Ok(inserted == 1)
    }

    fn enqueue_execution(&self, execution: &ExecutionRecord) -> Result<bool, ExecutionStoreError> {
        self.initialize().map_err(runtime_error)?;
        if execution.priority < 0
            || execution
                .dedupe_key
                .as_deref()
                .is_some_and(|key| key.trim().is_empty())
        {
            return Err(ExecutionStoreError::Conflict(
                "Execution priority 或业务幂等键无效".to_owned(),
            ));
        }
        let inserted = {
            let connection = self.connection().map_err(runtime_error)?;
            connection
            .execute(
                "INSERT INTO jarvis_executions(execution_id,goal_id,session_id,attempt,priority,dedupe_key,status,lease_json,retry_at_ms,checkpoint_json,payload_json,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,?8,?9,?10,?11) ON CONFLICT(execution_id) DO NOTHING",
                params![
                    execution.execution_id.as_str(),
                    execution.goal_id.as_str(),
                    execution.session_id.as_ref().map(|s| s.as_str()),
                    i64::from(execution.attempt),
                    execution.priority,
                    execution.dedupe_key.as_deref(),
                    status_code(execution.status),
                    execution.checkpoint.as_ref().map(serde_json::to_string).transpose().map_err(serialize_error)?,
                    execution.payload_json,
                    i64_value(execution.created_at_ms).map_err(runtime_error)?,
                    i64_value(execution.updated_at_ms).map_err(runtime_error)?,
                ],
            )
            .map_err(enqueue_error)?
        };
        if inserted == 1 {
            return Ok(true);
        }
        let existing = self
            .get_execution(&execution.execution_id)?
            .ok_or_else(|| {
                ExecutionStoreError::Conflict("Execution 并发写入后无法读取".to_owned())
            })?;
        if existing.goal_id == execution.goal_id
            && existing.attempt == execution.attempt
            && existing.priority == execution.priority
            && existing.dedupe_key == execution.dedupe_key
        {
            Ok(false)
        } else {
            Err(ExecutionStoreError::Conflict(
                "execution_id 已绑定不同 Goal 或 attempt".to_owned(),
            ))
        }
    }

    fn get_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<Option<ExecutionRecord>, ExecutionStoreError> {
        let connection = self.connection().map_err(runtime_error)?;
        connection
            .query_row(
                &format!("SELECT {EXECUTION_COLUMNS} FROM jarvis_executions WHERE execution_id=?1"),
                params![execution_id.as_str()],
                read_execution,
            )
            .optional()
            .map_err(storage_error)
    }

    fn list_executions_for_goal(
        &self,
        goal_id: &GoalId,
        limit: u32,
    ) -> Result<Vec<ExecutionRecord>, ExecutionStoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection().map_err(runtime_error)?;
        let mut statement = connection
            .prepare("SELECT execution_id,goal_id,session_id,attempt,priority,dedupe_key,status,lease_json,retry_at_ms,checkpoint_json,created_at_ms,updated_at_ms,payload_json FROM jarvis_executions WHERE goal_id=?1 ORDER BY created_at_ms DESC, execution_id DESC LIMIT ?2")
            .map_err(storage_error)?;
        statement
            .query_map(
                rusqlite::params![goal_id.as_str(), i64::from(limit)],
                read_execution,
            )
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)
    }

    fn claim_executions(
        &self,
        limit: u32,
        owner: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Vec<ExecutionRecord>, ExecutionStoreError> {
        if limit == 0 || owner.trim().is_empty() || lease_duration_ms == 0 {
            return Err(ExecutionStoreError::Conflict("claim 参数无效".to_owned()));
        }
        self.initialize().map_err(runtime_error)?;
        let mut connection = self.connection().map_err(runtime_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let now = i64_value(now_ms).map_err(runtime_error)?;
        let expires = i64_value(now_ms.saturating_add(lease_duration_ms)).map_err(runtime_error)?;
        let batch_limit = i64::from(limit.min(MAX_EXECUTION_CLAIM_BATCH));
        let mut queued_stmt = transaction
            .prepare(&format!("SELECT {EXECUTION_COLUMNS} FROM jarvis_executions WHERE status='queued' AND (retry_at_ms IS NULL OR retry_at_ms<=?1) AND EXISTS (SELECT 1 FROM jarvis_goals WHERE jarvis_goals.goal_id=jarvis_executions.goal_id AND jarvis_goals.status <> 'cancelled') ORDER BY priority DESC, created_at_ms ASC, execution_id ASC LIMIT ?2"))
            .map_err(storage_error)?;
        let queued_executions = queued_stmt
            .query_map(params![now, batch_limit], read_execution)
            .map_err(storage_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_error)?;
        drop(queued_stmt);

        // 租约失效的 running Execution 不在此处重新领取，由中断隔离路径收敛为 uncertain。
        let mut all = Vec::with_capacity(queued_executions.len());
        all.extend(queued_executions);
        all.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.created_at_ms.cmp(&b.created_at_ms))
                .then_with(|| a.execution_id.as_str().cmp(b.execution_id.as_str()))
        });
        all.truncate(limit.min(MAX_EXECUTION_CLAIM_BATCH) as usize);
        let mut executions = all;
        if executions.is_empty() {
            transaction.commit().map_err(storage_error)?;
            return Ok(executions);
        }

        let lease_token = Uuid::new_v4().to_string();
        let lease = ExecutionLease {
            owner: owner.to_owned(),
            token: lease_token.clone(),
            expires_at_ms: u64::try_from(expires)
                .map_err(|_| ExecutionStoreError::Conflict("lease 到期时间无效".to_owned()))?,
        };
        let lease_json = serde_json::to_string(&lease).map_err(serialize_error)?;
        let ids = executions
            .iter()
            .map(|execution| execution.execution_id.as_str().to_owned())
            .collect::<Vec<_>>();
        let placeholders = (6..(6 + ids.len()))
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE jarvis_executions SET status='running',lease_json=?1,claimed_by=?2,claimed_generation=?3,lease_expires_at_ms=?4,retry_at_ms=NULL,updated_at_ms=?5 WHERE execution_id IN ({placeholders}) AND status='queued' AND (retry_at_ms IS NULL OR retry_at_ms<=?5)"
        );
        let owner_value = owner.to_owned();
        let mut bindings: Vec<&dyn ToSql> =
            vec![&lease_json, &owner_value, &lease_token, &expires, &now];
        bindings.extend(ids.iter().map(|id| id as &dyn ToSql));
        let changed = transaction
            .execute(&sql, params_from_iter(bindings))
            .map_err(storage_error)?;
        if changed != executions.len() {
            return Err(ExecutionStoreError::Conflict(
                "Execution claim 在事务内发生并发变化".to_owned(),
            ));
        }
        for execution in &mut executions {
            execution.status = ExecutionStatus::Running;
            execution.lease = Some(lease.clone());
            execution.retry_at_ms = None;
            execution.updated_at_ms = now_ms;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(executions)
    }

    fn renew_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<(), ExecutionStoreError> {
        if lease_duration_ms == 0 {
            return Err(ExecutionStoreError::Conflict(
                "lease 时长必须大于 0".to_owned(),
            ));
        }
        let connection = self.connection().map_err(runtime_error)?;
        let lease = ExecutionLease {
            owner: owner.to_owned(),
            token: lease_token.to_owned(),
            expires_at_ms: now_ms.saturating_add(lease_duration_ms),
        };
        let changed = connection
            .execute(
                "UPDATE jarvis_executions SET lease_json=?1,lease_expires_at_ms=?2,updated_at_ms=?3 WHERE execution_id=?4 AND status='running' AND cancellation_requested_at_ms IS NULL AND claimed_by=?5 AND claimed_generation=?6 AND lease_expires_at_ms>?3",
                params![
                    serde_json::to_string(&lease).map_err(serialize_error)?,
                    i64_value(lease.expires_at_ms).map_err(runtime_error)?,
                    i64_value(now_ms).map_err(runtime_error)?,
                    execution_id.as_str(),
                    owner,
                    lease_token
                ],
            )
            .map_err(storage_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(ExecutionStoreError::Conflict(
                "Execution lease 已失效".to_owned(),
            ))
        }
    }

    fn fail_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        max_attempts: u32,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError> {
        if max_attempts == 0 {
            return Err(ExecutionStoreError::Conflict(
                "最大尝试次数必须大于 0".to_owned(),
            ));
        }
        let connection = self.connection().map_err(runtime_error)?;
        let current_attempt: Option<u32> = connection
            .query_row(
                "SELECT attempt FROM jarvis_executions WHERE execution_id=?1 AND status='running' AND claimed_by=?2 AND claimed_generation=?3",
                params![execution_id.as_str(), owner, lease_token],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)?
            .map(|attempt| u32::try_from(attempt).map_err(|_| ExecutionStoreError::Conflict("Execution attempt 超出范围".to_owned())))
            .transpose()?;
        let attempt = current_attempt
            .ok_or_else(|| ExecutionStoreError::Conflict("Execution lease 已失效".to_owned()))?;
        let terminal = attempt >= max_attempts;
        let retry_at_ms = (!terminal).then(|| {
            let exponent = attempt.saturating_sub(1).min(RETRY_BACKOFF_MAX_EXPONENT);
            let backoff_ms = RETRY_BACKOFF_BASE_MS.saturating_mul(1_u64 << exponent);
            now_ms.saturating_add(backoff_ms)
        });
        let next_attempt = attempt.saturating_add(1);
        let changed = connection
            .execute(
                "UPDATE jarvis_executions SET status=?1,attempt=CASE WHEN ?1='queued' THEN ?2 ELSE attempt END,lease_json=NULL,claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL,retry_at_ms=?3,updated_at_ms=?4 WHERE execution_id=?5 AND status='running' AND claimed_by=?6 AND claimed_generation=?7",
                params![
                    if terminal { "failed" } else { "queued" },
                    i64::from(next_attempt),
                    retry_at_ms.map(i64_value).transpose().map_err(runtime_error)?,
                    i64_value(now_ms).map_err(runtime_error)?,
                    execution_id.as_str(),
                    owner,
                    lease_token
                ],
            )
            .map_err(enqueue_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(ExecutionStoreError::Conflict(
                "Execution lease 已失效".to_owned(),
            ))
        }
    }

    fn cancel_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError> {
        self.settle_execution(
            execution_id,
            owner,
            lease_token,
            ExecutionStatus::Cancelled,
            now_ms,
        )
    }

    fn settle_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        status: ExecutionStatus,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError> {
        if !status.is_terminal()
            && !matches!(
                status,
                ExecutionStatus::Waiting | ExecutionStatus::Uncertain
            )
        {
            return Err(ExecutionStoreError::Conflict(
                "只能提交等待、uncertain 或终态".to_owned(),
            ));
        }
        let changed = {
            let connection = self.connection().map_err(runtime_error)?;
            connection
                .execute(
                    "UPDATE jarvis_executions SET status=?1,lease_json=NULL,claimed_by=NULL,claimed_generation=NULL,lease_expires_at_ms=NULL,cancelled_at_ms=CASE WHEN ?1='cancelled' THEN ?2 ELSE cancelled_at_ms END,updated_at_ms=?2 WHERE execution_id=?3 AND status='running' AND (cancellation_requested_at_ms IS NULL OR ?1='cancelled') AND claimed_by=?4 AND claimed_generation=?5",
                    params![status_code(status), i64_value(now_ms).map_err(runtime_error)?, execution_id.as_str(), owner, lease_token],
                )
                .map_err(storage_error)?
        };
        if changed == 1 {
            Ok(())
        } else if self.get_execution(execution_id)?.is_none() {
            Err(ExecutionStoreError::NotFound(execution_id.clone()))
        } else {
            Err(ExecutionStoreError::Conflict(
                "Execution lease 已失效".to_owned(),
            ))
        }
    }

    fn execution_for_goal_dedupe(
        &self,
        goal_id: &GoalId,
        dedupe_key: &str,
    ) -> Result<Option<ExecutionRecord>, ExecutionStoreError> {
        let connection = self.connection().map_err(runtime_error)?;
        connection.query_row(
            &format!("SELECT {EXECUTION_COLUMNS} FROM jarvis_executions WHERE goal_id=?1 AND dedupe_key=?2 ORDER BY created_at_ms,execution_id LIMIT 1"),
            params![goal_id.as_str(), dedupe_key],
            read_execution,
        ).optional().map_err(storage_error)
    }
}

fn read_execution(row: &Row<'_>) -> rusqlite::Result<ExecutionRecord> {
    Ok(ExecutionRecord {
        execution_id: ExecutionId::new(row.get::<_, String>(0)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        goal_id: GoalId::new(row.get::<_, String>(1)?)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        session_id: row
            .get::<_, Option<String>>(2)?
            .map(|value| DaemonSessionId::new(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        attempt: u32::try_from(row.get::<_, i64>(3)?).map_err(|_| rusqlite::Error::InvalidQuery)?,
        priority: row.get(4)?,
        dedupe_key: row.get(5)?,
        status: parse_status(&row.get::<_, String>(6)?)?,
        lease: row
            .get::<_, Option<String>>(7)?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        retry_at_ms: row.get::<_, Option<i64>>(8)?.map(u64_value).transpose()?,
        checkpoint: row
            .get::<_, Option<String>>(9)?
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at_ms: u64_value(row.get(10)?)?,
        updated_at_ms: u64_value(row.get(11)?)?,
        payload_json: row.get(12)?,
    })
}

fn parse_status(value: &str) -> rusqlite::Result<ExecutionStatus> {
    match value {
        "queued" => Ok(ExecutionStatus::Queued),
        "running" => Ok(ExecutionStatus::Running),
        "waiting" => Ok(ExecutionStatus::Waiting),
        "uncertain" => Ok(ExecutionStatus::Uncertain),
        "completed" => Ok(ExecutionStatus::Completed),
        "failed" => Ok(ExecutionStatus::Failed),
        "cancelled" => Ok(ExecutionStatus::Cancelled),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn status_code(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Queued => "queued",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Waiting => "waiting",
        ExecutionStatus::Uncertain => "uncertain",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

fn runtime_error(error: impl std::fmt::Display) -> ExecutionStoreError {
    ExecutionStoreError::Storage(error.to_string())
}

fn storage_error(error: rusqlite::Error) -> ExecutionStoreError {
    ExecutionStoreError::Storage(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> ExecutionStoreError {
    ExecutionStoreError::Storage(error.to_string())
}

fn enqueue_error(error: rusqlite::Error) -> ExecutionStoreError {
    if error
        .to_string()
        .contains("jarvis_executions.goal_id, jarvis_executions.dedupe_key")
    {
        ExecutionStoreError::Conflict("同一 Goal 的业务幂等键已有 active Execution".to_owned())
    } else {
        storage_error(error)
    }
}
