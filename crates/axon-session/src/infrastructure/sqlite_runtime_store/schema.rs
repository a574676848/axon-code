//! 运行态 SQLite schema 与索引定义。
//!
//! 事实源按生命周期拆成两个库，各自一套 DDL 与一个 `PRAGMA user_version`：
//! `control.sqlite3` 承载需要长期保留、需要原子条件更新的控制面状态；
//! `outbox.sqlite3` 承载有界、可按过期时间整批删除的待消费队列。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError, RwLock};

use rusqlite::{Connection, TransactionBehavior};

/// WAL 模式的 PRAGMA 返回值。
const WAL_JOURNAL_MODE: &str = "wal";

/// 建库目标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SchemaKind {
    /// 控制面库。
    Control,
    /// 待消费队列库。
    Outbox,
}

impl SchemaKind {
    /// schema 版本号，写入 `PRAGMA user_version`。
    ///
    /// 任何 schema 变更都必须递增：目标态只认当前版本的库，旧库直接拒绝打开，
    /// 不做补列、不做回填。
    const fn version(self) -> i64 {
        match self {
            Self::Control => 4,
            Self::Outbox => 1,
        }
    }

    const fn ddl(self) -> &'static str {
        match self {
            Self::Control => CONTROL_DDL,
            Self::Outbox => OUTBOX_DDL,
        }
    }
}

static INITIALIZED_DATABASE_PATHS: LazyLock<RwLock<HashSet<PathBuf>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

/// 进程内首次建库的串行点。
///
/// `INITIALIZED_DATABASE_PATHS` 只能证明「已经建好」，无法阻止多个实例在同一瞬间
/// 同时进入建库路径。DDL 批次很长，并发执行会让另一个连接在准备语句时反复撞上写锁
/// 与 schema 变更重载（`SQLITE_BUSY`）。这里用进程级互斥把首次建库串行化：等待者随后
/// 命中全局缓存直接返回，稳态下每个库只真正执行一次 DDL。
static SCHEMA_INIT_LOCK: Mutex<()> = Mutex::new(());

fn canonical_or_raw(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 确保库处于 WAL 模式。
///
/// WAL 由 SQLite 持久记录在库头，切换成功后由后续所有连接继承；本函数只在首次建库
/// 路径（已持有进程内建库互斥）调用一次，因此不会与并发读者争抢独占锁。
fn ensure_wal_journal_mode(connection: &Connection) -> rusqlite::Result<()> {
    let current: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if current.eq_ignore_ascii_case(WAL_JOURNAL_MODE) {
        return Ok(());
    }
    // 切换后的取值由 SQLite 保证（成功为 "wal"，失败为原模式），这里只关心是否报错。
    let _switched: String =
        connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    Ok(())
}

pub(super) fn is_schema_initialized_for_path(path: &Path) -> bool {
    let normalized = canonical_or_raw(path);
    INITIALIZED_DATABASE_PATHS
        .read()
        .map(|paths| paths.contains(&normalized))
        .unwrap_or(false)
}

pub(super) fn initialize_schema_for_path(
    path: &Path,
    kind: SchemaKind,
    connection: &mut Connection,
) -> rusqlite::Result<()> {
    let _guard = SCHEMA_INIT_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // 等待期间库可能已由别的实例建好：此时直接复用，不再进入 DDL。
    if is_schema_initialized_for_path(path) {
        return Ok(());
    }
    // WAL 是库级持久属性，且切换需要独占锁：只在首次建库、并已持有进程内建库互斥时
    // 执行一次。逐连接重复切换会在并发读流下反复争抢独占锁（实测 busy_timeout 到期仍
    // 以 `SQLITE_BUSY` 失败），而稳态下的新连接只需一次无锁的模式读取即可确认。
    ensure_wal_journal_mode(connection)?;
    // 首次建库必须落在一个 IMMEDIATE 写事务里。DDL 批次很长，且 `user_version` 在批次
    // **末尾**才写入：并发首次打开（本机 run、daemon、子代理执行面同时打开同一个库）时，
    // 后来的连接会进入「已建部分表、尚未写版本」的窗口，把正常建库误判为陌生库
    // （`InvalidQuery`），并把半成品库永久留在该状态。IMMEDIATE 使并发初始化者与普通
    // 写者一样按 busy_timeout 排队，建库要么整体可见、要么整体不可见。
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    initialize_schema(&transaction, kind)?;
    transaction.commit()?;
    let normalized = canonical_or_raw(path);
    if let Ok(mut paths) = INITIALIZED_DATABASE_PATHS.write() {
        paths.insert(normalized);
    }
    Ok(())
}

/// 初始化单个库的 schema。
pub(super) fn initialize_schema(connection: &Connection, kind: SchemaKind) -> rusqlite::Result<()> {
    let existing_tables: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if existing_tables > 0 && version != kind.version() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    // 版本一致时仍然执行 DDL：全部语句都是 `CREATE ... IF NOT EXISTS`，既有库靠这一步
    // 补上新增的表与索引，空库靠这一步建库。版本号只拦不兼容变更，不承担建库语义。
    connection.execute_batch(kind.ddl())?;
    // `user_version` 不支持参数绑定，取值来自本模块常量。
    connection.execute_batch(&format!("PRAGMA user_version = {}", kind.version()))?;
    Ok(())
}

/// 控制面库：长期保留的状态机、目标与账本。
const CONTROL_DDL: &str = "
        CREATE TABLE IF NOT EXISTS daemon_instances (
            host_id TEXT PRIMARY KEY, generation TEXT NOT NULL, pid INTEGER NOT NULL,
            process_start_id TEXT, state TEXT NOT NULL, lease_expires_at_ms INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS daemon_instances_retention
            ON daemon_instances(lease_expires_at_ms, host_id);
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY, run_session_id TEXT NOT NULL, root_session_id TEXT NOT NULL,
            cwd TEXT NOT NULL, status TEXT NOT NULL, owner_host_id TEXT, owner_generation TEXT,
            worker_attempt INTEGER NOT NULL, context_version INTEGER NOT NULL,
            last_checkpoint_seq INTEGER NOT NULL, last_event_sequence INTEGER NOT NULL,
            last_activity_at_ms INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            workspace_id TEXT, title TEXT,
            pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
            run_generation INTEGER NOT NULL DEFAULT 0,
            provider TEXT, model TEXT, model_name TEXT
        );
        CREATE INDEX IF NOT EXISTS sessions_recovery
            ON sessions(status, owner_host_id, owner_generation);
        CREATE INDEX IF NOT EXISTS sessions_retention
            ON sessions(last_activity_at_ms, status, session_id);
        CREATE INDEX IF NOT EXISTS sessions_owner
            ON sessions(owner_host_id);
        CREATE INDEX IF NOT EXISTS sessions_by_activity
            ON sessions(pinned DESC, last_activity_at_ms DESC);
        CREATE TABLE IF NOT EXISTS session_metrics (
            session_id TEXT PRIMARY KEY,
            turn_count INTEGER NOT NULL DEFAULT 0,
            turn_succeeded INTEGER NOT NULL DEFAULT 0,
            turn_failed INTEGER NOT NULL DEFAULT 0,
            turn_interrupted INTEGER NOT NULL DEFAULT 0,
            total_active_ms INTEGER NOT NULL DEFAULT 0,
            total_provider_ms INTEGER NOT NULL DEFAULT 0,
            total_tool_ms INTEGER NOT NULL DEFAULT 0,
            max_turn_ms INTEGER NOT NULL DEFAULT 0,
            ttft_sum_ms INTEGER NOT NULL DEFAULT 0,
            ttft_samples INTEGER NOT NULL DEFAULT 0,
            ttft_max_ms INTEGER NOT NULL DEFAULT 0,
            ttft_histogram_json TEXT NOT NULL DEFAULT '[]',
            turn_histogram_json TEXT NOT NULL DEFAULT '[]',
            channel_mix_json TEXT NOT NULL DEFAULT '{}',
            total_input_tokens INTEGER NOT NULL DEFAULT 0,
            total_output_tokens INTEGER NOT NULL DEFAULT 0,
            total_reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            total_cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            total_cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            total_tool_calls INTEGER NOT NULL DEFAULT 0,
            jobs_started INTEGER NOT NULL DEFAULT 0,
            jobs_completed INTEGER NOT NULL DEFAULT 0,
            jobs_failed INTEGER NOT NULL DEFAULT 0,
            jobs_running INTEGER NOT NULL DEFAULT 0,
            first_turn_at_ms INTEGER,
            last_turn_at_ms INTEGER,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS session_checkpoints (
            session_id TEXT NOT NULL,
            checkpoint_seq INTEGER NOT NULL,
            action_id TEXT NOT NULL,
            context_version INTEGER NOT NULL,
            tool_receipt_sequence INTEGER NOT NULL,
            goal_revision INTEGER,
            token_usage INTEGER,
            time_usage_ms INTEGER,
            agent_tree_summary TEXT,
            transcript_ref TEXT,
            created_at_ms INTEGER NOT NULL,
            PRIMARY KEY(session_id, checkpoint_seq)
        );
        CREATE INDEX IF NOT EXISTS session_checkpoints_latest
            ON session_checkpoints(session_id, checkpoint_seq DESC);
        CREATE TABLE IF NOT EXISTS schedules (
            schedule_id TEXT PRIMARY KEY, session_id TEXT, kind TEXT NOT NULL,
            expression TEXT NOT NULL, payload_json TEXT NOT NULL, delivery_mode TEXT NOT NULL, status TEXT NOT NULL,
            next_run_at_ms INTEGER, lease_owner TEXT, lease_expires_at_ms INTEGER,
            failure_count INTEGER NOT NULL, retry_at_ms INTEGER, last_run_at_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS schedules_due
            ON schedules(status, next_run_at_ms, lease_expires_at_ms);
        CREATE INDEX IF NOT EXISTS schedules_session
            ON schedules(session_id);
        CREATE TABLE IF NOT EXISTS upgrade_handoffs (
            admission_id TEXT PRIMARY KEY, handoff_token TEXT NOT NULL UNIQUE,
            target_version TEXT NOT NULL DEFAULT 'unknown',
            old_host_id TEXT NOT NULL, old_generation TEXT NOT NULL,
            deadline_at_ms INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL, state TEXT NOT NULL,
            claimed_by TEXT, claimed_at_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS upgrade_handoffs_ready
            ON upgrade_handoffs(state, deadline_at_ms);
        CREATE INDEX IF NOT EXISTS upgrade_handoffs_owner
            ON upgrade_handoffs(old_host_id, admission_id);
        CREATE INDEX IF NOT EXISTS upgrade_handoffs_expiry
            ON upgrade_handoffs(deadline_at_ms, admission_id);
        CREATE TABLE IF NOT EXISTS ingress_cursors (
            adapter TEXT NOT NULL, partition_key TEXT NOT NULL, cursor_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL, PRIMARY KEY(adapter, partition_key)
        );
        CREATE TABLE IF NOT EXISTS workspaces (
            workspace_id TEXT PRIMARY KEY NOT NULL,
            display_name TEXT NOT NULL,
            workspace_root TEXT NOT NULL,
            git_remote TEXT,
            default_branch TEXT,
            aliases_json TEXT NOT NULL,
            memory_namespace TEXT NOT NULL,
            pinned INTEGER NOT NULL,
            last_seen_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS workspaces_recent
            ON workspaces(last_seen_at_ms DESC, workspace_id);
        CREATE TABLE IF NOT EXISTS inbox (
            interaction_id TEXT PRIMARY KEY NOT NULL,
            source_channel TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            scope_json TEXT NOT NULL,
            audience_json TEXT NOT NULL,
            reply_route_json TEXT NOT NULL,
            content TEXT NOT NULL,
            received_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            attachments_json TEXT NOT NULL DEFAULT '[]',
            model TEXT,
            reasoning_effort TEXT,
            session_id TEXT NOT NULL DEFAULT '',
            tool_summary TEXT NOT NULL DEFAULT ''
        );
        CREATE UNIQUE INDEX IF NOT EXISTS inbox_source
            ON inbox(source_channel, source_message_id);
        CREATE INDEX IF NOT EXISTS inbox_retention
            ON inbox(received_at_ms, interaction_id);
        CREATE INDEX IF NOT EXISTS inbox_scope_recent
            ON inbox(scope_json, received_at_ms DESC, interaction_id DESC);
        CREATE INDEX IF NOT EXISTS inbox_scope_channel_recent
            ON inbox(scope_json, source_channel, received_at_ms DESC, interaction_id DESC);
        CREATE INDEX IF NOT EXISTS inbox_session_recent
            ON inbox(session_id, received_at_ms DESC, interaction_id DESC);
        CREATE TABLE IF NOT EXISTS resume_pointer (
            singleton_key TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );        CREATE TABLE IF NOT EXISTS goals (
            goal_id TEXT PRIMARY KEY NOT NULL,
            scope_json TEXT NOT NULL,
            parent_goal_id TEXT,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            revision INTEGER NOT NULL,
            plan_json TEXT NOT NULL,
            acceptance_json TEXT NOT NULL,
            progress_json TEXT,
            waiting_reason TEXT,
            next_step TEXT,
            priority INTEGER NOT NULL DEFAULT 0,
            deadline_ms INTEGER,
            created_from_interaction_id TEXT,
            cancellation_reason TEXT,
            cancellation_requested_at_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS goals_active
            ON goals(status, updated_at_ms, goal_id);
        CREATE INDEX IF NOT EXISTS goals_scope_active
            ON goals(scope_json, status, updated_at_ms, goal_id);
        CREATE INDEX IF NOT EXISTS goals_priority
            ON goals(status, priority DESC, updated_at_ms, goal_id);
        CREATE TABLE IF NOT EXISTS work_items (
            execution_id TEXT PRIMARY KEY NOT NULL,
            goal_id TEXT,
            session_id TEXT,
            kind TEXT NOT NULL DEFAULT 'turn',
            payload_json TEXT NOT NULL DEFAULT '{}',
            goal_revision INTEGER,
            context_version INTEGER,
            attempt INTEGER NOT NULL,
            priority INTEGER NOT NULL,
            dedupe_key TEXT,
            inbox_id TEXT,
            terminal_kind TEXT,
            status TEXT NOT NULL,
            lease_token TEXT,
            lease_json TEXT,
            claimed_by TEXT,
            claimed_generation TEXT,
            lease_expires_at_ms INTEGER,
            started_at_ms INTEGER,
            committed_at_ms INTEGER,
            cancelled_at_ms INTEGER,
            retry_at_ms INTEGER,
            checkpoint_json TEXT,
            cancellation_requested_at_ms INTEGER,
            cancellation_reason TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            FOREIGN KEY(session_id) REFERENCES sessions(session_id)
        );
        CREATE TABLE IF NOT EXISTS work_item_history (
            work_id TEXT PRIMARY KEY NOT NULL,
            goal_id TEXT,
            session_id TEXT,
            kind TEXT NOT NULL DEFAULT 'turn',
            payload_json TEXT NOT NULL DEFAULT '{}',
            goal_revision INTEGER,
            context_version INTEGER,
            attempt INTEGER NOT NULL,
            priority INTEGER NOT NULL,
            dedupe_key TEXT,
            inbox_id TEXT,
            terminal_kind TEXT NOT NULL,
            status TEXT NOT NULL,
            started_at_ms INTEGER,
            committed_at_ms INTEGER,
            cancelled_at_ms INTEGER,
            checkpoint_json TEXT,
            cancellation_reason TEXT,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            finished_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS work_item_history_goal
            ON work_item_history(goal_id, finished_at_ms DESC, work_id DESC);
        CREATE INDEX IF NOT EXISTS work_item_history_session
            ON work_item_history(session_id, finished_at_ms DESC, work_id DESC);
        CREATE INDEX IF NOT EXISTS work_item_history_inbox
            ON work_item_history(inbox_id);
        CREATE INDEX IF NOT EXISTS work_item_history_terminal
            ON work_item_history(terminal_kind);
        CREATE INDEX IF NOT EXISTS work_item_history_status
            ON work_item_history(status);
        CREATE VIEW IF NOT EXISTS work_items_all AS
            SELECT execution_id, goal_id, session_id, kind, payload_json, goal_revision,
                   context_version, attempt, priority, dedupe_key, inbox_id, terminal_kind,
                   status, lease_token, lease_json, claimed_by, claimed_generation,
                   lease_expires_at_ms, started_at_ms, committed_at_ms, cancelled_at_ms,
                   retry_at_ms, checkpoint_json, cancellation_requested_at_ms,
                   cancellation_reason, created_at_ms, updated_at_ms
            FROM work_items
            UNION ALL
            SELECT work_id AS execution_id, goal_id, session_id, kind, payload_json,
                   goal_revision, context_version, attempt, priority, dedupe_key, inbox_id,
                   terminal_kind, status, NULL, NULL, NULL, NULL, NULL, started_at_ms,
                   committed_at_ms, cancelled_at_ms, NULL, checkpoint_json, NULL,
                   cancellation_reason, created_at_ms, updated_at_ms
            FROM work_item_history;       CREATE INDEX IF NOT EXISTS work_items_goal_history
            ON work_items(goal_id, created_at_ms DESC, execution_id DESC);
        CREATE INDEX IF NOT EXISTS work_items_goal_dedupe
            ON work_items(goal_id, dedupe_key, created_at_ms, execution_id);
        CREATE UNIQUE INDEX IF NOT EXISTS work_items_active_dedupe
            ON work_items(COALESCE(session_id, goal_id), dedupe_key)
            WHERE dedupe_key IS NOT NULL
              AND status NOT IN ('completed','failed','cancelled');
        CREATE INDEX IF NOT EXISTS work_items_claim
            ON work_items(status, priority DESC, created_at_ms, execution_id, retry_at_ms);
        CREATE INDEX IF NOT EXISTS work_items_session_claim
            ON work_items(session_id, status, priority DESC, created_at_ms, execution_id, retry_at_ms, lease_expires_at_ms);
        CREATE INDEX IF NOT EXISTS work_items_stale_running
            ON work_items(status, priority DESC, created_at_ms, execution_id, lease_expires_at_ms)
            WHERE status='running';
        CREATE UNIQUE INDEX IF NOT EXISTS work_items_inbox
            ON work_items(inbox_id)
            WHERE inbox_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS work_items_inbox_state
            ON work_items(inbox_id, status, lease_expires_at_ms);
        CREATE INDEX IF NOT EXISTS work_items_terminal
            ON work_items(terminal_kind, updated_at_ms)
            WHERE terminal_kind IS NOT NULL;
        CREATE INDEX IF NOT EXISTS work_items_conclusive
            ON work_items(status, updated_at_ms ASC, execution_id ASC);
        CREATE TABLE IF NOT EXISTS goal_results (
            result_id TEXT PRIMARY KEY NOT NULL,
            goal_id TEXT NOT NULL,
            execution_id TEXT NOT NULL,
            succeeded INTEGER NOT NULL,
            summary TEXT NOT NULL,
            receipt_refs_json TEXT NOT NULL,
            artifact_refs_json TEXT NOT NULL,
            error_kind TEXT,
            created_at_ms INTEGER NOT NULL,
            -- 只约束 goal：执行行终结后会搬进 work_item_history，不能再对在途表建外键。
            FOREIGN KEY(goal_id) REFERENCES goals(goal_id)
        );
        CREATE INDEX IF NOT EXISTS goal_results_goal
            ON goal_results(goal_id, created_at_ms DESC, result_id);
        CREATE INDEX IF NOT EXISTS goal_results_execution
            ON goal_results(execution_id);
        CREATE INDEX IF NOT EXISTS goal_results_created
            ON goal_results(created_at_ms ASC, result_id ASC);
        CREATE TABLE IF NOT EXISTS goal_result_reads (
            result_id TEXT PRIMARY KEY NOT NULL,
            acknowledged_at_ms INTEGER NOT NULL,
            acknowledged_source TEXT NOT NULL,
            FOREIGN KEY(result_id) REFERENCES goal_results(result_id)
        );
        CREATE TABLE IF NOT EXISTS decisions (
            decision_id TEXT PRIMARY KEY NOT NULL,
            scope_json TEXT NOT NULL,
            actor TEXT NOT NULL,
            effect TEXT NOT NULL,
            revision INTEGER NOT NULL,
            expires_at_ms INTEGER,
            created_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS decisions_scope
            ON decisions(effect, expires_at_ms, created_at_ms, decision_id);
        CREATE UNIQUE INDEX IF NOT EXISTS decisions_scope_revision
            ON decisions(scope_json, revision);
        CREATE INDEX IF NOT EXISTS decisions_expiry
            ON decisions(expires_at_ms)
            WHERE expires_at_ms IS NOT NULL;
        CREATE TABLE IF NOT EXISTS ledger_events (
            event_id TEXT PRIMARY KEY NOT NULL,
            sequence INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            cause_event_id TEXT,
            payload_json TEXT NOT NULL,
            occurred_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS ledger_events_idempotency
            ON ledger_events(idempotency_key);
        CREATE UNIQUE INDEX IF NOT EXISTS ledger_events_sequence
            ON ledger_events(sequence);
        CREATE INDEX IF NOT EXISTS ledger_events_goal_lookup
            ON ledger_events(
                COALESCE(
                    json_extract(payload_json, '$.goal_id'),
                    json_extract(payload_json, '$.goalId')
                ),
                sequence
            );
";

/// 待消费队列库：有界、按过期时间整批删除的协作事实。
const OUTBOX_DDL: &str = "
        CREATE TABLE IF NOT EXISTS client_presence (
            client_id TEXT PRIMARY KEY NOT NULL,
            workspace_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            destination TEXT NOT NULL,
            registered_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS client_presence_workspace
            ON client_presence(workspace_id, expires_at_ms DESC);
        CREATE INDEX IF NOT EXISTS client_presence_expiry
            ON client_presence(expires_at_ms ASC, client_id ASC);
        CREATE TABLE IF NOT EXISTS notifications (
            notification_id TEXT PRIMARY KEY NOT NULL,
            workspace_id TEXT NOT NULL,
            goal_id TEXT NOT NULL,
            execution_id TEXT NOT NULL UNIQUE,
            content TEXT NOT NULL,
            summary TEXT NOT NULL,
            result_ids_json TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS notifications_state
            ON notifications(state, created_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notifications_workspace
            ON notifications(workspace_id, state, created_at_ms ASC);
        CREATE INDEX IF NOT EXISTS notifications_retention
            ON notifications(state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notifications_expiry
            ON notifications(expires_at_ms ASC, notification_id ASC)
            WHERE state IN ('pending','dispatched');
        CREATE INDEX IF NOT EXISTS notifications_expiry_all
            ON notifications(expires_at_ms ASC, notification_id ASC);
        CREATE TABLE IF NOT EXISTS notification_deliveries (
            notification_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            destination TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY(notification_id, channel, destination),
            FOREIGN KEY(notification_id) REFERENCES notifications(notification_id)
        );
        CREATE INDEX IF NOT EXISTS notification_deliveries_inbox
            ON notification_deliveries(channel, state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notification_deliveries_target_inbox
            ON notification_deliveries(channel, destination, state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS notification_deliveries_retention
            ON notification_deliveries(state, updated_at_ms ASC, notification_id ASC);
        CREATE TABLE IF NOT EXISTS agent_messages (
            message_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            author TEXT NOT NULL,
            recipient TEXT NOT NULL,
            kind TEXT NOT NULL,
            content TEXT NOT NULL,
            trigger_turn INTEGER NOT NULL,
            continuation_id TEXT,
            state TEXT NOT NULL,
            attempt_count INTEGER NOT NULL,
            last_attempt_at_ms INTEGER,
            retry_at_ms INTEGER,
            last_error TEXT,
            delivered_at_ms INTEGER,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS agent_messages_idempotency
            ON agent_messages(idempotency_key);
        CREATE INDEX IF NOT EXISTS agent_messages_recipient
            ON agent_messages(owner_id, recipient, state, created_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS agent_messages_retry
            ON agent_messages(state, retry_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS agent_messages_retention
            ON agent_messages(state, expires_at_ms, message_id);
        CREATE TABLE IF NOT EXISTS continuations (
            continuation_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            owner_id TEXT NOT NULL,
            parent_session_id TEXT NOT NULL,
            child_session_id TEXT NOT NULL,
            agent_path TEXT NOT NULL,
            job_ref TEXT,
            execution_ref TEXT,
            terminal_kind TEXT NOT NULL,
            exit_reason TEXT NOT NULL,
            summary TEXT NOT NULL,
            artifact_path TEXT,
            token_input INTEGER,
            token_output INTEGER,
            token_reasoning INTEGER,
            state TEXT NOT NULL,
            consumed_at_ms INTEGER,
            consumed_by TEXT,
            attempt_count INTEGER NOT NULL,
            last_attempt_at_ms INTEGER,
            retry_at_ms INTEGER,
            last_error TEXT,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS continuations_idempotency
            ON continuations(idempotency_key);
        CREATE INDEX IF NOT EXISTS continuations_owner
            ON continuations(owner_id, state, created_at_ms, continuation_id);
        CREATE INDEX IF NOT EXISTS continuations_child
            ON continuations(child_session_id, created_at_ms DESC, continuation_id);
        CREATE INDEX IF NOT EXISTS continuations_retention
            ON continuations(state, expires_at_ms, continuation_id);
        CREATE TABLE IF NOT EXISTS user_questions (
            question_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            session_id TEXT NOT NULL,
            interaction_id TEXT NOT NULL,
            turn_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            questions_json TEXT NOT NULL,
            handoff_text TEXT,
            state TEXT NOT NULL,
            deadline_at_ms INTEGER NOT NULL,
            answer_json TEXT,
            answered_at_ms INTEGER,
            answered_channel TEXT,
            delivered INTEGER NOT NULL DEFAULT 0,
            delivery_mode TEXT,
            created_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS user_questions_idempotency
            ON user_questions(idempotency_key);
        CREATE INDEX IF NOT EXISTS user_questions_session_state
            ON user_questions(session_id, state, created_at_ms DESC);
        CREATE INDEX IF NOT EXISTS user_questions_reconcile
            ON user_questions(state, delivered, expires_at_ms);
";
