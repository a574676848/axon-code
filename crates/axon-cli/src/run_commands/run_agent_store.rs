//! 本机 run 与 daemon 共享的 runtime 库解析。
//!
//! mailbox 与 continuation 都是 run 级协作事实，与 `axon run` 主链路同源
//! （`<axon_root>/data/runtime.sqlite3`）。本模块是该路径的**唯一解析点**：
//! 两处各自拼路径会让「写进去的」与「查询/恢复读到的」可能不是同一个库。

use std::path::Path;

use axon_session::SqliteRuntimeStore;

/// 打开与 `axon run` 主链路同源的共享 runtime 库。
pub(crate) fn shared_runtime_store(config_path: &Path) -> Result<SqliteRuntimeStore, String> {
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(config_path)?;
    Ok(SqliteRuntimeStore::from_root(data_root))
}
