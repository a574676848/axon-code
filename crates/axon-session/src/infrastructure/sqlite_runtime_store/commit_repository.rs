//! Execution 提交与状态迁移原子持久化。

use axon_domain::{DaemonSessionId, SessionActionId, SessionActionStatus};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::domain::runtime::{DaemonHostRecord, SessionActionRecord, SessionEventRecord};
use crate::ports::{CheckpointTelemetry, RuntimeStoreError};

pub(super) fn update_action_status(
    store: &SqliteRuntimeStore,
    action_id: &SessionActionId,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    status: SessionActionStatus,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    let changed = transaction
         .execute(
             "UPDATE jarvis_executions SET status=CASE ?1 WHEN 'committed' THEN 'completed' ELSE ?1 END,committed_at_ms=CASE WHEN ?1='committed' THEN ?2 ELSE committed_at_ms END,cancelled_at_ms=CASE WHEN ?1='cancelled' THEN ?2 ELSE cancelled_at_ms END,lease_json=CASE WHEN ?1='running' THEN lease_json ELSE NULL END,lease_expires_at_ms=CASE WHEN ?1='running' THEN lease_expires_at_ms ELSE NULL END WHERE execution_id=?3 AND session_id=?4 AND claimed_by=?5 AND claimed_generation=?6",
             params![
                 status.as_str(),
                 i64_value(now_ms)?,
                 action_id.as_str(),
                 session_id.as_str(),
                 host.host_id.as_str(),
                 host.generation
             ],
         )
         .map_err(storage)?;
    if changed == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    transaction.commit().map_err(storage)
}

pub(super) fn commit_action(
    store: &SqliteRuntimeStore,
    action_id: &SessionActionId,
    session_id: &DaemonSessionId,
    host: &DaemonHostRecord,
    events: &[(String, String)],
    next_execution: Option<&SessionActionRecord>,
    settlement: Option<&crate::ports::ExecutionSettlement>,
    checkpoint: &CheckpointTelemetry,
    now_ms: u64,
) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    SqliteRuntimeStore::ensure_fence(&transaction, host, now_ms)?;
    if let Some(commit) = settlement {
        if commit.execution.execution_id.as_str() != action_id.as_str()
            || commit.execution.session_id.as_ref() != Some(session_id)
            || next_execution.is_some_and(|next| {
                commit.goal.status != axon_domain::GoalStatus::Active
                    || next.goal_id.as_ref() != Some(&commit.goal.goal_id)
                    || next.session_id != *session_id
            })
        {
            return Err(RuntimeStoreError::Conflict(
                "结算或续跑身份与当前 Goal/Execution 不匹配".to_owned(),
            ));
        }
        super::ledger_commit_repository::commit_in_transaction(
            &transaction,
            &commit.execution,
            &commit.owner,
            &commit.lease_token,
            &commit.result,
            &commit.goal,
            commit.expected_goal_revision,
            &commit.event,
        )
        .map_err(|error| match error {
            crate::ports::LedgerCommitError::Storage(message) => {
                RuntimeStoreError::Storage(message)
            }
            _ => RuntimeStoreError::Conflict(error.to_string()),
        })?;
    }
    let last_sequence = transaction
        .query_row(
            "SELECT last_event_sequence FROM session_runtimes WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| RuntimeStoreError::NotFound(session_id.to_string()))?;
    let mut next_sequence = u64::try_from(last_sequence)
        .map_err(|_| RuntimeStoreError::Storage("事件序号超出范围".to_owned()))?;
    let mut records = Vec::with_capacity(events.len());
    for (event_type, payload_json) in events {
        next_sequence = next_sequence
            .checked_add(1)
            .ok_or_else(|| RuntimeStoreError::Storage("事件序号溢出".to_owned()))?;
        transaction
             .execute(
                 "INSERT INTO session_events(session_id,sequence,generation,event_type,payload_json,emitted_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",
                 params![
                     session_id.as_str(),
                     i64_value(next_sequence)?,
                     host.generation,
                     event_type,
                     payload_json,
                     i64_value(now_ms)?
                 ],
             )
             .map_err(storage)?;
        records.push(SessionEventRecord {
            session_id: session_id.clone(),
            sequence: next_sequence,
            generation: host.generation.clone(),
            event_type: event_type.clone(),
            payload_json: payload_json.clone(),
            emitted_at_ms: now_ms,
        });
    }
    let current_goal_id: String = transaction
        .query_row(
            "SELECT goal_id FROM jarvis_executions WHERE execution_id=?1 AND session_id=?2 AND claimed_by=?3 AND claimed_generation=?4",
            params![
                action_id.as_str(),
                session_id.as_str(),
                host.host_id.as_str(),
                host.generation
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| RuntimeStoreError::NotFound(action_id.to_string()))?;
    if let Some(next_execution) = next_execution {
        transaction
             .execute(
                 "INSERT INTO jarvis_executions(execution_id,goal_id,session_id,kind,priority,status,dedupe_key,payload_json,goal_revision,context_version,attempt,created_at_ms,updated_at_ms) VALUES(?1,?2,?3,?4,?5,'queued',?6,?7,?8,?9,0,?10,?10) ON CONFLICT(execution_id) DO NOTHING",
                 params![
                     next_execution.action_id.as_str(),
                     current_goal_id,
                     next_execution.session_id.as_str(),
                     next_execution.kind,
                     next_execution.priority,
                     next_execution.dedupe_key,
                     next_execution.payload_json,
                     next_execution.goal_revision.map(i64_value).transpose()?,
                     next_execution.context_version.map(i64_value).transpose()?,
                     i64_value(now_ms)?
                 ],
             )
             .map_err(storage)?;
    }
    let settled = transaction
         .execute(
             "UPDATE jarvis_executions SET status=CASE WHEN status IN ('failed','cancelled','uncertain') THEN status ELSE 'completed' END,committed_at_ms=?1,lease_json=NULL,lease_expires_at_ms=NULL,claimed_by=NULL,claimed_generation=NULL WHERE execution_id=?2 AND session_id=?3 AND claimed_by=?4 AND claimed_generation=?5",
             params![
                 i64_value(now_ms)?,
                 action_id.as_str(),
                 session_id.as_str(),
                 host.host_id.as_str(),
                 host.generation
             ],
         )
         .map_err(storage)?;
    if settled == 0 {
        return Err(RuntimeStoreError::Fenced);
    }
    let checkpoint_seq = transaction
        .query_row(
            "SELECT last_checkpoint_seq + 1 FROM session_runtimes WHERE session_id=?1",
            params![session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage)?;
    let goal_revision: Option<i64> = transaction
        .query_row(
            "SELECT goal_revision FROM jarvis_executions WHERE execution_id=?1",
            params![action_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map_err(storage)?
        .flatten();
    transaction
         .execute(
             "INSERT INTO session_checkpoints(session_id,checkpoint_seq,action_id,context_version,tool_receipt_sequence,goal_revision,token_usage,time_usage_ms,agent_tree_summary,transcript_ref,created_at_ms) VALUES(?1,?2,?3,COALESCE((SELECT context_version FROM session_runtimes WHERE session_id=?1),0),?4,?5,?6,?7,?8,?9,?10)",
             params![
                 session_id.as_str(),
                 checkpoint_seq,
                 action_id.as_str(),
                 i64_value(checkpoint.tool_receipt_sequence.unwrap_or(next_sequence))?,
                 goal_revision,
                 checkpoint.token_usage.map(i64_value).transpose()?,
                 checkpoint.time_usage_ms.map(i64_value).transpose()?,
                 checkpoint.agent_tree_summary,
                 checkpoint.transcript_ref,
                 i64_value(now_ms)?
             ],
         )
         .map_err(storage)?;
    transaction
         .execute(
             "UPDATE session_runtimes SET status='idle',owner_host_id=?1,owner_generation=?2,last_checkpoint_seq=last_checkpoint_seq+1,last_event_sequence=?3,last_activity_at_ms=?4,updated_at_ms=?4 WHERE session_id=?5",
             params![
                 host.host_id.as_str(),
                 host.generation,
                 i64_value(next_sequence)?,
                 i64_value(now_ms)?,
                 session_id.as_str()
             ],
         )
         .map_err(storage)?;
    transaction.commit().map_err(storage)?;
    Ok(records)
}
