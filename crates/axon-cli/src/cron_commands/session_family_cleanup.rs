//! 过期 Session family 的跨存储一致性清理。
//!
//! 会话事实来自 thread 事件面与控制面：thread 文件给出会话、父子关系与最近活动时间，
//! 运行态库给出守护进程侧的活动，治理队列给出正在跑作业的保护名单。

use std::{
    collections::{HashMap, HashSet},
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use axon_session::{
    JsonlThreadEventStore, ResumePointerStore as _, SqliteRuntimeStore, project_threads,
};
use rusqlite::{Connection, TransactionBehavior, params_from_iter, types::Value};

use super::SessionCleanupReport;
use crate::cron_commands::runtime_cleanup::{
    cleanup_runtime_session_families, count_runtime_session_families,
    protected_runtime_session_ids, stale_runtime_session_ids,
};

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

/// 过期会话家族候选：会话事实来自 thread 事件面，运行态侧补充守护进程会话。
///
/// 家族按 `parent_thread_id` 归并到根：只要族内有一条受保护会话或未过期会话，
/// 整个家族都不清理，避免回放链被删成断头。
pub(super) fn stale_session_family_ids(root: &Path, cutoff_ms: u64) -> Result<Vec<String>, String> {
    let mut protected = protected_governance_sessions(root)?;
    let sessions = family_members(root)?;
    let current = current_resume_session(root);
    let known = sessions
        .iter()
        .map(|session| session.thread_id.clone())
        .collect::<HashSet<_>>();
    let mut ids = stale_thread_family_ids(&sessions, current.as_deref(), &protected, cutoff_ms);
    extend_runtime_protection(root, &ids, &mut protected)?;
    ids = stale_thread_family_ids(&sessions, current.as_deref(), &protected, cutoff_ms);
    ids.extend(
        stale_runtime_session_ids(root, cutoff_ms)?
            .into_iter()
            .filter(|session_id| !protected.contains(session_id) && !known.contains(session_id)),
    );
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// 家族判定只需要三个标量：会话 id、父 id、最近活动时间。
#[derive(Debug, Clone)]
struct FamilyMember {
    thread_id: String,
    parent_thread_id: Option<String>,
    updated_at_ms: u64,
    /// 父 id 指向一个投影里不存在的会话：该族一律不清理，宁可漏删也不误删。
    dangling_parent: bool,
}

/// 读取会话清单：优先派生索引，缺失时回退整表投影。
fn family_members(root: &Path) -> Result<Vec<FamilyMember>, String> {
    let store = JsonlThreadEventStore;
    if let Some(snapshot) = store
        .read_thread_index(root)
        .map_err(|error| format!("读取 thread 索引失败：{error}"))?
    {
        let known = snapshot
            .entries
            .iter()
            .map(|entry| entry.thread_id.clone())
            .collect::<HashSet<_>>();
        return Ok(snapshot
            .entries
            .into_iter()
            .map(|entry| FamilyMember {
                dangling_parent: entry
                    .parent_thread_id
                    .as_deref()
                    .is_some_and(|parent| !known.contains(parent)),
                thread_id: entry.thread_id,
                parent_thread_id: entry.parent_thread_id,
                updated_at_ms: entry.updated_at_ms,
            })
            .collect());
    }
    // 索引尚未建立（旧库首次运行或缓存缺失）：回退整表投影，并把损坏分片数如实记下来。
    let scan = store
        .scan_all(root)
        .map_err(|error| format!("扫描 thread 事件面失败：{error}"))?;
    if !scan.issues.is_empty() {
        axon_logger::warning(&format!(
            "thread 事件面有 {} 个分片不可读，保留判定按剩余会话进行",
            scan.issues.len()
        ));
    }
    let details = project_threads(&scan.records);
    let known = details
        .iter()
        .map(|detail| detail.thread.thread_id.clone())
        .collect::<HashSet<_>>();
    Ok(details
        .into_iter()
        .map(|detail| {
            let thread = detail.thread;
            FamilyMember {
                dangling_parent: thread
                    .parent_thread_id
                    .as_deref()
                    .is_some_and(|parent| !known.contains(parent)),
                thread_id: thread.thread_id,
                parent_thread_id: thread.parent_thread_id,
                updated_at_ms: thread.updated_at_ms,
            }
        })
        .collect())
}

/// 当前 `--continue` 指向的会话：它所在的家族永不过期。
fn current_resume_session(root: &Path) -> Option<String> {
    let store = SqliteRuntimeStore::from_root(root.join(axon_session::AXON_RUNTIME_DATA_DIR));
    store
        .read_resume_pointer()
        .ok()
        .flatten()
        .map(|session| session.as_str().to_owned())
}

/// 按家族判定过期：族内全部成员都早于截止时间，且不含受保护会话与当前家族。
fn stale_thread_family_ids(
    sessions: &[FamilyMember],
    current_session_id: Option<&str>,
    protected_session_ids: &HashSet<String>,
    cutoff_ms: u64,
) -> Vec<String> {
    let parents = sessions
        .iter()
        .map(|session| (session.thread_id.clone(), session.parent_thread_id.clone()))
        .collect::<HashMap<_, _>>();
    let protected_root = current_session_id.map(|session_id| session_root(session_id, &parents));
    let mut families = HashMap::<String, Vec<&FamilyMember>>::new();
    for session in sessions {
        let root = session_root(&session.thread_id, &parents);
        families.entry(root).or_default().push(session);
    }
    let mut stale_ids = families
        .into_iter()
        .filter(|(root, members)| {
            protected_root.as_deref() != Some(root.as_str())
                && !members
                    .iter()
                    .any(|member| protected_session_ids.contains(&member.thread_id))
                && !members.iter().any(|member| member.dangling_parent)
                && members
                    .iter()
                    .all(|member| member.updated_at_ms < cutoff_ms)
        })
        .flat_map(|(_, members)| members.into_iter().map(|member| member.thread_id.clone()))
        .collect::<Vec<_>>();
    stale_ids.sort();
    stale_ids
}

/// 沿父链归并到家族根；遇到环或缺失父节点时停在原地。
fn session_root(session_id: &str, parents: &HashMap<String, Option<String>>) -> String {
    let mut current = session_id.to_owned();
    let mut visited = HashSet::new();
    while let Some(Some(parent)) = parents.get(&current) {
        if !visited.insert(current.clone()) || !parents.contains_key(parent) {
            break;
        }
        current.clone_from(parent);
    }
    current
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
    super::apply_runtime_cleanup_counts(report, count_runtime_session_families(root, session_ids)?);
    Ok(())
}

pub(super) fn cleanup_session_family_data(
    root: &Path,
    cutoff_ms: u64,
    session_ids: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let current = stale_session_family_ids(root, cutoff_ms)?;
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
        // 先删会话上下文、后删 thread 文件：上下文删除失败时 thread 轮次仍在，
        // 下一轮清理还能凭它重新定位整个家族，不会留下无人认领的孤儿数据。
        remove_session_context_files(root, ids, report)?;
        remove_thread_files(root, ids, report)?;
        processed.extend(ids.iter().cloned());
    }
    report.deleted_session_count = processed.len();
    Ok(())
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
