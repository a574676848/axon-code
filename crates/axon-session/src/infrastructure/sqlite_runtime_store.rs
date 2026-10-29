//! `runtime.sqlite3` 的统一事实源。
use crate::domain::runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord,
    SessionRuntimeRecord,
};
use crate::ports::{
    CheckpointTelemetry, RuntimeStore, RuntimeStoreError,
    ingress_cursor_store::SessionIngressCursorStore,
};
use axon_domain::{
    DaemonSessionId, InteractionId, RuntimeHostId, SessionActionId, SessionActionStatus,
    SessionRuntimeStatus,
};
use rusqlite::{TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[path = "sqlite_runtime_store/action_repository.rs"]
pub mod action_repository;
#[path = "sqlite_runtime_store/agent_continuation_repository.rs"]
mod agent_continuation_repository;
#[path = "sqlite_runtime_store/agent_mailbox_repository.rs"]
mod agent_mailbox_repository;
#[path = "sqlite_runtime_store/client_delivery_repository.rs"]
mod client_delivery_repository;
#[path = "sqlite_runtime_store/commit_repository.rs"]
mod commit_repository;
#[path = "sqlite_runtime_store/connection_pool.rs"]
mod connection_pool;
#[path = "sqlite_runtime_store/decision_repository.rs"]
mod decision_repository;
#[path = "sqlite_runtime_store/event_repository.rs"]
pub mod event_repository;
#[path = "sqlite_runtime_store/execution_repository.rs"]
mod execution_repository;
#[path = "sqlite_runtime_store/failure_repository.rs"]
mod failure_repository;
#[path = "sqlite_runtime_store/goal_commit_repository.rs"]
mod goal_commit_repository;
#[path = "sqlite_runtime_store/goal_repository.rs"]
mod goal_repository;
#[path = "sqlite_runtime_store/goal_result_document.rs"]
mod goal_result_document;
#[path = "sqlite_runtime_store/health_repository.rs"]
mod health_repository;
#[path = "sqlite_runtime_store/helpers.rs"]
mod helpers;
#[path = "sqlite_runtime_store/host_process.rs"]
mod host_process;
#[path = "sqlite_runtime_store/interaction_repository.rs"]
mod interaction_repository;
#[path = "sqlite_runtime_store/interrupted_execution_repository.rs"]
mod interrupted_execution_repository;
#[path = "sqlite_runtime_store/interrupted_interaction_repository.rs"]
mod interrupted_interaction_repository;
#[path = "sqlite_runtime_store/lane_query_repository.rs"]
mod lane_query_repository;
#[path = "sqlite_runtime_store/ledger_commit_repository.rs"]
mod ledger_commit_repository;
#[path = "sqlite_runtime_store/ledger_event_repository.rs"]
mod ledger_event_repository;
#[path = "sqlite_runtime_store/workspace_repository.rs"]
mod workspace_repository;
use connection_pool::{ConnectionPool, PooledConnection};
use helpers::{i64_value, storage};
#[path = "sqlite_runtime_store/lease_repository.rs"]
pub mod lease_repository;
#[path = "sqlite_runtime_store/result_repository.rs"]
mod result_repository;
#[path = "sqlite_runtime_store/runtime_repository.rs"]
pub mod runtime_repository;
#[path = "sqlite_runtime_store/schedule_repository.rs"]
pub mod schedule_repository;
#[path = "sqlite_runtime_store/schema.rs"]
mod schema;
#[path = "sqlite_runtime_store/session_control_repository.rs"]
mod session_control_repository;
#[path = "sqlite_runtime_store/transaction.rs"]
pub mod transaction;
#[path = "sqlite_runtime_store/user_question_repository.rs"]
mod user_question_repository;

const DATABASE_FILE_NAME: &str = "runtime.sqlite3";
const HOST_STATE_RUNNING: &str = "running";

/// SQLite Runtime Store。
///
/// Store 在进程内共享连接池：SQLite 单写者模型下，逐操作重建连接会反复承担
/// 文件打开与 WAL 配置开销，实测每次提交代价相差两个数量级。
#[derive(Debug, Clone)]
pub struct SqliteRuntimeStore {
    path: PathBuf,
    schema_ready: Arc<AtomicBool>,
    schema_init_lock: Arc<Mutex<()>>,
    pool: Arc<ConnectionPool>,
}

impl SqliteRuntimeStore {
    /// 从数据库文件路径创建 Store。
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            pool: Arc::new(ConnectionPool::new(path.clone())),
            path,
            schema_ready: Arc::new(AtomicBool::new(false)),
            schema_init_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 使用自定义写锁等待上限创建 Store。
    ///
    /// 默认构造器沿用生产写锁上限；本构造器用于注入更小的上限，让
    /// 「超过等待上限后有界失败且不留下幽灵 claim」得到确定性验证，
    /// 而不必空转完整生产超时。
    #[must_use]
    pub fn with_busy_timeout(path: impl Into<PathBuf>, busy_timeout: Duration) -> Self {
        let path = path.into();
        Self {
            pool: Arc::new(ConnectionPool::with_busy_timeout(
                path.clone(),
                busy_timeout,
            )),
            path,
            schema_ready: Arc::new(AtomicBool::new(false)),
            schema_init_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 从连接池取用一条运行时连接。
    pub(crate) fn connection(&self) -> Result<PooledConnection<'_>, RuntimeStoreError> {
        self.pool.acquire()
    }

    /// 从 Axon 根目录创建 Store。
    #[must_use]
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        Self::new(root.as_ref().join(DATABASE_FILE_NAME))
    }

    /// 返回数据库路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 回收仍标记为 running、但对应进程已经退出的 Host lease。
    ///
    /// 服务被 launchd/systemd 强制停止时，进程可能来不及执行持久化清理；
    /// 启动新 Host 前回收这些记录，避免等待完整 lease TTL。
    pub fn reap_dead_hosts(&self, now_ms: u64) -> Result<u32, RuntimeStoreError> {
        runtime_repository::reap_dead_hosts(self, now_ms)
    }
}

impl RuntimeStore for SqliteRuntimeStore {
    fn initialize(&self) -> Result<(), RuntimeStoreError> {
        if self.schema_ready.load(Ordering::Acquire)
            || schema::is_schema_initialized_for_path(&self.path)
        {
            if !self.schema_ready.load(Ordering::Relaxed) {
                self.schema_ready.store(true, Ordering::Release);
            }
            return Ok(());
        }
        let _guard = self
            .schema_init_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.schema_ready.load(Ordering::Acquire)
            || schema::is_schema_initialized_for_path(&self.path)
        {
            self.schema_ready.store(true, Ordering::Release);
            return Ok(());
        }
        let mut connection = self.connection()?;
        schema::initialize_schema_for_path(&self.path, &mut connection).map_err(storage)?;
        self.schema_ready.store(true, Ordering::Release);
        Ok(())
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
                "UPDATE session_runtimes SET status='recovering',owner_host_id=?1,owner_generation=?2,last_activity_at_ms=?3,updated_at_ms=?3 WHERE owner_host_id=?1 AND owner_generation=?2 AND status IN ('running','waiting','compacting')",
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
        let changed = transaction.execute("UPDATE session_runtimes SET status=?1,owner_host_id=?2,owner_generation=?3,updated_at_ms=?4,last_activity_at_ms=?4 WHERE session_id=?5", params![status.as_str(), host.host_id.as_str(), host.generation, i64_value(now_ms)?, session_id.as_str()]).map_err(storage)?;
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

    fn append_event(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        event_type: &str,
        payload_json: &str,
        now_ms: u64,
    ) -> Result<SessionEventRecord, RuntimeStoreError> {
        event_repository::append_event(self, session_id, host, event_type, payload_json, now_ms)
    }

    fn append_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        event_repository::append_events(self, session_id, host, events, now_ms)
    }

    fn append_interaction_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        interaction_id: &InteractionId,
        lease_token: &str,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        event_repository::append_interaction_events(
            self,
            session_id,
            host,
            interaction_id,
            lease_token,
            events,
            now_ms,
        )
    }

    fn replay_events(
        &self,
        session_id: &DaemonSessionId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        event_repository::replay_events(self, session_id, after_sequence, limit)
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
