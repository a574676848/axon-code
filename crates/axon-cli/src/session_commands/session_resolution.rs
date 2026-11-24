//! 会话标识与运行窗口标识的解析。
//!
//! 两个标识空间各有唯一入口，互不回落：
//!
//! - 会话标识是 `sessions.session_id`（服务端生成的 ULID），用户可见的稳定身份。
//!   `axon sessions` 与 `--continue` 恢复指针只认它；解析读一次会话行，运行窗口取自
//!   `sessions.current_run_id`。
//! - 运行窗口标识是 `sessions.current_run_id`，只在 `axon debug session` 的 Thread 级
//!   入口出现。解析只做两件事：确认事件流里存在这段窗口，按归属列反查会话。
//!
//! 不做「先当 Thread 命中、再按会话回落」的串行回落，也不做「先查自己的行、再查认领者」
//! 的二次回落：传入哪一类标识就只走那一条路径，查不到直接报错。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{JsonlThreadEventStore, RuntimeStore as _, SqliteRuntimeStore};
use rusqlite::OptionalExtension as _;

use crate::runtime_paths::{runtime_axon_data_path_static, runtime_axon_root_path};

/// 入口接受的标识空间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    /// 会话标识：`axon sessions` 与 `--continue` 恢复指针。
    Session,
    /// 运行窗口标识：`axon debug session`。
    Thread,
}

/// 解析结果：要操作的 Thread 与它归属的会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedSession {
    /// Thread 事件流线程 ID，即会话行的 `current_run_id`。
    pub(crate) thread_id: String,
    /// 会话标识；Thread 尚未登记会话时为 `None`。
    pub(crate) session_id: Option<String>,
}

/// 按入口的标识空间解析一次，给出要操作的 Thread。
pub(crate) fn resolve_session_target(
    config_path: &Path,
    kind: TargetKind,
    given: &str,
) -> Result<ResolvedSession, String> {
    let given = given.trim();
    if given.is_empty() {
        return Err("会话 ID 不能为空".to_owned());
    }
    let root = runtime_axon_root_path(config_path)?;
    match kind {
        TargetKind::Session => resolve_by_session(config_path, &root, given),
        TargetKind::Thread => resolve_by_thread(config_path, &root, given),
    }
}

/// 会话标识 → 运行窗口。只读一次会话行，再把运行窗口确认到事件流。
fn resolve_by_session(
    config_path: &Path,
    root: &Path,
    given: &str,
) -> Result<ResolvedSession, String> {
    let session =
        DaemonSessionId::new(given.to_owned()).map_err(|_| format!("会话 ID 非法：{given}"))?;
    let store = open_control_store(config_path)?;
    let record = store
        .get_session(&session)
        .map_err(|_| session_not_found(given))?;
    if !thread_exists(root, &record.current_run_id)? {
        return Err(session_not_found(given));
    }
    Ok(ResolvedSession {
        thread_id: record.current_run_id,
        session_id: Some(record.session_id.as_str().to_owned()),
    })
}

/// 运行窗口标识 → Thread 本身，并带回归属会话（可能尚未登记）。
fn resolve_by_thread(
    config_path: &Path,
    root: &Path,
    given: &str,
) -> Result<ResolvedSession, String> {
    if !thread_exists(root, given)? {
        return Err(thread_not_found(given));
    }
    let store = open_control_store(config_path)?;
    Ok(ResolvedSession {
        thread_id: given.to_owned(),
        session_id: session_of_thread(&store, given)?,
    })
}

/// 运行窗口归属的会话标识；没有归属行返回 `None`。
pub(crate) fn session_of_thread(
    store: &SqliteRuntimeStore,
    thread_id: &str,
) -> Result<Option<String>, String> {
    store
        .with_read_connection(|connection| {
            connection
                .query_row(
                    "SELECT session_id FROM sessions WHERE current_run_id = ?1 LIMIT 1",
                    [thread_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .map_err(|error| error.to_string())
}

fn thread_exists(root: &Path, thread_id: &str) -> Result<bool, String> {
    JsonlThreadEventStore::thread_shard_paths(root, thread_id).map(|paths| !paths.is_empty())
}

fn session_not_found(given: &str) -> String {
    format!("未找到会话：{given}。先用 `axon sessions list` 确认可用会话 ID")
}

fn thread_not_found(given: &str) -> String {
    format!("未找到 Thread：{given}。先用 `axon debug session list` 确认可用 Thread ID")
}

/// 打开控制面库：会话身份与运行窗口的唯一读取入口。
pub(crate) fn open_control_store(config_path: &Path) -> Result<SqliteRuntimeStore, String> {
    let data_root = runtime_axon_data_path_static(config_path)?;
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().map_err(|error| error.to_string())?;
    Ok(store)
}

#[cfg(test)]
#[path = "session_resolution_tests.rs"]
mod tests;
