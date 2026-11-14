//! rollout stale Session family 的跨存储一致性清理。

use std::{
    collections::HashSet,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use axon_session::JsonlThreadEventStore;
use rusqlite::{Connection, TransactionBehavior, params_from_iter, types::Value};

use super::{
    SessionCleanupReport,
    session_cleanup_feishu::{apply_feishu_cleanup, apply_feishu_cleanup_count},
};
use crate::cron_commands::runtime_cleanup::{
    cleanup_runtime_session_families, count_runtime_session_families,
    protected_runtime_session_ids, stale_runtime_session_ids,
};
use crate::run_commands::RolloutSqliteStore;

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const GOVERNANCE_DATABASE_FILE: &str = "post_run_governance.sqlite3";
const GOVERNANCE_STATUS_COMPLETED: &str = "completed";
const GOVERNANCE_STATUS_FAILED: &str = "failed";
const GOVERNANCE_STATUS_QUEUED: &str = "queued";
const GOVERNANCE_STATUS_RUNNING: &str = "running";
/// 单批会话数：单个会话可能持有数十万条事件，批次过大会让删除事务长时间持有写锁。
const SESSION_FAMILY_BATCH_LIMIT: usize = 8;
/// 单轮会话族清理的墙上时钟预算：预算耗尽只结束本轮，剩余会话由下一轮续跑。
const SESSION_FAMILY_TIME_BUDGET: Duration = Duration::from_secs(120);
/// 单轮增量回收的页数：4 KiB 页下约等于 16 MiB，避免删了行文件却不缩。
const INCREMENTAL_VACUUM_PAGE_BATCH: u32 = 4_096;

/// 会话上下文文件计数：notes 状态文件、便签文件与归档分片。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SessionContextCounts {
    pub(super) notes_state_files: usize,
    pub(super) note_files: usize,
    pub(super) archived_shards: usize,
}

/// 过期会话家族候选：rollout 索引与运行态两侧取并集。
///
/// rollout 只索引交互式主会话，子代理车道与历史 desktop 车道不在其中；
/// 只按 rollout 取候选会让这些会话的根记录永久滞留，其事件也就永远没有
/// 回收入口。运行态侧按最后一次活动时间自判过期，并对受治理保护的会话去重。
pub(super) fn stale_session_family_ids(
    root: &Path,
    rollout_path: &Path,
    cutoff_ms: u64,
) -> Result<Vec<String>, String> {
    let mut protected = protected_governance_sessions(root)?;
    let mut ids = Vec::new();
    let mut rollout_ids = HashSet::new();
    if rollout_path.exists() {
        let mut store = RolloutSqliteStore::open_read_only(rollout_path)?;
        let initial = store.stale_session_family_ids(cutoff_ms, &protected)?;
        extend_runtime_protection(root, &initial, &mut protected)?;
        ids = store.stale_session_family_ids(cutoff_ms, &protected)?;
        // rollout 的家族保护是整族语义：只要有一个成员受保护，整族都不动。
        // 因此运行态补充候选只覆盖 rollout 完全没索引的会话，不得越过家族边界。
        rollout_ids = rollout_session_ids(rollout_path)?;
    }
    ids.extend(
        stale_runtime_session_ids(root, cutoff_ms)?
            .into_iter()
            .filter(|session_id| {
                !protected.contains(session_id) && !rollout_ids.contains(session_id)
            }),
    );
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// rollout 索引里已登记的会话集合：运行态补充候选必须避开这些会话。
fn rollout_session_ids(rollout_path: &Path) -> Result<HashSet<String>, String> {
    let connection =
        Connection::open_with_flags(rollout_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "rollout 会话索引打开失败".to_owned())?;
    let mut statement = connection
        .prepare("SELECT session_id FROM rollout_sessions")
        .map_err(|_| "rollout 会话索引查询失败".to_owned())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| "rollout 会话索引查询失败".to_owned())?;
    rows.collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|_| "rollout 会话索引读取失败".to_owned())
}

pub(super) fn count_session_family_data(
    root: &Path,
    session_ids: &[String],
    cutoff_ms: u64,
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    report.deleted_governance_count = count_governance(root, session_ids, cutoff_ms)?;
    report.deleted_thread_file_count = count_thread_files(root, session_ids)?;
    apply_context_counts(report, count_session_context_files(root, session_ids)?);
    apply_feishu_cleanup_count(root, session_ids, report)?;
    super::apply_runtime_cleanup_counts(report, count_runtime_session_families(root, session_ids)?);
    Ok(())
}

pub(super) fn cleanup_session_family_data(
    root: &Path,
    rollout_path: &Path,
    cutoff_ms: u64,
    session_ids: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let current = stale_session_family_ids(root, rollout_path, cutoff_ms)?;
    let requested = session_ids.iter().collect::<HashSet<_>>();
    let eligible = current
        .into_iter()
        .filter(|session_id| requested.contains(session_id))
        .collect::<Vec<_>>();
    let deadline = Instant::now() + SESSION_FAMILY_TIME_BUDGET;
    let mut processed: Vec<String> = Vec::new();
    for ids in eligible.chunks(SESSION_FAMILY_BATCH_LIMIT) {
        if Instant::now() >= deadline {
            break;
        }
        super::apply_runtime_cleanup_counts(report, cleanup_runtime_session_families(root, ids)?);
        report.deleted_governance_count = report
            .deleted_governance_count
            .saturating_add(cleanup_governance(root, ids, cutoff_ms)?);
        apply_feishu_cleanup(root, ids, report)?;
        remove_thread_files(root, ids, report)?;
        remove_session_context_files(root, ids, report)?;
        processed.extend(ids.iter().cloned());
    }
    finalize_rollout_cleanup(root, rollout_path, cutoff_ms, &processed, report)
}

fn protected_governance_sessions(root: &Path) -> Result<HashSet<String>, String> {
    let path = governance_database_path(root);
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let connection = open_database(&path, "治理")?;
    let mut statement = connection
        .prepare(
            "SELECT session_id FROM governance_jobs
             WHERE status IN (?1, ?2)",
        )
        .map_err(|_| "治理活跃作业读取失败".to_owned())?;
    let rows = statement
        .query_map(
            [GOVERNANCE_STATUS_QUEUED, GOVERNANCE_STATUS_RUNNING],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| "治理活跃作业读取失败".to_owned())?;
    rows.collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|_| "治理活跃作业读取失败".to_owned())
}

fn extend_runtime_protection(
    root: &Path,
    candidates: &[String],
    protected: &mut HashSet<String>,
) -> Result<(), String> {
    protected.extend(protected_runtime_session_ids(root, candidates)?);
    Ok(())
}

fn count_governance(root: &Path, session_ids: &[String], cutoff_ms: u64) -> Result<usize, String> {
    let path = governance_database_path(root);
    if session_ids.is_empty() || !path.exists() {
        return Ok(0);
    }
    let connection = open_database(&path, "治理")?;
    let mut count = 0_usize;
    for ids in session_ids.chunks(SESSION_FAMILY_BATCH_LIMIT) {
        let sql = format!(
            "SELECT COUNT(*) FROM governance_jobs
             WHERE status IN (?1,?2) AND updated_at_ms < ?3
               AND session_id IN ({})",
            placeholders(ids.len())
        );
        let values = governance_values(ids, cutoff_ms);
        let batch: i64 = connection
            .query_row(&sql, params_from_iter(values.iter()), |row| row.get(0))
            .map_err(|_| "治理 dry-run 统计失败".to_owned())?;
        count = count.saturating_add(usize::try_from(batch).unwrap_or(usize::MAX));
    }
    Ok(count)
}

fn cleanup_governance(
    root: &Path,
    session_ids: &[String],
    cutoff_ms: u64,
) -> Result<usize, String> {
    let path = governance_database_path(root);
    if session_ids.is_empty() || !path.exists() {
        return Ok(0);
    }
    let mut connection = open_database(&path, "治理")?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| "治理清理事务启动失败".to_owned())?;
    let sql = format!(
        "DELETE FROM governance_jobs
         WHERE status IN (?1,?2) AND updated_at_ms < ?3
           AND session_id IN ({})",
        placeholders(session_ids.len())
    );
    let values = governance_values(session_ids, cutoff_ms);
    let deleted = transaction
        .execute(&sql, params_from_iter(values.iter()))
        .map_err(|_| "治理历史清理失败".to_owned())?;
    transaction
        .commit()
        .map_err(|_| "治理清理事务提交失败".to_owned())?;
    if deleted > 0 {
        reclaim_database_space(&connection, "治理")?;
    }
    Ok(deleted)
}

fn governance_values(session_ids: &[String], cutoff_ms: u64) -> Vec<Value> {
    let mut values = vec![
        Value::Text(GOVERNANCE_STATUS_COMPLETED.to_owned()),
        Value::Text(GOVERNANCE_STATUS_FAILED.to_owned()),
        Value::Integer(i64::try_from(cutoff_ms).unwrap_or(i64::MAX)),
    ];
    values.extend(session_ids.iter().cloned().map(Value::Text));
    values
}

fn count_thread_files(root: &Path, session_ids: &[String]) -> Result<usize, String> {
    let mut count = 0_usize;
    for id in session_ids {
        if !JsonlThreadEventStore::thread_shard_paths(root, id)?.is_empty() {
            count += 1;
        }
    }
    Ok(count)
}

fn remove_thread_files(
    root: &Path,
    session_ids: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let thread_store = JsonlThreadEventStore;
    for session_id in session_ids {
        if thread_store.remove_thread(root, session_id)? {
            report.deleted_thread_file_count += 1;
        }
    }
    Ok(())
}

fn count_session_context_files(
    root: &Path,
    session_ids: &[String],
) -> Result<SessionContextCounts, String> {
    let sessions_dir = root.join("sessions");
    let mut counts = SessionContextCounts::default();
    for id in session_ids {
        let session_dir = sessions_dir.join(id);
        if session_dir.is_dir() {
            merge_context_counts(&mut counts, count_session_dir_context(&session_dir)?);
        }
    }
    Ok(counts)
}

fn count_session_dir_context(session_dir: &Path) -> Result<SessionContextCounts, String> {
    Ok(SessionContextCounts {
        notes_state_files: usize::from(
            session_dir
                .join(axon_agent_runtime::SESSION_NOTES_FILE_NAME)
                .is_file(),
        ),
        note_files: count_files_recursive(&session_dir.join(axon_agent_runtime::NOTES_DIR_NAME))?,
        archived_shards: count_files_recursive(
            &session_dir.join(axon_agent_runtime::ARCHIVED_HISTORY_DIR_NAME),
        )?,
    })
}

fn count_files_recursive(directory: &Path) -> Result<usize, String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        // 路径不存在，或该路径被同名文件占用：里面都不可能有文件，一律按 0 计。
        // 只认 `NotFound` 会让同一份数据在 unix 上变成硬错误（unix 报 `NotADirectory`），
        // 于是「删除会话上下文目录失败」这条真正要向上传递的结论被提前的读取错误盖住。
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(0);
        }
        Err(error) => return Err(format!("读取会话上下文目录失败：{error}")),
    };
    let mut count = 0_usize;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("读取会话上下文条目失败：{error}"))?
            .path();
        if path.is_dir() {
            count = count.saturating_add(count_files_recursive(&path)?);
        } else if path.is_file() {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn remove_session_context_files(
    root: &Path,
    session_ids: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let sessions_dir = root.join("sessions");
    for id in session_ids {
        let session_dir = sessions_dir.join(id);
        if !session_dir.exists() {
            continue;
        }
        let counts = count_session_dir_context(&session_dir)?;
        std::fs::remove_dir_all(&session_dir).map_err(|error| {
            format!("删除会话上下文目录 {} 失败：{error}", session_dir.display())
        })?;
        apply_context_counts(report, counts);
    }
    Ok(())
}

fn finalize_rollout_cleanup(
    root: &Path,
    rollout_path: &Path,
    cutoff_ms: u64,
    session_ids: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    if session_ids.is_empty() || !rollout_path.exists() {
        return Ok(());
    }
    let mut protected = protected_governance_sessions(root)?;
    extend_runtime_protection(root, session_ids, &mut protected)?;
    let mut store = RolloutSqliteStore::open(rollout_path)?;
    let rollout =
        store.finalize_stale_session_family_removal(session_ids, cutoff_ms, &protected)?;
    report.deleted_session_count = rollout.session_ids.len();
    report.vacuumed_rollout = rollout.vacuumed;
    Ok(())
}

fn apply_context_counts(report: &mut SessionCleanupReport, counts: SessionContextCounts) {
    report.deleted_session_notes_count = report
        .deleted_session_notes_count
        .saturating_add(counts.notes_state_files);
    report.deleted_note_file_count = report
        .deleted_note_file_count
        .saturating_add(counts.note_files);
    report.deleted_archived_shard_count = report
        .deleted_archived_shard_count
        .saturating_add(counts.archived_shards);
}

fn merge_context_counts(total: &mut SessionContextCounts, counts: SessionContextCounts) {
    total.notes_state_files = total
        .notes_state_files
        .saturating_add(counts.notes_state_files);
    total.note_files = total.note_files.saturating_add(counts.note_files);
    total.archived_shards = total.archived_shards.saturating_add(counts.archived_shards);
}

fn governance_database_path(root: &Path) -> PathBuf {
    axon_session::session_store_path(root).join(GOVERNANCE_DATABASE_FILE)
}

fn open_database(path: &Path, label: &str) -> Result<Connection, String> {
    let connection =
        Connection::open(path).map_err(|_| format!("{label} SQLite 数据库打开失败"))?;
    connection
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|_| format!("{label} SQLite busy timeout 配置失败"))?;
    Ok(connection)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

fn reclaim_database_space(connection: &Connection, label: &str) -> Result<(), String> {
    connection
        .execute_batch(&format!(
            "PRAGMA incremental_vacuum({INCREMENTAL_VACUUM_PAGE_BATCH})"
        ))
        .map_err(|_| format!("{label} SQLite 增量空间回收失败"))
}

#[cfg(test)]
#[path = "session_family_cleanup_tests.rs"]
mod tests;
