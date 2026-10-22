//! Goal 首次提交的 SQLite 原子事务。

use super::{
    SqliteRuntimeStore,
    helpers::{i64_value, u64_value},
};
use crate::ports::{GoalCommitError, GoalCommitStore, RuntimeStore};
use axon_domain::{ExecutionRecord, ExecutionStatus, GoalRecord, GoalStatus, JarvisEvent};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

impl GoalCommitStore for SqliteRuntimeStore {
    fn commit_goal_start(
        &self,
        goal: &GoalRecord,
        execution: &ExecutionRecord,
        event: &JarvisEvent,
    ) -> Result<(), GoalCommitError> {
        validate_inputs(goal, execution, event)?;
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        ensure_goal(&transaction, goal)?;
        ensure_execution(&transaction, execution)?;
        ensure_event(&transaction, event)?;
        transaction.commit().map_err(storage_error)
    }

    fn commit_existing_goal_start(
        &self,
        goal: &GoalRecord,
        expected_revision: u64,
        execution: &ExecutionRecord,
        event: &JarvisEvent,
    ) -> Result<(), GoalCommitError> {
        if goal.status != GoalStatus::Active || goal.revision != expected_revision.saturating_add(1)
        {
            return Err(GoalCommitError::Conflict(
                "已有 Goal 启动提交必须为 Active 且 revision 已递增一次".to_owned(),
            ));
        }
        if execution.goal_id != goal.goal_id
            || execution.status != ExecutionStatus::Queued
            || execution.attempt == 0
            || execution.lease.is_some()
            || execution.retry_at_ms.is_some()
            || execution.checkpoint.is_some()
        {
            return Err(GoalCommitError::Conflict(
                "启动 Execution 未正确绑定 Goal 或不是 queued 初始状态".to_owned(),
            ));
        }
        if event.sequence != 0
            || event.event_type.trim().is_empty()
            || event.idempotency_key.trim().is_empty()
        {
            return Err(GoalCommitError::Conflict(
                "启动 Event envelope 无效".to_owned(),
            ));
        }
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let updated = transaction
            .execute(
                "UPDATE jarvis_goals SET status='active',revision=?1,updated_at_ms=?2,waiting_reason=NULL WHERE goal_id=?3 AND revision=?4 AND status IN ('proposed','waiting','paused') AND NOT EXISTS (SELECT 1 FROM jarvis_executions WHERE goal_id=?3 AND status='uncertain')",
                params![
                    i64_value(goal.revision).map_err(storage_error)?,
                    i64_value(goal.updated_at_ms).map_err(storage_error)?,
                    goal.goal_id.as_str(),
                    i64_value(expected_revision).map_err(storage_error)?,
                ],
            )
            .map_err(storage_error)?;
        if updated == 0 {
            return Err(GoalCommitError::Conflict(
                "Goal revision 或状态已变化，或存在未决的 uncertain Execution，启动被拒绝"
                    .to_owned(),
            ));
        }
        ensure_execution(&transaction, execution)?;
        ensure_event(&transaction, event)?;
        transaction.commit().map_err(storage_error)
    }

    fn commit_goal_proposal(
        &self,
        goal: &GoalRecord,
        event: &JarvisEvent,
    ) -> Result<(), GoalCommitError> {
        if goal.status != GoalStatus::Proposed
            || goal.revision != 0
            || event.sequence != 0
            || event.event_type.trim().is_empty()
            || event.idempotency_key.trim().is_empty()
        {
            return Err(GoalCommitError::Conflict(
                "Goal 创建 envelope 无效".to_owned(),
            ));
        }
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        ensure_goal(&transaction, goal)?;
        ensure_event(&transaction, event)?;
        transaction.commit().map_err(storage_error)
    }
}

fn validate_inputs(
    goal: &GoalRecord,
    execution: &ExecutionRecord,
    event: &JarvisEvent,
) -> Result<(), GoalCommitError> {
    if goal.status != GoalStatus::Proposed || goal.revision != 0 {
        return Err(GoalCommitError::Conflict(
            "首次提交 Goal 必须为 Proposed 且 revision 为 0".to_owned(),
        ));
    }
    if execution.goal_id != goal.goal_id
        || execution.status != ExecutionStatus::Queued
        || execution.attempt != 1
        || execution.lease.is_some()
        || execution.retry_at_ms.is_some()
        || execution.checkpoint.is_some()
    {
        return Err(GoalCommitError::Conflict(
            "首个 Execution 未正确绑定 Goal 或不是 queued 初始状态".to_owned(),
        ));
    }
    if event.sequence != 0
        || event.event_type.trim().is_empty()
        || event.idempotency_key.trim().is_empty()
    {
        return Err(GoalCommitError::Conflict(
            "Event envelope 必须使用待分配 sequence 且字段非空".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_goal(
    transaction: &rusqlite::Transaction<'_>,
    goal: &GoalRecord,
) -> Result<(), GoalCommitError> {
    let inserted = transaction
        .execute(
            "INSERT INTO jarvis_goals(goal_id,scope_json,title,status,revision,plan_json,acceptance_json,progress_json,waiting_reason,next_step,priority,deadline_ms,created_from_interaction_id,cancellation_reason,cancellation_requested_at_ms,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,'proposed',0,?4,?5,?6,NULL,NULL,?7,NULL,?8,NULL,NULL,?9,?9) ON CONFLICT(goal_id) DO NOTHING",
            params![
                goal.goal_id.as_str(),
                serde_json::to_string(&goal.scope).map_err(serialize_error)?,
                goal.title,
                serde_json::to_string(&goal.plan).map_err(serialize_error)?,
                serde_json::to_string(&goal.acceptance).map_err(serialize_error)?,
                goal.progress.as_ref().map(serde_json::to_string).transpose().map_err(serialize_error)?,
                goal.priority,
                goal.created_from_interaction_id,
                i64_value(goal.created_at_ms).map_err(storage_error)?,
            ],
        )
        .map_err(storage_error)?;
    if inserted == 1 {
        return Ok(());
    }
    let existing = transaction
        .query_row(
            "SELECT scope_json,title,status,revision,plan_json,acceptance_json,created_at_ms,updated_at_ms FROM jarvis_goals WHERE goal_id=?1",
            params![goal.goal_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error)?
        .ok_or_else(|| GoalCommitError::Conflict("Goal 并发创建后无法读取".to_owned()))?;
    let expected = (
        serde_json::to_string(&goal.scope).map_err(serialize_error)?,
        goal.title.clone(),
        "proposed".to_owned(),
        0_i64,
        serde_json::to_string(&goal.plan).map_err(serialize_error)?,
        serde_json::to_string(&goal.acceptance).map_err(serialize_error)?,
        i64_value(goal.created_at_ms).map_err(storage_error)?,
        i64_value(goal.updated_at_ms).map_err(storage_error)?,
    );
    if existing != expected {
        return Err(GoalCommitError::Conflict(
            "goal_id 已绑定不同 Goal 内容".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_execution(
    transaction: &rusqlite::Transaction<'_>,
    execution: &ExecutionRecord,
) -> Result<(), GoalCommitError> {
    // 新建 Execution 必须绑定创建时的 Session epoch：执行 Worker 依赖该版本
    // 判定 Action 是否陈旧，缺失绑定会被误判为 stale 并直接取消。
    let context_version = match execution.session_id.as_ref() {
        Some(session_id) => {
            let value = transaction
                .query_row(
                    "SELECT context_version FROM session_runtimes WHERE session_id=?1",
                    params![session_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(storage_error)?
                .ok_or_else(|| {
                    GoalCommitError::Conflict("Execution 绑定的 Session 不存在".to_owned())
                })?;
            Some(u64_value(value).map_err(storage_error)?)
        }
        None => None,
    };
    let context_version = context_version
        .map(i64_value)
        .transpose()
        .map_err(storage_error)?;
    let inserted = transaction
        .execute(
            "INSERT INTO jarvis_executions(execution_id,goal_id,session_id,attempt,priority,dedupe_key,status,context_version,payload_json,lease_json,retry_at_ms,checkpoint_json,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?8,NULL,NULL,NULL,?9,?10) ON CONFLICT(execution_id) DO NOTHING",
            params![
                execution.execution_id.as_str(),
                execution.goal_id.as_str(),
                execution.session_id.as_ref().map(|s| s.as_str()),
                execution.attempt,
                execution.priority,
                execution.dedupe_key.as_deref(),
                context_version,
                execution.payload_json,
                i64_value(execution.created_at_ms).map_err(storage_error)?,
                i64_value(execution.updated_at_ms).map_err(storage_error)?,
            ],
        )
        .map_err(storage_error)?;
    if inserted == 1 {
        return Ok(());
    }
    let matches: bool = transaction
        .query_row(
            "SELECT goal_id=?1 AND (session_id IS ?2) AND attempt=?3 AND priority=?4 AND (dedupe_key IS ?5) AND (context_version IS ?6) AND payload_json=?8 AND status='queued' FROM jarvis_executions WHERE execution_id=?7",
            params![
                execution.goal_id.as_str(),
                execution.session_id.as_ref().map(|s| s.as_str()),
                execution.attempt,
                execution.priority,
                execution.dedupe_key.as_deref(),
                context_version,
                execution.execution_id.as_str(),
                execution.payload_json
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?
        .unwrap_or(false);
    if !matches {
        return Err(GoalCommitError::Conflict(
            "execution_id 已绑定不同 Execution 内容".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_event(
    transaction: &rusqlite::Transaction<'_>,
    event: &JarvisEvent,
) -> Result<(), GoalCommitError> {
    type PersistedEvent = (String, i64, String, String, Option<String>, String, i64);

    let existing: Option<PersistedEvent> = transaction
        .query_row(
            "SELECT event_id,sequence,event_type,idempotency_key,cause_event_id,payload_json,occurred_at_ms FROM jarvis_events WHERE idempotency_key=?1",
            params![event.idempotency_key.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error)?;
    if let Some((
        event_id,
        sequence,
        event_type,
        key,
        cause_event_id,
        payload_json,
        occurred_at_ms,
    )) = existing
    {
        let expected = (
            event.event_id.as_str().to_owned(),
            sequence,
            event.event_type.clone(),
            event.idempotency_key.clone(),
            event.cause_event_id.as_ref().map(ToString::to_string),
            event.payload_json.clone(),
            i64_value(event.occurred_at_ms).map_err(storage_error)?,
        );
        if (
            event_id,
            sequence,
            event_type,
            key,
            cause_event_id,
            payload_json,
            occurred_at_ms,
        ) == expected
        {
            return Ok(());
        }
        return Err(GoalCommitError::Conflict(
            "Event 幂等键已绑定不同内容".to_owned(),
        ));
    }
    let expected_sequence: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM jarvis_events",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
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
                i64_value(event.occurred_at_ms).map_err(storage_error)?,
            ],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn storage_error(error: impl std::fmt::Display) -> GoalCommitError {
    GoalCommitError::Storage(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> GoalCommitError {
    GoalCommitError::Storage(error.to_string())
}
