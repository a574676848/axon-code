//! 会话 ID 到 Thread ID 的解析。
//!
//! `axon sessions list` 列的是运行态会话（控制面 `sessions` 行），而 Thread 事件流按运行
//! 窗口 `current_run_id` 落盘：CLI 复合车道发起的会话两者并不相等。用户会把清单里的 ID
//! 原样交给 `sessions show` / `read` / `resume` / `fork` / `export` / `replay` / `delete`，
//! 因此这些入口统一先解析一次：先把入参当 Thread 直接命中（`debug session list` 与
//! `--session` 用的就是 Thread ID），未命中再回落到该会话所属的运行窗口。
//!
//! 解析同时给出一个可写进 `--continue` 恢复指针的会话身份：指针要能重建渠道与投递目标，
//! 因此优先用控制面会话 ID，而不是 Thread ID。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{JsonlThreadEventStore, RuntimeStore as _, SqliteRuntimeStore};
use rusqlite::OptionalExtension as _;

use crate::runtime_paths::{runtime_axon_data_path_static, runtime_axon_root_path};

/// 解析结果：Thread 身份与恢复指针要用的会话身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSession {
    /// Thread 事件流线程 ID。
    pub(crate) thread_id: String,
    /// 控制面会话 ID；`--continue` 指针写这个值才能重建渠道与投递目标。
    pub(crate) session_id: String,
}

/// 把用户输入的会话 ID 解析成 Thread ID。
pub(crate) fn resolve_session_target(
    config_path: &Path,
    given: &str,
) -> Result<ResolvedSession, String> {
    let given = given.trim();
    if given.is_empty() {
        return Err("会话 ID 不能为空".to_owned());
    }
    let root = runtime_axon_root_path(config_path)?;
    let store = open_store(config_path)?;
    // 1) 直接按 Thread 命中：`debug session list`、`--session`、fork 出来的 ID 都走这里。
    if thread_exists(&root, given)? {
        let session_id = owning_session_id(&store, given)?;
        return Ok(ResolvedSession {
            thread_id: given.to_owned(),
            session_id: session_id.unwrap_or_else(|| given.to_owned()),
        });
    }
    // 2) 按运行态会话回落：会话行的 current_run_id 就是它的 Thread。
    let session =
        DaemonSessionId::new(given.to_owned()).map_err(|_| format!("会话 ID 非法：{given}"))?;
    let record = store
        .get_session(&session)
        .map_err(|_| session_not_found(given))?;
    if !thread_exists(&root, &record.current_run_id)? {
        return Err(session_not_found(given));
    }
    Ok(ResolvedSession {
        thread_id: record.current_run_id,
        session_id: given.to_owned(),
    })
}

/// 该 Thread 是否是某个复合车道会话的运行窗口。
///
/// 只用于一处判断：CLI 直跑时补齐会话行前先确认没有车道认领这段运行窗口，
/// 否则会给同一段对话再登记一条重复会话。
pub(crate) fn run_window_has_lane(store: &SqliteRuntimeStore, thread_id: &str) -> bool {
    store
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT 1 FROM sessions WHERE current_run_id = ?1 LIMIT 1",
                    [thread_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
        })
        .is_ok_and(|found| found.is_some())
}

/// 找出该 Thread 对应的会话身份：自己的行优先，其次回落到认领该运行窗口的车道。
fn owning_session_id(
    store: &SqliteRuntimeStore,
    thread_id: &str,
) -> Result<Option<String>, String> {
    store
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT session_id FROM sessions WHERE session_id = ?1 LIMIT 1",
                    [thread_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .and_then(|own| match own {
                    Some(session_id) => Ok(Some(session_id)),
                    None => connection
                        .query_row(
                            "SELECT session_id FROM sessions WHERE current_run_id = ?1
                             ORDER BY last_activity_at_ms DESC LIMIT 1",
                            [thread_id],
                            |row| row.get::<_, String>(0),
                        )
                        .optional(),
                })
        })
        .map_err(|error| error.to_string())
}

fn thread_exists(root: &Path, thread_id: &str) -> Result<bool, String> {
    JsonlThreadEventStore::thread_shard_paths(root, thread_id).map(|paths| !paths.is_empty())
}

fn session_not_found(given: &str) -> String {
    format!(
        "未找到会话：{given}。先用 `axon sessions list` 或 `axon debug session list` 查看可用 ID"
    )
}

fn open_store(config_path: &Path) -> Result<SqliteRuntimeStore, String> {
    let data_root = runtime_axon_data_path_static(config_path)?;
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().map_err(|error| error.to_string())?;
    Ok(store)
}

#[cfg(test)]
#[path = "session_resolution_tests.rs"]
mod tests;
