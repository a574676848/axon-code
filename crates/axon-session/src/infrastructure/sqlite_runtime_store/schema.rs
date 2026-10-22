//! Runtime SQLite schema 与索引定义。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError, RwLock};

use rusqlite::{Connection, TransactionBehavior};

const SCHEMA_IDENTITY: &str = "jarvis-runtime-goal-v1";
/// WAL 模式的 PRAGMA 返回值。
const WAL_JOURNAL_MODE: &str = "wal";

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
    // 首次建库必须落在一个 IMMEDIATE 写事务里。DDL 批次很长，且 identity 行在批次
    // **末尾**才写入：并发首次打开（本机 run、daemon、子代理执行面同时打开同一个
    // runtime 库）时，后来的连接会进入「已建部分表、尚未写 identity」的窗口，把正常
    // 建库误判为陌生库（`InvalidQuery`），并把半成品库永久留在该状态。IMMEDIATE 使
    // 并发初始化者与普通写者一样按 busy_timeout 排队，建库要么整体可见、要么整体不可见。
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    initialize_schema(&transaction)?;
    transaction.commit()?;
    let normalized = canonical_or_raw(path);
    if let Ok(mut paths) = INITIALIZED_DATABASE_PATHS.write() {
        paths.insert(normalized);
    }
    Ok(())
}

/// 初始化运行时事实源 schema。
pub(super) fn initialize_schema(connection: &Connection) -> rusqlite::Result<()> {
    let identity_table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='jarvis_schema_identity')",
        [],
        |row| row.get(0),
    )?;
    if !identity_table_exists {
        let existing_tables: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if existing_tables > 0 {
            return Err(rusqlite::Error::InvalidQuery);
        }
    } else {
        let identity: String = connection.query_row(
            "SELECT identity FROM jarvis_schema_identity LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        if identity != SCHEMA_IDENTITY {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS daemon_hosts (
            host_id TEXT PRIMARY KEY, generation TEXT NOT NULL, pid INTEGER NOT NULL,
            process_start_id TEXT, state TEXT NOT NULL, lease_expires_at_ms INTEGER NOT NULL,
            created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS daemon_hosts_retention
            ON daemon_hosts(lease_expires_at_ms, host_id);
        CREATE TABLE IF NOT EXISTS session_runtimes (
            session_id TEXT PRIMARY KEY, run_session_id TEXT NOT NULL, root_session_id TEXT NOT NULL,
            cwd TEXT NOT NULL, status TEXT NOT NULL, owner_host_id TEXT, owner_generation TEXT,
            worker_attempt INTEGER NOT NULL, context_version INTEGER NOT NULL,
            last_checkpoint_seq INTEGER NOT NULL, last_event_sequence INTEGER NOT NULL,
            last_activity_at_ms INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS session_runtimes_recovery
            ON session_runtimes(status, owner_host_id, owner_generation);
        CREATE INDEX IF NOT EXISTS session_runtimes_retention
            ON session_runtimes(last_activity_at_ms, status, session_id);
        CREATE INDEX IF NOT EXISTS session_runtimes_owner
            ON session_runtimes(owner_host_id);
        CREATE TABLE IF NOT EXISTS session_events (
            session_id TEXT NOT NULL, sequence INTEGER NOT NULL, generation TEXT NOT NULL,
            event_type TEXT NOT NULL, payload_json TEXT NOT NULL, emitted_at_ms INTEGER NOT NULL,
            PRIMARY KEY(session_id, sequence),
            FOREIGN KEY(session_id) REFERENCES session_runtimes(session_id)
        );
        CREATE INDEX IF NOT EXISTS session_events_retention
            ON session_events(emitted_at_ms, session_id, sequence);
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
        CREATE TABLE IF NOT EXISTS runtime_schedules (
            schedule_id TEXT PRIMARY KEY, session_id TEXT, kind TEXT NOT NULL,
            expression TEXT NOT NULL, payload_json TEXT NOT NULL, delivery_mode TEXT NOT NULL, status TEXT NOT NULL,
            next_run_at_ms INTEGER, lease_owner TEXT, lease_expires_at_ms INTEGER,
            failure_count INTEGER NOT NULL, retry_at_ms INTEGER, last_run_at_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS runtime_schedules_due
            ON runtime_schedules(status, next_run_at_ms, lease_expires_at_ms);
        CREATE INDEX IF NOT EXISTS runtime_schedules_session
            ON runtime_schedules(session_id);
        CREATE TABLE IF NOT EXISTS daemon_upgrade_admissions (
            admission_id TEXT PRIMARY KEY, handoff_token TEXT NOT NULL UNIQUE,
            target_version TEXT NOT NULL DEFAULT 'unknown',
            old_host_id TEXT NOT NULL, old_generation TEXT NOT NULL,
            deadline_at_ms INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL, state TEXT NOT NULL,
            claimed_by TEXT, claimed_at_ms INTEGER
        );
        CREATE INDEX IF NOT EXISTS daemon_upgrade_admissions_ready
            ON daemon_upgrade_admissions(state, deadline_at_ms);
        CREATE INDEX IF NOT EXISTS daemon_upgrade_admissions_owner
            ON daemon_upgrade_admissions(old_host_id, admission_id);
        CREATE INDEX IF NOT EXISTS daemon_upgrade_admissions_expiry
            ON daemon_upgrade_admissions(deadline_at_ms, admission_id);
        CREATE TABLE IF NOT EXISTS transport_ingress_cursors (
            adapter TEXT NOT NULL, partition_key TEXT NOT NULL, cursor_json TEXT NOT NULL,
            updated_at_ms INTEGER NOT NULL, PRIMARY KEY(adapter, partition_key)
        );
        CREATE TABLE IF NOT EXISTS jarvis_workspaces (
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
        CREATE INDEX IF NOT EXISTS jarvis_workspaces_recent
            ON jarvis_workspaces(last_seen_at_ms DESC, workspace_id);
        CREATE TABLE IF NOT EXISTS jarvis_interactions (
            interaction_id TEXT PRIMARY KEY NOT NULL,
            source_channel TEXT NOT NULL,
            source_message_id TEXT NOT NULL,
            scope_json TEXT NOT NULL,
            audience_json TEXT NOT NULL,
            reply_route_json TEXT NOT NULL,
            content TEXT NOT NULL,
            status TEXT NOT NULL,
            received_at_ms INTEGER NOT NULL,
            lease_token TEXT,
            lease_expires_at_ms INTEGER,
            updated_at_ms INTEGER NOT NULL,
            attachments_json TEXT NOT NULL DEFAULT '[]',
            lane_id TEXT NOT NULL DEFAULT ''
        );
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_interactions_source
            ON jarvis_interactions(source_channel, source_message_id);
        CREATE INDEX IF NOT EXISTS jarvis_interactions_claim
            ON jarvis_interactions(status, lease_expires_at_ms, received_at_ms, interaction_id);
        CREATE INDEX IF NOT EXISTS jarvis_interactions_retention
            ON jarvis_interactions(received_at_ms, status);
        CREATE INDEX IF NOT EXISTS jarvis_interactions_scope_status_recent
            ON jarvis_interactions(scope_json, status, received_at_ms DESC, interaction_id DESC);
        CREATE INDEX IF NOT EXISTS jarvis_interactions_scope_channel_recent
            ON jarvis_interactions(scope_json, source_channel, received_at_ms DESC, interaction_id DESC);
        CREATE INDEX IF NOT EXISTS jarvis_interactions_scope_channel_status_recent
            ON jarvis_interactions(scope_json, source_channel, status, received_at_ms DESC, interaction_id DESC);
        CREATE TABLE IF NOT EXISTS jarvis_goals (
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
        CREATE INDEX IF NOT EXISTS jarvis_goals_active
            ON jarvis_goals(status, updated_at_ms, goal_id);
        CREATE INDEX IF NOT EXISTS jarvis_goals_scope_active
            ON jarvis_goals(scope_json, status, updated_at_ms, goal_id);
        CREATE INDEX IF NOT EXISTS jarvis_goals_priority
            ON jarvis_goals(status, priority DESC, updated_at_ms, goal_id);
        CREATE TABLE IF NOT EXISTS jarvis_executions (
            execution_id TEXT PRIMARY KEY NOT NULL,
            goal_id TEXT NOT NULL,
            session_id TEXT,
            kind TEXT NOT NULL DEFAULT 'turn',
            payload_json TEXT NOT NULL DEFAULT '{}',
            goal_revision INTEGER,
            context_version INTEGER,
            attempt INTEGER NOT NULL,
            priority INTEGER NOT NULL,
            dedupe_key TEXT,
            status TEXT NOT NULL,
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
            FOREIGN KEY(session_id) REFERENCES session_runtimes(session_id)
        );
        DROP INDEX IF EXISTS jarvis_executions_attempt;
        CREATE INDEX IF NOT EXISTS jarvis_executions_goal_history
            ON jarvis_executions(goal_id, created_at_ms DESC, execution_id DESC);
        CREATE INDEX IF NOT EXISTS jarvis_executions_goal_dedupe
            ON jarvis_executions(goal_id, dedupe_key, created_at_ms, execution_id);
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_executions_active_dedupe
            ON jarvis_executions(COALESCE(session_id, goal_id), dedupe_key)
            WHERE dedupe_key IS NOT NULL
              AND status NOT IN ('completed','failed','cancelled');
        CREATE INDEX IF NOT EXISTS jarvis_executions_claim
            ON jarvis_executions(status, priority DESC, created_at_ms, execution_id, retry_at_ms);
        CREATE INDEX IF NOT EXISTS jarvis_executions_session_claim
            ON jarvis_executions(session_id, status, priority DESC, created_at_ms, execution_id, retry_at_ms, lease_expires_at_ms);
        CREATE INDEX IF NOT EXISTS jarvis_executions_stale_running
            ON jarvis_executions(status, priority DESC, created_at_ms, execution_id, lease_expires_at_ms)
            WHERE status='running';
        CREATE INDEX IF NOT EXISTS jarvis_executions_conclusive
            ON jarvis_executions(status, updated_at_ms ASC, execution_id ASC);
        CREATE TABLE IF NOT EXISTS jarvis_results (
            result_id TEXT PRIMARY KEY NOT NULL,
            goal_id TEXT NOT NULL,
            execution_id TEXT NOT NULL,
            succeeded INTEGER NOT NULL,
            summary TEXT NOT NULL,
            receipt_refs_json TEXT NOT NULL,
            artifact_refs_json TEXT NOT NULL,
            error_kind TEXT,
            created_at_ms INTEGER NOT NULL,
            FOREIGN KEY(goal_id) REFERENCES jarvis_goals(goal_id),
            FOREIGN KEY(execution_id) REFERENCES jarvis_executions(execution_id)
        );
        CREATE INDEX IF NOT EXISTS jarvis_results_goal
            ON jarvis_results(goal_id, created_at_ms DESC, result_id);
        CREATE INDEX IF NOT EXISTS jarvis_results_execution
            ON jarvis_results(execution_id);
        CREATE INDEX IF NOT EXISTS jarvis_results_created
            ON jarvis_results(created_at_ms ASC, result_id ASC);
        CREATE TABLE IF NOT EXISTS jarvis_result_reads (
            result_id TEXT PRIMARY KEY NOT NULL,
            acknowledged_at_ms INTEGER NOT NULL,
            acknowledged_source TEXT NOT NULL,
            FOREIGN KEY(result_id) REFERENCES jarvis_results(result_id)
        );
        CREATE TABLE IF NOT EXISTS jarvis_client_presence (
            client_id TEXT PRIMARY KEY NOT NULL,
            workspace_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            destination TEXT NOT NULL,
            registered_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS jarvis_client_presence_workspace
            ON jarvis_client_presence(workspace_id, expires_at_ms DESC);
        CREATE INDEX IF NOT EXISTS jarvis_client_presence_expiry
            ON jarvis_client_presence(expires_at_ms ASC, client_id ASC);
        CREATE TABLE IF NOT EXISTS jarvis_completion_notifications (
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
        CREATE INDEX IF NOT EXISTS jarvis_completion_notifications_state
            ON jarvis_completion_notifications(state, created_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS jarvis_completion_notifications_workspace
            ON jarvis_completion_notifications(workspace_id, state, created_at_ms ASC);
        CREATE INDEX IF NOT EXISTS jarvis_completion_notifications_retention
            ON jarvis_completion_notifications(state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS jarvis_completion_notifications_expiry
            ON jarvis_completion_notifications(expires_at_ms ASC, notification_id ASC)
            WHERE state IN ('pending','dispatched');
        CREATE INDEX IF NOT EXISTS jarvis_completion_notifications_expiry_all
            ON jarvis_completion_notifications(expires_at_ms ASC, notification_id ASC);
        CREATE TABLE IF NOT EXISTS jarvis_completion_deliveries (
            notification_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            destination TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            PRIMARY KEY(notification_id, channel, destination),
            FOREIGN KEY(notification_id) REFERENCES jarvis_completion_notifications(notification_id)
        );
        CREATE INDEX IF NOT EXISTS jarvis_completion_deliveries_inbox
            ON jarvis_completion_deliveries(channel, state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS jarvis_completion_deliveries_target_inbox
            ON jarvis_completion_deliveries(channel, destination, state, updated_at_ms ASC, notification_id ASC);
        CREATE INDEX IF NOT EXISTS jarvis_completion_deliveries_retention
            ON jarvis_completion_deliveries(state, updated_at_ms ASC, notification_id ASC);
        CREATE TABLE IF NOT EXISTS jarvis_agent_messages (
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
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_agent_messages_idempotency
            ON jarvis_agent_messages(idempotency_key);
        CREATE INDEX IF NOT EXISTS jarvis_agent_messages_recipient
            ON jarvis_agent_messages(owner_id, recipient, state, created_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS jarvis_agent_messages_retry
            ON jarvis_agent_messages(state, retry_at_ms, message_id);
        CREATE INDEX IF NOT EXISTS jarvis_agent_messages_retention
            ON jarvis_agent_messages(state, expires_at_ms, message_id);
        CREATE TABLE IF NOT EXISTS jarvis_continuations (
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
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_continuations_idempotency
            ON jarvis_continuations(idempotency_key);
        CREATE INDEX IF NOT EXISTS jarvis_continuations_owner
            ON jarvis_continuations(owner_id, state, created_at_ms, continuation_id);
        CREATE INDEX IF NOT EXISTS jarvis_continuations_child
            ON jarvis_continuations(child_session_id, created_at_ms DESC, continuation_id);
        CREATE INDEX IF NOT EXISTS jarvis_continuations_retention
            ON jarvis_continuations(state, expires_at_ms, continuation_id);
        CREATE TABLE IF NOT EXISTS jarvis_decisions (
            decision_id TEXT PRIMARY KEY NOT NULL,
            scope_json TEXT NOT NULL,
            actor TEXT NOT NULL,
            effect TEXT NOT NULL,
            revision INTEGER NOT NULL,
            expires_at_ms INTEGER,
            created_at_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS jarvis_decisions_scope
            ON jarvis_decisions(effect, expires_at_ms, created_at_ms, decision_id);
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_decisions_scope_revision
            ON jarvis_decisions(scope_json, revision);
        CREATE INDEX IF NOT EXISTS jarvis_decisions_expiry
            ON jarvis_decisions(expires_at_ms)
            WHERE expires_at_ms IS NOT NULL;
        CREATE TABLE IF NOT EXISTS jarvis_events (
            event_id TEXT PRIMARY KEY NOT NULL,
            sequence INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            idempotency_key TEXT NOT NULL,
            cause_event_id TEXT,
            payload_json TEXT NOT NULL,
            occurred_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_events_idempotency
            ON jarvis_events(idempotency_key);
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_events_sequence
            ON jarvis_events(sequence);
        CREATE INDEX IF NOT EXISTS jarvis_events_goal_lookup
            ON jarvis_events(
                COALESCE(
                    json_extract(payload_json, '$.goal_id'),
                    json_extract(payload_json, '$.goalId')
                ),
                sequence
            );
        CREATE TABLE IF NOT EXISTS jarvis_user_questions (
            question_id TEXT PRIMARY KEY NOT NULL,
            idempotency_key TEXT NOT NULL,
            lane_id TEXT NOT NULL,
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
        CREATE UNIQUE INDEX IF NOT EXISTS jarvis_user_questions_idempotency
            ON jarvis_user_questions(idempotency_key);
        CREATE INDEX IF NOT EXISTS jarvis_user_questions_lane_state
            ON jarvis_user_questions(lane_id, state, created_at_ms DESC);
        CREATE INDEX IF NOT EXISTS jarvis_user_questions_reconcile
            ON jarvis_user_questions(state, delivered, expires_at_ms);",
    )?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS jarvis_schema_identity (
            identity TEXT PRIMARY KEY NOT NULL
        );",
    )?;
    connection.execute(
        "INSERT OR IGNORE INTO jarvis_schema_identity(identity) VALUES(?1)",
        [SCHEMA_IDENTITY],
    )?;
    let identity: String = connection.query_row(
        "SELECT identity FROM jarvis_schema_identity LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    if identity != SCHEMA_IDENTITY {
        return Err(rusqlite::Error::InvalidQuery);
    }
    migrate_additive_columns(connection)?;
    Ok(())
}

/// 追加式列迁移。
///
/// DDL 使用 `CREATE TABLE IF NOT EXISTS`，既有运行态库不会因 DDL 变更而获得新列。
/// 这里只补齐**新增且可空/带默认值**的列：无损、幂等、不需要重建库，因此不会丢掉
/// 排队中的 Interaction 或 Host lease。schema identity 只在出现不兼容变更时递增。
///
/// 新增列的配套索引必须在本函数内、列补齐之后创建：既有库的 `lane_id` 由本函数
/// 补列，索引若写在 DDL 批次里会因列不存在而整批失败。
fn migrate_additive_columns(connection: &Connection) -> rusqlite::Result<()> {
    add_column_if_missing(
        connection,
        "jarvis_interactions",
        "attachments_json",
        "TEXT NOT NULL DEFAULT '[]'",
    )?;
    add_column_if_missing(
        connection,
        "jarvis_interactions",
        "lane_id",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    // lane 排他领取按 (lane_id, status, lease_expires_at_ms) 定位在途 processing。
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS jarvis_interactions_lane_processing
            ON jarvis_interactions(lane_id, status, lease_expires_at_ms);",
    )
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
        rusqlite::params![table, column],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(());
    }
    // 表名与列名来自本模块的常量字面量，不含外部输入。
    connection.execute_batch(&format!(
        "ALTER TABLE {table} ADD COLUMN {column} {definition}"
    ))
}
