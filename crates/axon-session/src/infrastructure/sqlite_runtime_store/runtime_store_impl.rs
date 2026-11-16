//! `RuntimeStore` 端口的 SQLite 实现：全部方法都委托到各 repository 模块。
//!
//! 单独成文件只是为了让 store 本体保持可读：它承载连接池、路径解析与事件日志入口。

use super::{
    action_repository, commit_repository, failure_repository,
    goal_result_document, interrupted_execution_repository, interrupted_interaction_repository,
    runtime_repository, schedule_repository, session_control_repository, SqliteRuntimeStore,
};
use super::helpers::{i64_value, storage};
use crate::domain::runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord,
    SessionRuntimeRecord,
};
use crate::ports::{CheckpointTelemetry, RuntimeStore, RuntimeStoreError};
use axon_domain::{
    DaemonSessionId, RuntimeHostId, SessionActionId, SessionActionStatus,
    SessionRuntimeStatus,
};
use rusqlite::{TransactionBehavior, params};

impl RuntimeStore for SqliteRuntimeStore {
    fn initialize(&self) -> Result<(), RuntimeStoreError> {
        self.control.ensure_schema()?;
        self.outbox.ensure_schema()
    }

    fn claim_host(
        &self,
        host_id: &RuntimeHostId,
        pid: u32,
        process_start_id: Option<&str>,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<DaemonHostRecord, RuntimeStoreError> {
        runtime_repository::claim_host(
            self,
            host_id,
            pid,
            process_start_id,
            now_ms,
            lease_duration_ms,
        )
    }

    fn renew_host(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        runtime_repository::renew_host(self, host, now_ms, lease_duration_ms)
    }

    fn request_upgrade(
        &self,
        host: &DaemonHostRecord,
        handoff_token: &str,
        target_version: &str,
        deadline_at_ms: u64,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        runtime_repository::request_upgrade(
            self,
            host,
            handoff_token,
            target_version,
            deadline_at_ms,
            now_ms,
        )
    }

    fn mark_handoff_ready(
        &self,
        host: &DaemonHostRecord,
        handoff_token: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        runtime_repository::mark_handoff_ready(self, host, handoff_token, now_ms)
    }

    fn interrupt_host_actions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<u32, RuntimeStoreError> {
        {
            let mut connection = self.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            Self::ensure_fence(&transaction, host, now_ms)?;
            transaction.execute(
                "UPDATE sessions SET status='recovering',owner_host_id=?1,owner_generation=?2,last_activity_at_ms=?3,updated_at_ms=?3 WHERE owner_host_id=?1 AND owner_generation=?2 AND status IN ('running','waiting','compacting')",
                params![host.host_id.as_str(), host.generation, i64_value(now_ms)?],
            ).map_err(storage)?;
            transaction.commit().map_err(storage)?;
        }
        interrupted_execution_repository::quarantine_host_executions(self, host, now_ms)
    }

    fn claim_upgrade_replacement(
        &self,
        host_id: &RuntimeHostId,
        pid: u32,
        process_start_id: Option<&str>,
        handoff_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<DaemonHostRecord, RuntimeStoreError> {
        runtime_repository::claim_upgrade_replacement(
            self,
            host_id,
            pid,
            process_start_id,
            handoff_token,
            now_ms,
            lease_duration_ms,
        )
    }

    fn create_session(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        self.create_session_impl(session_id, cwd, now_ms)
    }

    fn stop_session(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        session_control_repository::stop_session(self, session_id, host, now_ms)
    }

    fn reset_session(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        session_control_repository::reset_session(self, session_id, cwd, host, now_ms)
    }

    fn get_session(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError> {
        self.get_session_impl(session_id)
    }

    fn recover_expired_sessions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError> {
        runtime_repository::recover_expired_sessions(self, host, now_ms, limit)
    }

    fn set_session_status(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        status: SessionRuntimeStatus,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        Self::ensure_fence(&transaction, host, now_ms)?;
        let changed = transaction.execute("UPDATE sessions SET status=?1,owner_host_id=?2,owner_generation=?3,updated_at_ms=?4,last_activity_at_ms=?4 WHERE session_id=?5", params![status.as_str(), host.host_id.as_str(), host.generation, i64_value(now_ms)?, session_id.as_str()]).map_err(storage)?;
        if changed == 0 {
            return Err(RuntimeStoreError::NotFound(session_id.to_string()));
        }
        transaction.commit().map_err(storage)
    }

    fn enqueue_action(
        &self,
        action: &SessionActionRecord,
        now_ms: u64,
    ) -> Result<SessionActionRecord, RuntimeStoreError> {
        action_repository::enqueue_action(self, action, now_ms)
    }

    fn claim_next_action(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<SessionActionRecord>, RuntimeStoreError> {
        action_repository::claim_next_action(self, session_id, host, now_ms, lease_duration_ms)
    }

    fn renew_action_lease(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        action_repository::renew_action_lease(
            self,
            action_id,
            session_id,
            host,
            now_ms,
            lease_duration_ms,
        )
    }

    fn list_pending_actions(
        &self,
        session_id: &DaemonSessionId,
        limit: u32,
    ) -> Result<Vec<SessionActionRecord>, RuntimeStoreError> {
        self.list_pending_actions_impl(session_id, limit)
    }

    fn cancel_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<SessionActionRecord, RuntimeStoreError> {
        action_repository::cancel_action(self, action_id, session_id, now_ms)
    }

    fn list_runnable_execution_sessions(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError> {
        runtime_repository::list_runnable_execution_sessions(self, now_ms, limit)
    }

    fn update_action_status(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        status: SessionActionStatus,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError> {
        commit_repository::update_action_status(self, action_id, session_id, host, status, now_ms)
    }

    fn fail_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        error: &str,
        max_attempts: u32,
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        failure_repository::fail_action(
            self,
            action_id,
            session_id,
            host,
            error,
            max_attempts,
            now_ms,
        )
    }

    fn quarantine_interrupted_executions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        interrupted_execution_repository::quarantine_interrupted_executions(
            self, host, now_ms, limit,
        )
    }

    fn quarantine_interrupted_interactions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        interrupted_interaction_repository::quarantine_interrupted_interactions(
            self, host, now_ms, limit,
        )
    }

    fn commit_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        next_execution: Option<&SessionActionRecord>,
        settlement: Option<&crate::ports::ExecutionSettlement>,
        checkpoint: &CheckpointTelemetry,
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        commit_repository::commit_action(
            self,
            action_id,
            session_id,
            host,
            events,
            next_execution,
            settlement,
            checkpoint,
            now_ms,
        )
    }


    fn upsert_schedule(&self, schedule: &RuntimeScheduleRecord) -> Result<(), RuntimeStoreError> {
        schedule_repository::upsert_schedule(self, schedule)
    }

    fn claim_due_schedules(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeScheduleRecord>, RuntimeStoreError> {
        schedule_repository::claim_due_schedules(self, host, now_ms, lease_duration_ms, limit)
    }

    fn settle_schedule(
        &self,
        schedule_id: &str,
        host: &DaemonHostRecord,
        next_run_at_ms: Option<u64>,
        now_ms: u64,
        failed: bool,
    ) -> Result<(), RuntimeStoreError> {
        schedule_repository::settle_schedule(
            self,
            schedule_id,
            host,
            next_run_at_ms,
            now_ms,
            failed,
        )
    }

    fn save_goal_result_document(
        &self,
        session_id: &DaemonSessionId,
        execution_id: &axon_domain::ExecutionId,
        content: &str,
    ) -> Result<String, RuntimeStoreError> {
        goal_result_document::save(self, session_id, execution_id, content)
    }
}

