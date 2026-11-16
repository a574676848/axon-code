//! `runtime.sqlite3` 的统一事实源。
use crate::domain::runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord,
};
use crate::ports::{
    EventLogStore, RuntimeStoreError, ingress_cursor_store::SessionIngressCursorStore,
};
use axon_domain::{DaemonSessionId, InteractionId, SessionActionStatus};
use rusqlite::Connection;
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
#[path = "sqlite_runtime_store/event_log_repository.rs"]
mod event_log_repository;
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
#[path = "sqlite_runtime_store/interaction_query_repository.rs"]
mod interaction_query_repository;
#[path = "sqlite_runtime_store/interaction_repository.rs"]
mod interaction_repository;
#[path = "sqlite_runtime_store/interaction_repository_row.rs"]
mod interaction_repository_row;
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
use helpers::storage;
#[path = "sqlite_runtime_store/lease_repository.rs"]
pub mod lease_repository;
#[path = "sqlite_runtime_store/result_repository.rs"]
mod result_repository;
#[path = "sqlite_runtime_store/runtime_repository.rs"]
pub mod runtime_repository;
#[path = "sqlite_runtime_store/runtime_store_impl.rs"]
mod runtime_store_impl;
#[path = "sqlite_runtime_store/schedule_repository.rs"]
pub mod schedule_repository;
#[path = "sqlite_runtime_store/schema.rs"]
mod schema;
#[path = "sqlite_runtime_store/session_control_repository.rs"]
mod session_control_repository;
#[path = "sqlite_runtime_store/session_metrics_repository.rs"]
pub(crate) mod session_metrics_repository;
#[path = "sqlite_runtime_store/transaction.rs"]
pub mod transaction;
#[path = "sqlite_runtime_store/user_question_repository.rs"]
mod user_question_repository;

/// 控制面库文件名：会话、队列、目标、调度、游标、账本。
const CONTROL_DATABASE_FILE_NAME: &str = "control.sqlite3";
/// 待消费队列库文件名：Agent 消息、continuation、通知、用户提问、客户端在线。
const OUTBOX_DATABASE_FILE_NAME: &str = "outbox.sqlite3";
/// 会话事件日志根目录名。
const EVENTS_DIR: &str = "events";
const HOST_STATE_RUNNING: &str = "running";

/// Session 事件正文的内联上限（字节）。
///
/// 超过上限的正文按会话追加写入 `<库目录>/archive/session-events/<session>/<分片>.jsonl`，
/// 库里只留引用：B-Tree 只承载定长元数据，大段 tool 结果、diff 与终端输出不再撑大
/// 主库与 WAL。上限取 4 KiB：实测事件正文平均 605 B，只有少数大事件需要外置。
///
/// 流式帧（`wire_event`）不受这个上限约束，一律外置：它们单条只有数百字节，却占
/// 每日写入量的 99% 以上，内联会让主库随帧数线性膨胀。
pub const SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES: usize = 4 * 1024;

/// 一次 WAL 回写的结果。
///
/// `log_frames` 与 `checkpointed_frames` 一起回答「WAL 里还有多少帧没回写」；
/// `busy` 表示这一次被并发读者挡住，剩余部分留给下一次维护周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpoint {
    /// 是否被并发读者挡住。
    pub busy: bool,
    /// checkpoint 开始前 WAL 的帧数。
    pub log_frames: i64,
    /// 本次已回写到主库的帧数。
    pub checkpointed_frames: i64,
}

/// SQLite Runtime Store。
///
/// 事实源按生命周期拆成两个库：`control.sqlite3` 承载需要长期保留与原子条件更新的
/// 控制面状态，`outbox.sqlite3` 承载有界待消费队列。分库后清理 outbox 不会与核心
/// 事务争同一把写锁，两个库各自的 WAL 也互不影响。
///
/// Store 在进程内共享连接池：SQLite 单写者模型下，逐操作重建连接会反复承担
/// 文件打开与 WAL 配置开销，实测每次提交代价相差两个数量级。
#[derive(Debug, Clone)]
pub struct SqliteRuntimeStore {
    control: Arc<DatabaseHandle>,
    outbox: Arc<DatabaseHandle>,
    events_root: PathBuf,
}

/// 单个数据库文件的连接池与建库状态。
///
/// 每个库各自持有进度位与建库互斥：两个库的 DDL 互不相关，共享一把锁只会让
/// 首次打开多等一轮无用时间。
#[derive(Debug)]
struct DatabaseHandle {
    path: PathBuf,
    kind: schema::SchemaKind,
    pool: ConnectionPool,
    schema_ready: AtomicBool,
    schema_init_lock: Mutex<()>,
}

impl DatabaseHandle {
    fn new(path: PathBuf, kind: schema::SchemaKind, busy_timeout: Option<Duration>) -> Self {
        let pool = match busy_timeout {
            Some(timeout) => ConnectionPool::with_busy_timeout(path.clone(), timeout),
            None => ConnectionPool::new(path.clone()),
        };
        Self {
            path,
            kind,
            pool,
            schema_ready: AtomicBool::new(false),
            schema_init_lock: Mutex::new(()),
        }
    }

    fn connection(&self) -> Result<PooledConnection<'_>, RuntimeStoreError> {
        self.pool.acquire()
    }

    fn ensure_schema(&self) -> Result<(), RuntimeStoreError> {
        if self.schema_ready.load(Ordering::Acquire)
            || schema::is_schema_initialized_for_path(&self.path)
        {
            self.schema_ready.store(true, Ordering::Release);
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
        schema::initialize_schema_for_path(&self.path, self.kind, &mut connection)
            .map_err(storage)?;
        self.schema_ready.store(true, Ordering::Release);
        Ok(())
    }

    /// 在空闲窗口把 WAL 回写主库。
    fn checkpoint_wal(&self) -> Result<WalCheckpoint, RuntimeStoreError> {
        let connection = self.connection()?;
        connection
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok(WalCheckpoint {
                    busy: row.get::<_, i64>(0)? != 0,
                    log_frames: row.get(1)?,
                    checkpointed_frames: row.get(2)?,
                })
            })
            .map_err(storage)
    }
}

impl SqliteRuntimeStore {
    /// 从数据库文件路径创建 Store。
    ///
    /// 传入的路径只用于确定**所在目录**：控制面库与待消费队列库固定为该目录下的
    /// `control.sqlite3` 与 `outbox.sqlite3`。调用方不必关心具体文件名，也就不会出现
    /// 「run 写 control、daemon 读 runtime」这类分叉。
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::build(path.into(), None)
    }

    /// 使用自定义写锁等待上限创建 Store。
    ///
    /// 默认构造器沿用生产写锁上限；本构造器用于注入更小的上限，让
    /// 「超过等待上限后有界失败且不留下幽灵 claim」得到确定性验证，
    /// 而不必空转完整生产超时。
    #[must_use]
    pub fn with_busy_timeout(path: impl Into<PathBuf>, busy_timeout: Duration) -> Self {
        Self::build(path.into(), Some(busy_timeout))
    }

    fn build(path: PathBuf, busy_timeout: Option<Duration>) -> Self {
        let directory = path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let events_root = directory.join(EVENTS_DIR);
        Self {
            control: Arc::new(DatabaseHandle::new(
                directory.join(CONTROL_DATABASE_FILE_NAME),
                schema::SchemaKind::Control,
                busy_timeout,
            )),
            outbox: Arc::new(DatabaseHandle::new(
                directory.join(OUTBOX_DATABASE_FILE_NAME),
                schema::SchemaKind::Outbox,
                busy_timeout,
            )),
            events_root,
        }
    }

    /// 从连接池取用一条控制面连接。
    pub(crate) fn connection(&self) -> Result<PooledConnection<'_>, RuntimeStoreError> {
        self.control.connection()
    }

    /// 从连接池取用一条待消费队列连接。
    pub(crate) fn outbox_connection(&self) -> Result<PooledConnection<'_>, RuntimeStoreError> {
        self.outbox.connection()
    }

    /// 在共享连接池上执行一次只读查询。
    ///
    /// 只读仪表读数（车道用量、耗时、工具摘要）走这一层：逐请求 `Connection::open`
    /// 会绕过进程内连接池，每次都重新打开 SQLite 并重建空池，把单次读放大成
    /// 一次文件打开加一组 PRAGMA。
    pub fn with_read_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, RuntimeStoreError> {
        let connection = self.connection()?;
        f(&connection).map_err(|error| RuntimeStoreError::Storage(error.to_string()))
    }

    /// 在待消费队列库上执行一次只读查询。
    pub fn with_outbox_read_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, RuntimeStoreError> {
        let connection = self.outbox_connection()?;
        f(&connection).map_err(|error| RuntimeStoreError::Storage(error.to_string()))
    }

    /// 从 Axon 根目录创建 Store。
    #[must_use]
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        Self::new(root.as_ref().join(CONTROL_DATABASE_FILE_NAME))
    }

    /// 返回控制面库路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.control.path
    }

    /// 返回待消费队列库路径。
    #[must_use]
    pub fn outbox_path(&self) -> &Path {
        &self.outbox.path
    }

    /// 返回 Store 的两个库文件路径（控制面、待消费队列）。
    #[must_use]
    pub fn database_paths(&self) -> [&Path; 2] {
        [&self.control.path, &self.outbox.path]
    }

    /// 会话事件日志根目录。
    #[must_use]
    pub fn events_root(&self) -> &Path {
        &self.events_root
    }

    /// 打开某个会话的事件日志。
    #[must_use]
    pub fn event_log(&self, session_id: &DaemonSessionId) -> axon_eventlog::EventLog {
        axon_eventlog::EventLog::open(&self.events_root, session_id.as_str())
    }

    /// 读取会话累计指标；不存在时返回 `None`。
    pub fn session_metrics(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<session_metrics_repository::SessionMetricsSnapshot>, RuntimeStoreError> {
        session_metrics_repository::read_metrics(self, session_id)
    }

    /// 回收仍标记为 running、但对应进程已经退出的 Host lease。
    ///
    /// 服务被 launchd/systemd 强制停止时，进程可能来不及执行持久化清理；
    /// 启动新 Host 前回收这些记录，避免等待完整 lease TTL。
    pub fn reap_dead_hosts(&self, now_ms: u64) -> Result<u32, RuntimeStoreError> {
        runtime_repository::reap_dead_hosts(self, now_ms)
    }

    /// 在空闲窗口把两个库的 WAL 回写主库。
    ///
    /// `PASSIVE` 不阻塞并发读写：能回写的帧立即回写，拿不到干净窗口就把剩余部分留给
    /// 下一次。没有这一步，持续写入会让 WAL 漂到与主库同量级（实测 708 MiB / 703 MiB），
    /// 随后一次全量回写独占写锁数十秒，所有写者按 `busy_timeout` 排队失败。
    pub fn checkpoint_wal(&self) -> Result<WalCheckpoint, RuntimeStoreError> {
        let control = self.control.checkpoint_wal()?;
        // 队列库的回写失败不影响控制面结论：它的体量有界，下一个维护周期会再试。
        let _ = self.outbox.checkpoint_wal();
        Ok(control)
    }

    /// 事件正文归档根目录：`<库目录>/archive/session-events`。
    ///
    /// 与网关事件归档同处 `data/archive` 下，便于运维按一个目录做备份与清理。
    #[must_use]
    pub fn event_archive_root(&self) -> PathBuf {
        let parent = self.control.path.parent().unwrap_or(Path::new("."));
        parent
            .join(EVENT_ARCHIVE_DIR)
            .join(SESSION_EVENT_ARCHIVE_SUBDIR)
    }
}

/// 事件正文归档的目录层级：`data/archive/session-events`。
const EVENT_ARCHIVE_DIR: &str = "archive";
/// 事件正文归档在 `data/archive/` 下的子目录名；供清理任务排除，避免按文件保留期误删。
pub const SESSION_EVENT_ARCHIVE_SUBDIR: &str = "session-events";
/// 外置正文文件缺失时回放产出的占位事件类型：保持游标与分页完整，不拖垮整条车道。
pub const SESSION_EVENT_MISSING_ARCHIVE_TYPE: &str = "archive_missing";

impl EventLogStore for SqliteRuntimeStore {
    fn append_event(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        event_type: &str,
        payload_json: &str,
        now_ms: u64,
    ) -> Result<SessionEventRecord, RuntimeStoreError> {
        let batch = [(event_type.to_owned(), payload_json.to_owned())];
        let mut records =
            event_log_repository::append_events(self, session_id, host, &batch, now_ms, None)?;
        records
            .pop()
            .ok_or_else(|| RuntimeStoreError::Storage("事件未写入".to_owned()))
    }

    fn append_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        event_log_repository::append_events(self, session_id, host, events, now_ms, None)
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
        event_log_repository::append_events(
            self,
            session_id,
            host,
            events,
            now_ms,
            Some((interaction_id, lease_token)),
        )
    }
    fn replay_events(
        &self,
        session_id: &DaemonSessionId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        event_log_repository::replay_events(self, session_id, after_sequence, limit)
    }

    fn interaction_tail_window(
        &self,
        session_id: &DaemonSessionId,
        interaction_id: &InteractionId,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError> {
        event_log_repository::interaction_tail_window(self, session_id, interaction_id, limit)
    }
}
#[cfg(test)]
#[path = "sqlite_runtime_store_metrics_tests.rs"]
mod metrics_tests;
