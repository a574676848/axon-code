//! `axon debug session prune --orphans`：清理归属会话已经消失的非主会话。
//!
//! 子代理、Cron 与目标会话平时由会话树与保留策略维护，删单个会话的入口会拒绝它们。
//! 但归属会话被人工删掉之后，这些子会话就再也没有任何入口能收走：保留策略只按时间
//! 水位扫，运行态定位又会一直落到一条不可达的父会话上。本命令只处理这一种确定情形：
//! 归属会话指针指向一条已经不存在的会话。归属仍在的会话一律不动。
//!
//! 与控制面删除同源：会话族行、入站事实、指标与恢复指针走同一个事务；提交之后才回收
//! thread 分片、事件目录、事件归档与会话上下文目录，因此不会出现「文件没了、行还在」。

use std::path::Path;

use axon_session::{JsonlThreadEventStore, SqliteRuntimeStore};
use rusqlite::{OptionalExtension as _, params};
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
    runtime_paths::{runtime_axon_data_path_static, runtime_axon_root_path},
};

use super::session_delete::{
    SESSION_CONTEXT_DIR, archive_root, delete_orphan_family, remove_dir_if_present,
};
use super::{SessionPruneCommand, display_path, open_control_store};

/// 仍在推进的会话不收：归属缺失不代表这一轮已经停了。
const SESSION_ACTIVE_STATUSES: [&str; 5] =
    ["running", "compacting", "waiting", "recovering", "draining"];

/// 一条候选非主会话。
struct Candidate {
    session_id: String,
    kind: String,
    current_run_id: String,
    status: String,
    /// 归属会话指针；为空表示它自己就是根，不参与孤儿判定。
    root_session_id: Option<String>,
}

/// 清理回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionPruneReport {
    /// `session_pruned` 或 `session_pruned_partial`（有残留或跳过的条目）。
    status: &'static str,
    /// 线程事件存储根目录，便于核对残留位置。
    store_path: String,
    /// 扫描到的非主会话数。
    scanned: usize,
    /// 判定为孤儿并尝试删除的条数。
    orphaned: usize,
    /// 已删掉的会话。
    deleted: Vec<PrunedFamily>,
    /// 没有处理的会话与原因。
    skipped: Vec<SkippedFamily>,
    /// 删除后的磁盘残留；不影响控制面结论，但必须如实报告。
    warnings: Vec<String>,
}

/// 一条已删除的孤儿会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrunedFamily {
    session_id: String,
    kind: String,
    /// 控制面会话行。
    sessions: usize,
    /// 入站事实。
    inbox: usize,
    /// 待答提问（outbox 库）。
    user_questions: usize,
    /// 回收的 thread 分片数。
    thread_shards_removed: usize,
    /// 会话事件目录是否回收。
    event_log_removed: bool,
    /// 会话上下文目录是否回收。
    session_context_removed: bool,
}

/// 一条跳过的会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkippedFamily {
    session_id: String,
    kind: String,
    reason: String,
}

pub(super) fn run_session_prune(
    format: OutputFormat,
    config_path: &Path,
    command: &SessionPruneCommand,
) -> CliRunResult {
    if !command.orphans {
        return cli_error("session prune 需要 --orphans：本命令只清理归属会话已消失的非主会话");
    }
    let root = match runtime_axon_root_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let data_root = match runtime_axon_data_path_static(config_path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let store = match open_control_store(config_path) {
        Ok(store) => store,
        Err(error) => return cli_error(error),
    };
    let candidates = match scan_non_primary_sessions(&store) {
        Ok(candidates) => candidates,
        Err(error) => return cli_error(error),
    };
    let mut deleted = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut orphaned = 0_usize;
    for candidate in &candidates {
        let Some(parent) = candidate.root_session_id.as_deref() else {
            skipped.push(skip(candidate, "自身就是根会话，没有归属会话可判"));
            continue;
        };
        let parent_exists = match session_exists(&store, parent) {
            Ok(exists) => exists,
            Err(error) => return cli_error(error),
        };
        if parent_exists {
            skipped.push(skip(
                candidate,
                &format!("归属会话 {parent} 仍然存在，交给会话树与保留策略"),
            ));
            continue;
        }
        if SESSION_ACTIVE_STATUSES.contains(&candidate.status.as_str()) {
            skipped.push(skip(
                candidate,
                &format!(
                    "会话仍在运行（status={}），归属缺失也不在这里强删",
                    candidate.status
                ),
            ));
            continue;
        }
        orphaned = orphaned.saturating_add(1);
        let thread_ids = vec![candidate.current_run_id.clone()];
        let (rows, mut family_warnings) =
            match delete_orphan_family(&store, &candidate.session_id, &thread_ids) {
                Ok(result) => result,
                Err(error) => {
                    skipped.push(skip(candidate, &format!("删除被拒绝：{error}")));
                    continue;
                }
            };
        warnings.append(&mut family_warnings);
        let mut thread_shards_removed = 0_usize;
        for thread_id in &thread_ids {
            match JsonlThreadEventStore.remove_thread(&root, thread_id) {
                Ok(true) => thread_shards_removed = thread_shards_removed.saturating_add(1),
                Ok(false) => {}
                Err(error) => warnings.push(format!("删除 thread 分片 {thread_id} 失败：{error}")),
            }
        }
        let segment = axon_eventlog::EventLog::encode_segment(&candidate.session_id);
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
        let session_context_removed = remove_dir_if_present(
            &root.join(SESSION_CONTEXT_DIR).join(&candidate.session_id),
            "会话上下文目录",
            &mut warnings,
        );
        deleted.push(PrunedFamily {
            session_id: candidate.session_id.clone(),
            kind: candidate.kind.clone(),
            sessions: rows.sessions,
            inbox: rows.inbox,
            user_questions: rows.user_questions,
            thread_shards_removed,
            event_log_removed: events_removed || archive_removed,
            session_context_removed,
        });
    }
    let report = SessionPruneReport {
        status: if warnings.is_empty() {
            "session_pruned"
        } else {
            "session_pruned_partial"
        },
        store_path: display_path(&JsonlThreadEventStore::path(&root)),
        scanned: candidates.len(),
        orphaned,
        deleted,
        skipped,
        warnings,
    };
    render_prune(format, &report)
}

fn skip(candidate: &Candidate, reason: &str) -> SkippedFamily {
    SkippedFamily {
        session_id: candidate.session_id.clone(),
        kind: candidate.kind.clone(),
        reason: reason.to_owned(),
    }
}

/// 扫描全部非主会话；判定与删除都在调用方逐个进行，避免把「读」与「写」混进一个事务。
fn scan_non_primary_sessions(store: &SqliteRuntimeStore) -> Result<Vec<Candidate>, String> {
    store
        .with_read_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT session_id, kind, current_run_id, status, root_session_id
                   FROM sessions
                  WHERE kind <> 'primary'
                  ORDER BY created_at_ms",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(Candidate {
                    session_id: row.get(0)?,
                    kind: row.get(1)?,
                    current_run_id: row.get(2)?,
                    status: row.get(3)?,
                    root_session_id: row.get(4)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| format!("读取会话清单失败：{error}"))
}

fn session_exists(store: &SqliteRuntimeStore, session_id: &str) -> Result<bool, String> {
    store
        .with_read_connection(|connection| {
            let found: Option<i64> = connection
                .query_row(
                    "SELECT 1 FROM sessions WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
        .map_err(|error| format!("读取归属会话失败：{error}"))
}

fn render_prune(format: OutputFormat, report: &SessionPruneReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = format!(
                "Axon session prune: {}\n- scanned: {}\n- orphaned: {}\n- deleted: {}\n",
                report.status,
                report.scanned,
                report.orphaned,
                report.deleted.len()
            );
            for family in &report.deleted {
                let _ = std::fmt::Write::write_fmt(
                    &mut stdout,
                    format_args!(
                        "  - {} ({}) sessions={} inbox={} user_questions={} thread_shards_removed={}\n",
                        family.session_id,
                        family.kind,
                        family.sessions,
                        family.inbox,
                        family.user_questions,
                        family.thread_shards_removed
                    ),
                );
            }
            if !report.skipped.is_empty() {
                stdout.push_str("- skipped:\n");
                for item in &report.skipped {
                    let _ = std::fmt::Write::write_fmt(
                        &mut stdout,
                        format_args!("  - {} ({}): {}\n", item.session_id, item.kind, item.reason),
                    );
                }
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
#[path = "session_prune_tests.rs"]
mod tests;
