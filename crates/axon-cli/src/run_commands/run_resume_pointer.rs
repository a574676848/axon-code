//! `--continue` 的恢复指针：选择、快照与还原。
//!
//! 指针存在控制面 `resume_pointer` 单行表，与 rollout 库无关：会话内容在 thread 文件，
//! 指针只记「下一次接续去哪」。显式 `--session` 结束后要把指针还原成运行前的值，
//! 因此这里同时提供快照与还原。

use std::path::Path;

use axon_domain::DaemonSessionId;
use axon_session::{ResumePointerStore as _, RuntimeStore as _, SqliteRuntimeStore};

use crate::runtime_paths::runtime_axon_data_path_static;

/// 选择后续 `axon run --continue` 恢复的会话。
///
/// 先校验会话在控制面存在：指向不存在的会话等于把下一轮变成隐式新建，必须拒绝。
pub(crate) fn select_resume_session(config_path: &Path, session_id: &str) -> Result<(), String> {
    let store = open_store(config_path)?;
    let session = DaemonSessionId::new(session_id.to_owned())
        .map_err(|_| format!("会话 ID 非法：{session_id}"))?;
    store
        .get_session(&session)
        .map_err(|_| format!("会话不存在：{session_id}。先用 `axon sessions list` 查看可用会话"))?;
    store
        .write_resume_pointer(&session, axon_time::unix_now_ms())
        .map_err(|error| error.to_string())
}

/// 读取当前指针；从未设置时返回 `None`。
pub(crate) fn snapshot_resume_session(config_path: &Path) -> Result<Option<String>, String> {
    let store = open_store(config_path)?;
    Ok(store
        .read_resume_pointer()
        .map_err(|error| error.to_string())?
        .map(|session| session.as_str().to_owned()))
}

/// 把指针写回快照值。
pub(crate) fn restore_resume_session(
    config_path: &Path,
    snapshot: Option<&str>,
) -> Result<(), String> {
    let store = open_store(config_path)?;
    match snapshot {
        Some(session_id) => {
            let session = DaemonSessionId::new(session_id.to_owned())
                .map_err(|_| format!("会话 ID 非法：{session_id}"))?;
            store
                .write_resume_pointer(&session, axon_time::unix_now_ms())
                .map_err(|error| error.to_string())
        }
        None => store
            .clear_resume_pointer()
            .map_err(|error| error.to_string()),
    }
}

fn open_store(config_path: &Path) -> Result<SqliteRuntimeStore, String> {
    let data_root = runtime_axon_data_path_static(config_path).map_err(str::to_owned)?;
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().map_err(|error| error.to_string())?;
    Ok(store)
}

#[cfg(test)]
#[path = "run_resume_pointer_tests.rs"]
mod tests;
