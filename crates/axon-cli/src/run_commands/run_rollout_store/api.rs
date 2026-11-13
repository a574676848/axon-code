//! rollout store 的生产 API 适配层。

use std::path::Path;

use axon_session::ThreadCompaction;

use super::{
    CLI_ROLLOUT_DB_FILE, RolloutEventRecord, RolloutHistoryWindow, RolloutSqliteStore,
    resolved_rollout_db_path,
};

#[cfg(test)]
use super::RolloutEventAppendInput;

/// 将一次完成 run 原子提交为 SQLite SSOT。
pub(crate) fn commit_session_to_rollout(
    config_path: &Path,
    record: &super::super::run_session_store::RunSessionRecord,
    title: &str,
) -> Result<(), String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.commit_session(record, Some(title))
}

/// 确保当前轮的 rollout 记录存在（换窗用它定位本轮）。
///
/// 一条交互的首个 provider turn 就可能触发换窗，而这时的轮记录要到收尾才落地。
/// 这里先补一条起始记录：换窗始终能找到当前轮，收尾提交再按同一幂等键覆盖成完整记录，
/// sequence 因此保持稳定。未知轮次仍然不得推进窗口，该门禁在 store 层原样保留。
pub(crate) fn ensure_current_turn_to_rollout(
    config_path: &Path,
    record: &super::super::run_session_store::RunSessionRecord,
) -> Result<u64, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.ensure_started_turn(record)
}

/// 读取指定 session 最新已提交记录中的 token usage。
pub(crate) fn load_session_usage(
    config_path: &Path,
    session_id: &str,
) -> Result<Option<u64>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Ok(None);
    }
    Ok(RolloutSqliteStore::open(&path)?
        .load_by_id(session_id)?
        .and_then(|record| {
            record
                .usage
                .map(axon_agent_runtime::TokenUsageTelemetry::total_tokens)
        }))
}

/// 读取一条会话截至当前的累计用量（总量与各分桶）。
///
/// `session_id` 是运行时 session 标识（车道要先经 `session_runtimes` 解析成它的 run
/// session）。与 [`load_session_usage`] 的区别在口径：那个取的是**最近一轮**的用量，用于
/// 续接远程 provider；这里取的是整条会话各轮之和，用于界面上的会话累计读数。
/// 账本尚未建立时读作全零。
pub(crate) fn load_session_usage_totals_from_root(
    root: &Path,
    session_id: &str,
) -> Result<super::SessionUsageTotals, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_owned());
    }
    let path = axon_session::session_store_path(root).join(CLI_ROLLOUT_DB_FILE);
    if !path.exists() {
        return Ok(super::SessionUsageTotals::default());
    }
    // 读侧不跑建表语句：这条路径挂在历史列表上，每次翻页都会走一遍，不该附带 schema 初始化。
    let read_only = RolloutSqliteStore::open_read_only(&path)?;
    match read_only.session_usage_totals(session_id) {
        Ok(totals) => Ok(totals),
        // 旧库还没有用量列：退回写连接补列并回填一次，之后一律走只读路径。
        Err(_) => RolloutSqliteStore::open(&path)?.session_usage_totals(session_id),
    }
}

/// 选择后续 `run --continue` 恢复的 rollout Thread。
pub(crate) fn select_rollout_session(config_path: &Path, session_id: &str) -> Result<(), String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Err("rollout SQLite 存储尚未建立".to_owned());
    }
    RolloutSqliteStore::open(&path)?.select_session(session_id)
}
pub(crate) fn install_rollout_compaction(
    config_path: &Path,
    session_id: &str,
    compaction: &ThreadCompaction,
) -> Result<(), String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.install_compaction(session_id, compaction)
}

pub(crate) fn install_active_rollout_window(
    config_path: &Path,
    session_id: &str,
    assistant_part_id: &str,
    compaction: ThreadCompaction,
) -> Result<(ThreadCompaction, Option<ThreadCompaction>), String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.install_active_window(
        session_id,
        assistant_part_id,
        compaction,
    )
}

pub(crate) fn restore_rollout_compaction(
    config_path: &Path,
    session_id: &str,
    compaction: Option<&ThreadCompaction>,
) -> Result<(), String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.replace_compaction(session_id, compaction)
}

/// 回放指定 session 的结构化 rollout 事件。
pub(crate) fn replay_rollout_events(
    config_path: &Path,
    session_id: &str,
) -> Result<Vec<RolloutEventRecord>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    RolloutSqliteStore::open(&path)?.load_events(session_id)
}

#[cfg(test)]
pub(crate) fn append_rollout_event_for_session(
    config_path: &Path,
    input: RolloutEventAppendInput,
) -> Result<RolloutEventRecord, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.append_event(input)
}

pub(crate) fn load_rollout_history_window(
    config_path: &Path,
    session_id: &str,
) -> Result<RolloutHistoryWindow, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.load_history_window(session_id)
}

/// 精确读取治理作业引用的单个 turn，避免同一 session 的后续 turn 抢占读取结果。
pub(crate) fn load_session_turn(
    config_path: &Path,
    session_id: &str,
    assistant_part_id: &str,
) -> Result<Option<super::super::run_session_store::RunSessionRecord>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Ok(None);
    }
    RolloutSqliteStore::open(&path)?.load_turn_by_id(session_id, assistant_part_id)
}

pub(crate) fn load_rollout_history_window_from_root(
    root: &Path,
    session_id: &str,
) -> Result<RolloutHistoryWindow, String> {
    let path = axon_session::session_store_path(root).join(CLI_ROLLOUT_DB_FILE);
    RolloutSqliteStore::open(&path)?.load_history_window(session_id)
}

/// 读取指定 session 中 provider receipt 关联的完整工具输出。
pub(crate) fn read_rollout_tool_output(
    config_path: &Path,
    session_id: &str,
    provider_call_id: &str,
) -> Result<Vec<u8>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Err("rollout SQLite 存储尚未建立".to_owned());
    }
    let archive_root = crate::runtime_paths::runtime_axon_data_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.read_tool_output(&archive_root, session_id, provider_call_id)
}
