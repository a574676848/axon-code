//! Work Ledger Execution/Result/Goal/Event 原子提交 repository。

use super::SqliteRuntimeStore;
use crate::ports::{LedgerCommitError, LedgerCommitStore, RuntimeStore};
use axon_domain::{
    ExecutionRecord, ExecutionStatus, GoalRecord, JarvisEvent, ResultErrorKind, ResultRecord,
};
use rusqlite::{TransactionBehavior, params};

use super::helpers::i64_value;

impl LedgerCommitStore for SqliteRuntimeStore {
    fn commit_execution(
        &self,
        execution: &ExecutionRecord,
        owner: &str,
        lease_token: &str,
        result: &ResultRecord,
        goal: &GoalRecord,
        expected_goal_revision: u64,
        event: &JarvisEvent,
    ) -> Result<(), LedgerCommitError> {
        validate_commit_inputs(
            execution,
            owner,
            lease_token,
            result,
            goal,
            expected_goal_revision,
            event,
        )?;
        self.initialize().map_err(storage_error)?;
        let mut connection = self.connection().map_err(storage_error)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        commit_in_transaction(
            &transaction,
            execution,
            owner,
            lease_token,
            result,
            goal,
            expected_goal_revision,
            event,
        )?;
        transaction.commit().map_err(storage_error)
    }
}

/// 复用调用方事务，保证账本、Session checkpoint 与下一阶段同时成功或回滚。
pub(super) fn commit_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    execution: &ExecutionRecord,
    owner: &str,
    lease_token: &str,
    result: &ResultRecord,
    goal: &GoalRecord,
    expected_goal_revision: u64,
    event: &JarvisEvent,
) -> Result<(), LedgerCommitError> {
    validate_commit_inputs(
        execution,
        owner,
        lease_token,
        result,
        goal,
        expected_goal_revision,
        event,
    )?;
    let execution_status = status_code(execution.status);
    let updated = transaction
        .execute(
            "UPDATE jarvis_executions SET status=?1,lease_json=NULL,retry_at_ms=NULL,checkpoint_json=?2,updated_at_ms=?3 WHERE execution_id=?4 AND goal_id=?5 AND status='running' AND cancellation_requested_at_ms IS NULL AND EXISTS (SELECT 1 FROM jarvis_goals WHERE goal_id=?5 AND status <> 'cancelled') AND json_extract(lease_json,'$.owner')=?6 AND json_extract(lease_json,'$.token')=?7",
            params![
                execution_status,
                execution
                    .checkpoint
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(serialize_error)?,
                i64_value(execution.updated_at_ms).map_err(storage_error)?,
                execution.execution_id.as_str(),
                execution.goal_id.as_str(),
                owner,
                lease_token
            ],
        )
        .map_err(storage_error)?;
    if updated == 0 {
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM jarvis_executions WHERE execution_id=?1)",
                params![execution.execution_id.as_str()],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        return Err(if exists {
            LedgerCommitError::Conflict("Execution lease 已失效或状态不是 running".to_owned())
        } else {
            LedgerCommitError::NotFound(execution.execution_id.to_string())
        });
    }

    transaction
        .execute(
            "INSERT INTO jarvis_results(result_id,goal_id,execution_id,succeeded,summary,receipt_refs_json,artifact_refs_json,error_kind,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                result.result_id.as_str(),
                result.goal_id.as_str(),
                result.execution_id.as_str(),
                result.succeeded,
                result.summary,
                serde_json::to_string(&result.receipt_refs).map_err(serialize_error)?,
                serde_json::to_string(&result.artifact_refs).map_err(serialize_error)?,
                result.error_kind.map(error_kind_code),
                i64_value(result.created_at_ms).map_err(storage_error)?
            ],
        )
        .map_err(storage_error)?;

    let next_revision = expected_goal_revision
        .checked_add(1)
        .ok_or_else(|| LedgerCommitError::Conflict("Goal revision 溢出".to_owned()))?;
    let goal_updated = transaction
        .execute(
            "UPDATE jarvis_goals SET scope_json=?1,title=?2,status=?3,revision=?4,plan_json=?5,acceptance_json=?6,updated_at_ms=?7,waiting_reason=?10,next_step=?11,progress_json=?12 WHERE goal_id=?8 AND revision=?9",
            params![
                serde_json::to_string(&goal.scope).map_err(serialize_error)?,
                goal.title,
                goal_status_code(goal.status),
                i64_value(goal.revision).map_err(storage_error)?,
                serde_json::to_string(&goal.plan).map_err(serialize_error)?,
                serde_json::to_string(&goal.acceptance).map_err(serialize_error)?,
                i64_value(goal.updated_at_ms).map_err(storage_error)?,
                goal.goal_id.as_str(),
                i64_value(expected_goal_revision).map_err(storage_error)?,
                goal.waiting_reason,
                goal.next_step,
                goal.progress.as_ref().map(serde_json::to_string).transpose().map_err(serialize_error)?
            ],
        )
        .map_err(storage_error)?;
    if goal_updated == 0 {
        return Err(LedgerCommitError::Conflict(format!(
            "Goal revision 不匹配，期望 {}，实际提交 {}",
            expected_goal_revision, next_revision
        )));
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
        return Err(LedgerCommitError::Conflict(format!(
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

    Ok(())
}

fn validate_commit_inputs(
    execution: &ExecutionRecord,
    owner: &str,
    lease_token: &str,
    result: &ResultRecord,
    goal: &GoalRecord,
    expected_goal_revision: u64,
    event: &JarvisEvent,
) -> Result<(), LedgerCommitError> {
    if !execution.status.is_conclusive() {
        return Err(LedgerCommitError::Conflict(
            "只能提交已形成结论的 Execution".to_owned(),
        ));
    }
    if owner.trim().is_empty() || lease_token.trim().is_empty() {
        return Err(LedgerCommitError::Conflict(
            "Execution lease 参数不能为空".to_owned(),
        ));
    }
    if result.goal_id != execution.goal_id || result.execution_id != execution.execution_id {
        return Err(LedgerCommitError::Conflict(
            "Result 未绑定当前 Goal/Execution".to_owned(),
        ));
    }
    if goal.goal_id != execution.goal_id {
        return Err(LedgerCommitError::Conflict(
            "Goal 未绑定当前 Execution".to_owned(),
        ));
    }
    let expected_revision = expected_goal_revision
        .checked_add(1)
        .ok_or_else(|| LedgerCommitError::Conflict("Goal revision 溢出".to_owned()))?;
    if goal.revision != expected_revision {
        return Err(LedgerCommitError::Conflict(format!(
            "Goal revision 必须为 {}，收到 {}",
            expected_revision, goal.revision
        )));
    }
    if event.event_type.trim().is_empty() || event.idempotency_key.trim().is_empty() {
        return Err(LedgerCommitError::Conflict(
            "Event envelope 无效".to_owned(),
        ));
    }
    Ok(())
}

fn status_code(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Uncertain => "uncertain",
        ExecutionStatus::Cancelled => "cancelled",
        _ => unreachable!("validated conclusive status"),
    }
}

fn goal_status_code(status: axon_domain::GoalStatus) -> &'static str {
    match status {
        axon_domain::GoalStatus::Proposed => "proposed",
        axon_domain::GoalStatus::Active => "active",
        axon_domain::GoalStatus::Waiting => "waiting",
        axon_domain::GoalStatus::Paused => "paused",
        axon_domain::GoalStatus::Completed => "completed",
        axon_domain::GoalStatus::Failed => "failed",
        axon_domain::GoalStatus::Cancelled => "cancelled",
    }
}

fn error_kind_code(kind: ResultErrorKind) -> &'static str {
    match kind {
        ResultErrorKind::Provider => "provider",
        ResultErrorKind::Permission => "permission",
        ResultErrorKind::Uncertain => "uncertain",
        ResultErrorKind::Runtime => "runtime",
    }
}

fn storage_error(error: impl std::fmt::Display) -> LedgerCommitError {
    LedgerCommitError::Storage(error.to_string())
}

fn serialize_error(error: serde_json::Error) -> LedgerCommitError {
    LedgerCommitError::Storage(error.to_string())
}
