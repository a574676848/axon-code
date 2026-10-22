//! Daemon runtime SQLite 的 R1~R7 保留清理。
//!
//! 会话清理定时任务必须覆盖目标态的全部运行态事实：Interaction、
//! Task、Execution、Result、Decision、Event、Session Window、调度、宿主与
//! 升级准入。删除按有界批次推进，整轮另受墙上时钟预算约束：预算耗尽即停止，
//! 剩余数据由下一轮 Cron 幂等继续，避免清理长时间争抢 Daemon 的写锁。

use std::{
    path::Path,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params_from_iter};

use super::runtime_cleanup_counts::count_expired_rows;
use super::runtime_cleanup_families::{
    delete_expired_agent_messages, delete_expired_client_presence, delete_expired_continuations,
    delete_expired_daemon_hosts, delete_expired_decisions, delete_expired_events,
    delete_expired_session_events, delete_expired_upgrade_admissions,
    delete_expired_user_questions, delete_session_families_by_ids,
    delete_settled_completion_deliveries, delete_settled_completion_notifications,
    delete_terminal_goals, delete_terminal_interactions,
};

const RUNTIME_DATABASE_FILE: &str = "runtime.sqlite3";
/// 单批删除的行数上限：直接决定单次 IMMEDIATE 事务的写锁持有时间。
const RUNTIME_CLEANUP_BATCH_LIMIT: usize = 256;
/// 单轮清理的墙上时钟预算：超出后停止本轮，剩余过期数据由下轮 Cron 继续。
const RUNTIME_CLEANUP_TIME_BUDGET: Duration = Duration::from_secs(20);
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const INCREMENTAL_VACUUM_PAGE_BATCH: u32 = 256;

/// 终态 Interaction 状态：只有终态记录参与按保留期回收。
pub(super) const INTERACTION_TERMINAL_STATUSES: [&str; 2] = ["understood", "rejected"];
/// 终态 Task 状态：长期承诺结束后才允许回收。
pub(super) const TASK_TERMINAL_STATUSES: [&str; 3] = ["completed", "failed", "cancelled"];
/// 活跃 Execution 状态：保护其 Task 与 Session Window 不被回收。
pub(super) const EXECUTION_ACTIVE_STATUSES: [&str; 3] = ["queued", "waiting", "running"];
/// 活跃 Session 状态：仍在推进的 Session Window 不参与回收。
pub(super) const SESSION_ACTIVE_STATUSES: [&str; 5] =
    ["running", "compacting", "waiting", "recovering", "draining"];
/// Event 保底保留条数：即使全部超出保留期也保留最新一段，避免 sequence 回退。
pub(super) const LEDGER_EVENT_MIN_RETAINED: i64 = 1_000;
/// 活跃 Session Event 保底保留条数：避免高频 Workspace 车道无限增长。
pub(super) const SESSION_EVENT_MIN_RETAINED: i64 = 1_000;

/// 一次运行态清理的分表计数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RuntimeCleanupCounts {
    pub(crate) sessions: usize,
    pub(crate) session_events: usize,
    pub(crate) session_checkpoints: usize,
    pub(crate) runtime_schedules: usize,
    pub(crate) interactions: usize,
    pub(crate) goals: usize,
    pub(crate) executions: usize,
    pub(crate) results: usize,
    pub(crate) result_reads: usize,
    pub(crate) decisions: usize,
    pub(crate) events: usize,
    pub(crate) daemon_hosts: usize,
    pub(crate) upgrade_admissions: usize,
    pub(crate) client_presence: usize,
    pub(crate) completion_notifications: usize,
    pub(crate) completion_deliveries: usize,
    pub(crate) agent_messages: usize,
    pub(crate) continuations: usize,
    pub(crate) user_questions: usize,
}

impl RuntimeCleanupCounts {
    pub(super) fn merge(&mut self, other: Self) {
        self.sessions = self.sessions.saturating_add(other.sessions);
        self.session_events = self.session_events.saturating_add(other.session_events);
        self.session_checkpoints = self
            .session_checkpoints
            .saturating_add(other.session_checkpoints);
        self.runtime_schedules = self
            .runtime_schedules
            .saturating_add(other.runtime_schedules);
        self.interactions = self.interactions.saturating_add(other.interactions);
        self.goals = self.goals.saturating_add(other.goals);
        self.executions = self.executions.saturating_add(other.executions);
        self.results = self.results.saturating_add(other.results);
        self.result_reads = self.result_reads.saturating_add(other.result_reads);
        self.decisions = self.decisions.saturating_add(other.decisions);
        self.events = self.events.saturating_add(other.events);
        self.daemon_hosts = self.daemon_hosts.saturating_add(other.daemon_hosts);
        self.upgrade_admissions = self
            .upgrade_admissions
            .saturating_add(other.upgrade_admissions);
        self.client_presence = self.client_presence.saturating_add(other.client_presence);
        self.completion_notifications = self
            .completion_notifications
            .saturating_add(other.completion_notifications);
        self.completion_deliveries = self
            .completion_deliveries
            .saturating_add(other.completion_deliveries);
        self.agent_messages = self.agent_messages.saturating_add(other.agent_messages);
        self.continuations = self.continuations.saturating_add(other.continuations);
        self.user_questions = self.user_questions.saturating_add(other.user_questions);
    }

    /// 是否发生了任何行删除。
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 按保留期清理运行态事实，返回分表删除计数。
pub(crate) fn cleanup_runtime_data(
    root: &Path,
    cutoff_ms: u64,
) -> Result<RuntimeCleanupCounts, String> {
    run_runtime_cleanup(root, cutoff_ms, false)
}

/// 只统计将要删除的行，不产生任何写入。
pub(crate) fn count_runtime_cleanup(
    root: &Path,
    cutoff_ms: u64,
) -> Result<RuntimeCleanupCounts, String> {
    run_runtime_cleanup(root, cutoff_ms, true)
}

fn run_runtime_cleanup(
    root: &Path,
    cutoff_ms: u64,
    dry_run: bool,
) -> Result<RuntimeCleanupCounts, String> {
    run_runtime_cleanup_with_budget(root, cutoff_ms, dry_run, RUNTIME_CLEANUP_TIME_BUDGET)
}

/// 带显式时间预算的清理入口：容量验收与预算截断测试直接复用。
fn run_runtime_cleanup_with_budget(
    root: &Path,
    cutoff_ms: u64,
    dry_run: bool,
    time_budget: Duration,
) -> Result<RuntimeCleanupCounts, String> {
    let path = root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(RUNTIME_DATABASE_FILE);
    if !path.exists() {
        return Ok(RuntimeCleanupCounts::default());
    }
    let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
    let mut connection = open_database(&path)?;
    if dry_run {
        return count_expired_rows(&connection, cutoff_ms);
    }
    let deadline = Instant::now() + time_budget;
    let mut counts = RuntimeCleanupCounts::default();
    counts.merge(delete_expired_upgrade_admissions(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_expired_session_events(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_terminal_goals(&mut connection, cutoff_ms, deadline)?);
    counts.merge(delete_terminal_interactions(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_expired_decisions(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_expired_events(&mut connection, cutoff_ms, deadline)?);
    counts.merge(delete_expired_daemon_hosts(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_expired_client_presence(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    // 渠道投递记录通过外键引用通知本体，必须先删派生 delivery，再删 notification。
    counts.merge(delete_settled_completion_deliveries(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_settled_completion_notifications(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    // mailbox 消息与 continuation 是 run 级协作事实：前者 24 小时、后者 7 天的自身
    // 保留窗口先于 Cron 水位到期，因此这里的回收不会删掉仍可投递或仍可装配的事实。
    counts.merge(delete_expired_agent_messages(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_expired_continuations(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    counts.merge(delete_expired_user_questions(
        &mut connection,
        cutoff_ms,
        deadline,
    )?);
    if !counts.is_empty() {
        reclaim_database_space(&connection)?;
    }
    Ok(counts)
}

/// 读取候选中仍活跃或仍被 Execution 引用的 Session。
pub(super) fn protected_runtime_session_ids(
    root: &Path,
    candidate_ids: &[String],
) -> Result<Vec<String>, String> {
    let path = runtime_database_path(root);
    if candidate_ids.is_empty() || !path.exists() {
        return Ok(Vec::new());
    }
    let connection = open_read_only_database(&path)?;
    let mut protected = Vec::new();
    for ids in candidate_ids.chunks(RUNTIME_CLEANUP_BATCH_LIMIT) {
        let placeholders = placeholders(ids.len());
        let active_sql = format!(
            "SELECT session_id FROM session_runtimes
             WHERE session_id IN ({placeholders}) AND status IN ({active})",
            active = quoted_list(&SESSION_ACTIVE_STATUSES),
        );
        let mut statement = connection
            .prepare(&active_sql)
            .map_err(|_| "Daemon runtime 活跃 Session 查询失败".to_owned())?;
        let rows = statement
            .query_map(params_from_iter(ids.iter()), |row| row.get::<_, String>(0))
            .map_err(|_| "Daemon runtime 活跃 Session 查询失败".to_owned())?;
        protected.extend(
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| "Daemon runtime 活跃 Session 读取失败".to_owned())?,
        );

        let execution_sql = format!(
            "SELECT DISTINCT session_id FROM jarvis_executions
             WHERE session_id IN ({placeholders})"
        );
        let mut statement = connection
            .prepare(&execution_sql)
            .map_err(|_| "Daemon runtime Execution 引用查询失败".to_owned())?;
        let rows = statement
            .query_map(params_from_iter(ids.iter()), |row| row.get::<_, String>(0))
            .map_err(|_| "Daemon runtime Execution 引用查询失败".to_owned())?;
        protected.extend(
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| "Daemon runtime Execution 引用读取失败".to_owned())?,
        );
    }
    protected.sort();
    protected.dedup();
    Ok(protected)
}

/// 精确统计 rollout stale IDs 对应的 runtime Session family，不产生写入。
pub(super) fn count_runtime_session_families(
    root: &Path,
    session_ids: &[String],
) -> Result<RuntimeCleanupCounts, String> {
    let path = runtime_database_path(root);
    if session_ids.is_empty() || !path.exists() {
        return Ok(RuntimeCleanupCounts::default());
    }
    let connection = open_read_only_database(&path)?;
    let mut counts = RuntimeCleanupCounts::default();
    for ids in session_ids.chunks(RUNTIME_CLEANUP_BATCH_LIMIT) {
        let placeholders = placeholders(ids.len());
        counts.sessions = counts.sessions.saturating_add(count_ids_in(
            &connection,
            "session_runtimes",
            &placeholders,
            ids,
        )?);
        counts.session_events = counts.session_events.saturating_add(count_ids_in(
            &connection,
            "session_events",
            &placeholders,
            ids,
        )?);
        counts.session_checkpoints = counts.session_checkpoints.saturating_add(count_ids_in(
            &connection,
            "session_checkpoints",
            &placeholders,
            ids,
        )?);
        counts.runtime_schedules = counts.runtime_schedules.saturating_add(count_ids_in(
            &connection,
            "runtime_schedules",
            &placeholders,
            ids,
        )?);
    }
    Ok(counts)
}

/// 统计滚动事件清理与 stale Session family 清理的候选交集。
pub(super) fn count_runtime_session_event_overlap(
    root: &Path,
    session_ids: &[String],
    cutoff_ms: u64,
) -> Result<usize, String> {
    let path = runtime_database_path(root);
    if session_ids.is_empty() || !path.exists() {
        return Ok(0);
    }
    let connection = open_read_only_database(&path)?;
    let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
    let mut count = 0_usize;
    for ids in session_ids.chunks(RUNTIME_CLEANUP_BATCH_LIMIT) {
        let sql = format!(
            "SELECT COUNT(*) FROM session_events
             WHERE emitted_at_ms < ?
               AND session_id IN ({})
               AND EXISTS (
                   SELECT 1 FROM session_runtimes AS retained
                   WHERE retained.session_id = session_events.session_id
                     AND session_events.sequence <= CASE
                         WHEN retained.last_event_sequence > {retained}
                         THEN retained.last_event_sequence - {retained}
                         ELSE 0
                     END
               )",
            placeholders(ids.len()),
            retained = SESSION_EVENT_MIN_RETAINED,
        );
        let mut values = Vec::with_capacity(ids.len().saturating_add(1));
        values.push(rusqlite::types::Value::Integer(cutoff_ms));
        values.extend(ids.iter().cloned().map(rusqlite::types::Value::Text));
        let batch: i64 = connection
            .query_row(&sql, params_from_iter(values.iter()), |row| row.get(0))
            .map_err(|_| "Daemon runtime Session Event 重叠统计失败".to_owned())?;
        count = count.saturating_add(usize::try_from(batch).unwrap_or(usize::MAX));
    }
    Ok(count)
}

/// 按 rollout stale IDs 清理 runtime Session family。
pub(super) fn cleanup_runtime_session_families(
    root: &Path,
    session_ids: &[String],
) -> Result<RuntimeCleanupCounts, String> {
    let path = runtime_database_path(root);
    if session_ids.is_empty() || !path.exists() {
        return Ok(RuntimeCleanupCounts::default());
    }
    let mut connection = open_database(&path)?;
    let deadline = Instant::now() + RUNTIME_CLEANUP_TIME_BUDGET;
    let mut counts = RuntimeCleanupCounts::default();
    for ids in session_ids.chunks(RUNTIME_CLEANUP_BATCH_LIMIT) {
        if Instant::now() >= deadline {
            return Err("Daemon runtime Session family 清理超出时间预算".to_owned());
        }
        counts.merge(delete_session_families_by_ids(&mut connection, ids)?);
    }
    if !counts.is_empty() {
        reclaim_database_space(&connection)?;
    }
    Ok(counts)
}

fn count_ids_in(
    connection: &Connection,
    table: &str,
    placeholders: &str,
    ids: &[String],
) -> Result<usize, String> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE session_id IN ({placeholders})");
    let count: i64 = connection
        .query_row(&sql, params_from_iter(ids.iter()), |row| row.get(0))
        .map_err(|_| format!("Daemon runtime {table} Session family 统计失败"))?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// retained Session Event 滚动保留谓词（`?1` 为保留截止时间）。
///
/// 所有 Session 都按事件年龄和 sequence 水位滚动回收；rollout stale family 随后
/// 会继续删除剩余事件与根记录，二者共享报告计数但不共享候选判定。
pub(super) fn session_event_expiry_filter() -> String {
    format!(
        "FROM session_events
         WHERE emitted_at_ms < ?1
           AND EXISTS (
               SELECT 1 FROM session_runtimes AS retained
               WHERE retained.session_id = session_events.session_id
                 AND session_events.sequence <= CASE
                     WHEN retained.last_event_sequence > {retained}
                     THEN retained.last_event_sequence - {retained}
                     ELSE 0
                 END
           )",
        retained = SESSION_EVENT_MIN_RETAINED,
    )
}

/// Task 保留谓词（`?1` 为保留截止时间）：终态、无活跃 Execution、无未读 Result。
pub(super) fn task_expiry_filter() -> String {
    format!(
        "FROM jarvis_goals
         WHERE status IN ({terminal})
           AND updated_at_ms < ?1
           AND NOT EXISTS (
               SELECT 1 FROM jarvis_executions e
               WHERE e.goal_id = jarvis_goals.goal_id AND e.status IN ({active})
           )
           AND NOT EXISTS (
               SELECT 1 FROM jarvis_results r
               LEFT JOIN jarvis_result_reads rr ON rr.result_id = r.result_id
               WHERE r.goal_id = jarvis_goals.goal_id AND rr.result_id IS NULL
           )",
        terminal = quoted_list(&TASK_TERMINAL_STATUSES),
        active = quoted_list(&EXECUTION_ACTIVE_STATUSES),
    )
}

/// 按有界批次执行「选主键 -> 删依赖 -> 删根」的清理循环。
///
/// 每个族至少推进一批，之后每次提交都会检查墙上时钟预算；预算耗尽立即返回
/// 已删除计数，剩余数据留待下一轮 Cron，从而保证单轮清理时长有界。
pub(super) fn delete_family<F>(
    connection: &mut Connection,
    cutoff_ms: i64,
    deadline: Instant,
    select_sql: &str,
    mut delete_ids: F,
) -> Result<RuntimeCleanupCounts, String>
where
    F: FnMut(&rusqlite::Transaction<'_>, &[String]) -> Result<RuntimeCleanupCounts, String>,
{
    let mut counts = RuntimeCleanupCounts::default();
    let batch = i64::try_from(RUNTIME_CLEANUP_BATCH_LIMIT).unwrap_or(i64::MAX);
    let mut advanced = false;
    loop {
        // 每个族至少推进一批；此后一旦预算耗尽就交还给 Daemon，剩余数据由下一轮 Cron 续跑。
        if advanced && Instant::now() >= deadline {
            break;
        }
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| "Daemon runtime 清理事务启动失败".to_owned())?;
        let ids = {
            let mut statement = transaction
                .prepare_cached(select_sql)
                .map_err(|_| "Daemon runtime 过期记录查询失败".to_owned())?;
            statement
                .query_map([cutoff_ms, batch], |row| row.get::<_, String>(0))
                .map_err(|_| "Daemon runtime 过期记录查询失败".to_owned())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| "Daemon runtime 过期记录读取失败".to_owned())?
        };
        if ids.is_empty() {
            transaction
                .rollback()
                .map_err(|_| "Daemon runtime 清理事务回滚失败".to_owned())?;
            break;
        }
        counts.merge(delete_ids(&transaction, &ids)?);
        transaction
            .commit()
            .map_err(|_| "Daemon runtime 清理事务提交失败".to_owned())?;
        advanced = true;
    }
    Ok(counts)
}

pub(super) fn delete_ids_in(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    placeholders: &str,
    ids: &[String],
) -> Result<usize, String> {
    let sql = format!("DELETE FROM {table} WHERE {column} IN ({placeholders})");
    let mut statement = transaction
        .prepare_cached(&sql)
        .map_err(|_| format!("Daemon runtime {table} 清理失败"))?;
    statement
        .execute(params_from_iter(ids.iter()))
        .map_err(|_| format!("Daemon runtime {table} 清理失败"))
}

pub(super) fn count_rows(
    connection: &Connection,
    sql: &str,
    cutoff_ms: i64,
) -> Result<usize, String> {
    let count: i64 = connection
        .query_row(sql, [cutoff_ms], |row| row.get(0))
        .map_err(|_| "Daemon runtime 保留期统计失败".to_owned())?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

fn open_database(path: &Path) -> Result<Connection, String> {
    let connection =
        Connection::open(path).map_err(|_| "Daemon runtime SQLite 数据库打开失败".to_owned())?;
    connection
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|_| "Daemon runtime SQLite busy timeout 配置失败".to_owned())?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )
        .map_err(|_| "Daemon runtime SQLite 连接配置失败".to_owned())?;
    Ok(connection)
}

fn open_read_only_database(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "Daemon runtime SQLite 数据库只读打开失败".to_owned())?;
    connection
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|_| "Daemon runtime SQLite busy timeout 配置失败".to_owned())?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| "Daemon runtime SQLite 只读连接配置失败".to_owned())?;
    Ok(connection)
}

fn runtime_database_path(root: &Path) -> std::path::PathBuf {
    root.join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(RUNTIME_DATABASE_FILE)
}

/// 直接内联状态白名单：取值来自本模块常量，不存在外部输入。
pub(super) fn quoted_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn reclaim_database_space(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(&format!(
            "PRAGMA incremental_vacuum({INCREMENTAL_VACUUM_PAGE_BATCH})"
        ))
        .map_err(|_| "Daemon runtime SQLite 增量空间回收失败".to_owned())
}

#[cfg(test)]
#[path = "runtime_cleanup_agent_state_tests.rs"]
mod agent_state_tests;
#[cfg(test)]
#[path = "runtime_cleanup_tests.rs"]
mod tests;
