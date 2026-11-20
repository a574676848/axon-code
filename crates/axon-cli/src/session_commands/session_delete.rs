//! `axon sessions delete`：会话族的控制面事务删除。
//!
//! 删除单位是**会话族**而不是 thread 文件。只删 thread 分片会把会话行、入站事实与
//! 事件日志全部留下，TUI 与桌面端照旧列得出来、也照旧回放得出来，等于「删了但没删」。
//! 因此这里把控制面行（`sessions`）、入站事实（`inbox`）、工作项与其历史、会话指标、
//! 轮次投影、调度意图与恢复指针放进同一个 `IMMEDIATE` 事务：任何一步失败整体回滚，
//! 会话原样保留。事务提交之后才回收磁盘产物（thread 分片、会话事件目录、事件归档
//! 目录、会话上下文目录），因此不可能出现「事务回滚了、文件却已经删掉」。
//!
//! 判据与 `cron_commands::runtime_cleanup` 的会话族清理同源：在途会话与在途工作项一律
//! 拒绝删除，差别只在于这里是用户显式点名的一条会话，不再受保留期水位约束。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{
    JsonlThreadEventStore, RuntimeStore as _, SqliteRuntimeStore, ThreadEventStore as _,
};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
    runtime_paths::{runtime_axon_data_path_static, runtime_axon_root_path},
};

use super::{
    SessionDeleteCommand, TargetKind, display_path, open_control_store, resolve_session_target,
};

/// 在途会话状态：仍在推进的会话删掉等于把运行中的一轮摸走。
const SESSION_ACTIVE_STATUSES: [&str; 5] =
    ["running", "compacting", "waiting", "recovering", "draining"];
/// 在途工作项状态：排队、等待与运行中的回合都还在执行流里。
const WORK_ITEM_ACTIVE_STATUSES: [&str; 3] = ["queued", "waiting", "running"];
/// 会话事件归档目录：`<data>/archive/session-events`，与事件目录同名存放。
const EVENT_ARCHIVE_DIRS: [&str; 2] = ["archive", "session-events"];
/// 会话上下文目录（便签、运行笔记）所在的根子目录名。
pub(super) const SESSION_CONTEXT_DIR: &str = "sessions";

/// 一次删除在各表上的行数，逐项如实报告给调用方。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DeletedRows {
    /// `sessions` 行：0 表示控制面本来就没有这条会话。
    pub(super) sessions: usize,
    /// 入站事实。
    pub(super) inbox: usize,
    /// 在途工作项。
    work_items: usize,
    /// 工作项历史。
    work_item_history: usize,
    /// 会话累计指标。
    session_metrics: usize,
    /// 逐轮活动投影。
    turn_activity: usize,
    /// 轮次标量事实投影。
    thread_turn_facts: usize,
    /// 调度意图（属于该会话的定时项）。
    schedules: usize,
    /// `--continue` 恢复指针；指向已删会话的指针一并清掉。
    resume_pointer: usize,
    /// 待答提问（落在 outbox 库）。
    pub(super) user_questions: usize,
}

/// 删除回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionDeleteReport {
    /// `session_deleted` 或 `session_deleted_partial`（行已删干净，磁盘仍有残留）。
    status: &'static str,
    /// 事件存储根目录，便于用户核对残留位置。
    store_path: String,
    /// 调用方给出的会话或 Thread 标识。
    session_id: String,
    /// 控制面会话行是否真的被删掉。
    session_row_deleted: bool,
    /// 被删 thread 分片里的记录条数。
    deleted_thread_events: usize,
    /// 事件存储里其余 thread 的记录条数。
    remaining_events: usize,
    /// 分表删除行数。
    deleted_rows: DeletedRows,
    /// 成功删除的 thread 分片数。
    thread_shards_removed: usize,
    /// 会话事件目录是否被回收。
    event_log_removed: bool,
    /// 会话上下文目录（便签与运行笔记）是否被回收。
    session_context_removed: bool,
    /// 提交之后未能回收的残留；不影响控制面结论，但必须如实报告。
    warnings: Vec<String>,
}

/// 删除目标：要清的控制面会话与要回收的 thread 分片。
///
/// 会话级入口一定带会话行；Thread 级入口只有在那个窗口仍是某条会话的当前运行窗口时
/// 才带会话行，否则它是一段孤立的旧运行窗口，只回收分片。
struct DeleteTarget {
    session_id: Option<String>,
    thread_ids: Vec<String>,
}

/// 事务内的会话族终态判定。
enum FamilyState {
    /// 控制面没有这条会话：可能是并发删除，如实报告而不是静默成功。
    Missing,
    /// 受保护，不能删。
    Blocked(String),
    /// 可以删。
    Ready,
}

/// 事务提交与回滚之后的结果。
enum FamilyDelete {
    Missing,
    Blocked(String),
    Done(DeletedRows),
}

pub(super) fn run_session_delete(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionDeleteCommand,
    kind: TargetKind,
) -> CliRunResult {
    let given = command.session_id.trim();
    if given.is_empty() {
        return cli_error("session id 不能为空");
    }
    let root = match runtime_axon_root_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let target = match resolve_delete_target(config_path, kind, given) {
        Ok(target) => target,
        Err(error) => return cli_error(error),
    };
    let thread_store = JsonlThreadEventStore;
    let records = match thread_store.read_all(&root) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let deleted_thread_events = records
        .iter()
        .filter(|record| target.thread_ids.contains(&record.thread_id))
        .count();
    let data_root = match runtime_axon_data_path_static(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let store = SqliteRuntimeStore::from_root(&data_root);
    if let Err(error) = store.initialize() {
        return cli_error(error.to_string());
    }
    let mut warnings = Vec::new();
    let deleted_rows = match target.session_id.as_deref() {
        Some(session_id) => {
            match delete_session_family(
                &store,
                session_id,
                &target.thread_ids,
                false,
                &mut warnings,
            ) {
                Ok(rows) => rows,
                // 事务已回滚：控制面、thread 分片与事件目录都原样保留。
                Err(error) => return cli_error(error),
            }
        }
        None => DeletedRows::default(),
    };
    // 行确认删掉之后才动磁盘：顺序颠倒会留下「文件没了、行还在」的回放空洞。
    let mut thread_shards_removed = 0_usize;
    for thread_id in &target.thread_ids {
        match JsonlThreadEventStore.remove_thread(&root, thread_id) {
            Ok(true) => thread_shards_removed = thread_shards_removed.saturating_add(1),
            Ok(false) => {}
            Err(error) => warnings.push(format!("删除 thread 分片 {thread_id} 失败：{error}")),
        }
    }
    let mut event_log_removed = false;
    let mut session_context_removed = false;
    if let Some(session_id) = target.session_id.as_deref() {
        let segment = axon_eventlog::EventLog::encode_segment(session_id);
        // 两个目录都要试着回收：一个是事件正文，一个是正文归档，短路求值会漏掉后者。
        let events_removed = remove_dir_if_present(
            &store.events_root().join(&segment),
            "会话事件目录",
            &mut warnings,
        );
        let archive_removed = remove_dir_if_present(
            &archive_root(&data_root).join(&segment),
            "事件归档目录",
            &mut warnings,
        );
        event_log_removed = events_removed || archive_removed;
        session_context_removed = remove_dir_if_present(
            &root.join(SESSION_CONTEXT_DIR).join(session_id),
            "会话上下文目录",
            &mut warnings,
        );
    }
    let report = SessionDeleteReport {
        status: if warnings.is_empty() {
            "session_deleted"
        } else {
            "session_deleted_partial"
        },
        store_path: display_path(&JsonlThreadEventStore::path(&root)),
        session_id: given.to_owned(),
        session_row_deleted: deleted_rows.sessions > 0,
        deleted_thread_events,
        remaining_events: records.len().saturating_sub(deleted_thread_events),
        deleted_rows,
        thread_shards_removed,
        event_log_removed,
        session_context_removed,
        warnings,
    };
    render_delete(format, &report)
}

/// 解析删除目标：两个标识空间互不回落。
///
/// 会话级入口只认 `sessions.session_id`；Thread 级入口只认运行窗口分片，并在它正是某条
/// 会话的当前运行窗口时带出那条会话：只删分片会留下指向已删分片的 `current_run_id`，
/// 下一个读它的入口就会对着一根空指针回放。
fn resolve_delete_target(
    config_path: &Path,
    kind: TargetKind,
    given: &str,
) -> Result<DeleteTarget, String> {
    match kind {
        TargetKind::Session => {
            let session = DaemonSessionId::new(given.to_owned())
                .map_err(|_| format!("会话 ID 非法：{given}"))?;
            let store = open_control_store(config_path)?;
            let record = store.get_session(&session).map_err(|_| {
                format!("未找到会话：{given}。先用 `axon sessions list` 确认可用会话 ID")
            })?;
            Ok(DeleteTarget {
                session_id: Some(record.session_id.as_str().to_owned()),
                thread_ids: vec![record.current_run_id],
            })
        }
        TargetKind::Thread => {
            let resolved = resolve_session_target(config_path, TargetKind::Thread, given)?;
            Ok(DeleteTarget {
                session_id: resolved.session_id,
                thread_ids: vec![resolved.thread_id],
            })
        }
    }
}

/// 孤儿清理入口：只删「归属会话已经不在」的非主会话。
///
/// 放行只发生在调用方已确认归属缺失之后；本函数不再自行判断树关系，
/// 判定与列举都在 `session_prune`，避免两处对「孤儿」各说各话。
pub(super) fn delete_orphan_family(
    store: &SqliteRuntimeStore,
    session_id: &str,
    thread_ids: &[String],
) -> Result<(DeletedRows, Vec<String>), String> {
    let mut warnings = Vec::new();
    let rows = delete_session_family(store, session_id, thread_ids, true, &mut warnings)?;
    Ok((rows, warnings))
}

/// 在单个 `IMMEDIATE` 事务里删掉会话族；提交之后再补 outbox 侧。
///
/// 事务之外不再有任何控制面写入：失败路径只会把 `warnings` 填上磁盘残留，
/// 不存在「删一半」的时间窗。
fn delete_session_family(
    store: &SqliteRuntimeStore,
    session_id: &str,
    thread_ids: &[String],
    allow_non_primary: bool,
    warnings: &mut Vec<String>,
) -> Result<DeletedRows, String> {
    let outcome = store
        .with_read_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
            match family_state(&transaction, session_id, allow_non_primary)? {
                FamilyState::Missing => {
                    transaction.rollback()?;
                    return Ok(FamilyDelete::Missing);
                }
                FamilyState::Blocked(reason) => {
                    transaction.rollback()?;
                    return Ok(FamilyDelete::Blocked(reason));
                }
                FamilyState::Ready => {}
            }
            let rows = DeletedRows {
                // 外键 `work_items.session_id → sessions.session_id` 要求工作项先走一步。
                work_items: delete_by_session(&transaction, "work_items", session_id)?,
                work_item_history: delete_by_session(
                    &transaction,
                    "work_item_history",
                    session_id,
                )?,
                turn_activity: delete_by_session(&transaction, "turn_activity", session_id)?,
                inbox: delete_by_session(&transaction, "inbox", session_id)?,
                session_metrics: delete_by_session(&transaction, "session_metrics", session_id)?,
                schedules: delete_by_session(&transaction, "schedules", session_id)?,
                resume_pointer: delete_by_session(&transaction, "resume_pointer", session_id)?,
                thread_turn_facts: delete_thread_facts(&transaction, thread_ids)?,
                sessions: delete_by_session(&transaction, "sessions", session_id)?,
                ..DeletedRows::default()
            };
            transaction.commit()?;
            Ok(FamilyDelete::Done(rows))
        })
        .map_err(|error| format!("控制面删除失败，未删除任何数据：{error}"))?;
    let mut rows = match outcome {
        FamilyDelete::Missing => return Err(format!("未找到会话：{session_id}")),
        FamilyDelete::Blocked(reason) => return Err(reason),
        FamilyDelete::Done(rows) => rows,
    };
    // 待答提问落在 outbox 库，与控制面不是同一个文件：跨库无法与上面的事务一起提交，
    // 因此放在提交之后单独收。删不掉只记残留：提问自带过期时间，不会永久滞留。
    match delete_outbox_questions(store, session_id) {
        Ok(count) => rows.user_questions = count,
        Err(error) => warnings.push(format!("待答提问清理失败：{error}")),
    }
    Ok(rows)
}

/// 事务内的终态判定：不存在、受保护、可删。
///
/// `allow_non_primary` 只给孤儿清理用：子代理、Cron 与目标会话平时由会话树与保留策略
/// 维护，只有它们的归属会话已经不存在时，才由 `session prune --orphans` 显式放行。
fn family_state(
    transaction: &Transaction<'_>,
    session_id: &str,
    allow_non_primary: bool,
) -> rusqlite::Result<FamilyState> {
    let row = transaction
        .query_row(
            "SELECT status, kind FROM sessions WHERE session_id = ?1",
            params![session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((status, kind)) = row else {
        return Ok(FamilyState::Missing);
    };
    if kind != "primary" && !allow_non_primary {
        // 子代理、Cron 与目标会话的归属挂在会话树与保留策略上：从这里删会让父会话
        // 的子会话清单出现断点，因此只允许主会话走用户入口。
        return Ok(FamilyState::Blocked(format!(
            "会话 {session_id} 不是主会话（kind={kind}），其归属由会话树与保留策略维护，不在本入口删除"
        )));
    }
    if SESSION_ACTIVE_STATUSES.contains(&status.as_str()) {
        return Ok(FamilyState::Blocked(format!(
            "会话正在运行（status={status}），请先停止再删除"
        )));
    }
    let sql = format!(
        "SELECT COUNT(*) FROM work_items
          WHERE session_id = ?1 AND status IN ({})",
        quoted_list(&WORK_ITEM_ACTIVE_STATUSES)
    );
    let active: i64 = transaction.query_row(&sql, params![session_id], |row| row.get(0))?;
    if active > 0 {
        return Ok(FamilyState::Blocked(
            "会话还有排队或运行中的回合，请等它结束再删除".to_owned(),
        ));
    }
    Ok(FamilyState::Ready)
}

fn delete_by_session(
    transaction: &Transaction<'_>,
    table: &str,
    session_id: &str,
) -> rusqlite::Result<usize> {
    let sql = format!("DELETE FROM {table} WHERE session_id = ?1");
    transaction.execute(&sql, params![session_id])
}

/// 轮次标量事实按运行窗口命名（`sessions.current_run_id`），不是会话标识。
fn delete_thread_facts(
    transaction: &Transaction<'_>,
    thread_ids: &[String],
) -> rusqlite::Result<usize> {
    let mut deleted = 0_usize;
    for thread_id in thread_ids {
        deleted = deleted.saturating_add(transaction.execute(
            "DELETE FROM thread_turn_facts WHERE thread_id = ?1",
            params![thread_id],
        )?);
    }
    Ok(deleted)
}

fn delete_outbox_questions(store: &SqliteRuntimeStore, session_id: &str) -> Result<usize, String> {
    store
        .with_outbox_read_connection(|connection| {
            connection.execute(
                "DELETE FROM user_questions WHERE session_id = ?1",
                params![session_id],
            )
        })
        .map_err(|error| error.to_string())
}

/// 事件归档根：与事件目录同级的 `archive/session-events`。
pub(super) fn archive_root(data_root: &Path) -> std::path::PathBuf {
    EVENT_ARCHIVE_DIRS
        .iter()
        .fold(data_root.to_path_buf(), |path, segment| path.join(segment))
}

/// 目录不存在按「已回收」处理；删不掉只记残留，不回滚已经提交的控制面结论。
pub(super) fn remove_dir_if_present(path: &Path, label: &str, warnings: &mut Vec<String>) -> bool {
    if !path.exists() {
        return false;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(error) => {
            warnings.push(format!("删除{label}失败（{}）：{error}", path.display()));
            false
        }
    }
}

fn quoted_list(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(",")
}

fn render_delete(format: OutputFormat, report: &SessionDeleteReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let rows = report.deleted_rows;
            let mut stdout = format!(
                "已删除会话: {}\n- sessions: {}\n- inbox: {}\n- work_items: {}\n- work_item_history: {}\n- session_metrics: {}\n- turn_activity: {}\n- thread_turn_facts: {}\n- schedules: {}\n- user_questions: {}\n- thread_shards_removed: {}\n- deleted_thread_events: {}\n- remaining_events: {}\n- event_log_removed: {}\n",
                report.session_id,
                rows.sessions,
                rows.inbox,
                rows.work_items,
                rows.work_item_history,
                rows.session_metrics,
                rows.turn_activity,
                rows.thread_turn_facts,
                rows.schedules,
                rows.user_questions,
                report.thread_shards_removed,
                report.deleted_thread_events,
                report.remaining_events,
                report.event_log_removed,
            );
            if rows.resume_pointer > 0 {
                stdout.push_str("- 恢复指针已一并清除：`--continue` 不再指向该会话\n");
            }
            let stderr = if report.warnings.is_empty() {
                String::new()
            } else {
                format!("{}\n", report.warnings.join("\n"))
            };
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr,
            }
        }
    }
}

#[cfg(test)]
#[path = "session_delete_tests.rs"]
mod tests;
