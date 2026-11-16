//! rollout store 的生产 API 适配层。

use std::path::Path;

use axon_session::ThreadCompaction;

use super::{
    CLI_ROLLOUT_DB_FILE, RolloutEventRecord, RolloutSqliteStore, resolved_rollout_db_path,
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

/// 写入一条子代理会话的状态。
///
/// 子代理生命周期在起跑、完成与终态三处调用它，名册因此只读会话行就能回答
/// 「这条子代理是什么状态」，不必回放线程日志。
pub(crate) fn mark_child_session_status(
    config_path: &Path,
    session_id: &str,
    status: &str,
    at_ms: u64,
) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_owned());
    }
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.mark_session_status(session_id, status, at_ms)
}

/// 写入一条子代理会话的来源：用户可见的委派，还是运行侧内务。
///
/// 子代理起跑时写一次。名册据此把内务会话（自迭代、反思、语义显著度）排除在外，
/// 否则每个跑过一轮的会话都会长出一条并不存在的「子代理」。
pub(crate) fn mark_child_session_origin(
    config_path: &Path,
    session_id: &str,
    origin: &str,
) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_owned());
    }
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.mark_session_origin(session_id, origin)
}

/// 某条会话直接派生的子代理名册；按最近活动倒序。
///
/// 只走 `rollout_sessions.parent_session_id` 索引，线程日志一概不读：名册的代价与
/// 历史轮次的体量无关，打开车道即可拿到。台账尚未建立时读作空名册。
pub(crate) fn load_child_sessions_from_root(
    root: &Path,
    parent_session_id: &str,
    limit: usize,
) -> Result<Vec<super::ChildSessionRow>, String> {
    let parent_session_id = parent_session_id.trim();
    if parent_session_id.is_empty() {
        return Ok(Vec::new());
    }
    let path = axon_session::session_store_path(root).join(CLI_ROLLOUT_DB_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    // 读侧不跑建表语句：旧库还没有状态列时退回写连接补列一次，之后一律走只读路径。
    match RolloutSqliteStore::open_read_only(&path)?.select_child_sessions(parent_session_id, limit)
    {
        Ok(rows) => Ok(rows),
        Err(_) => RolloutSqliteStore::open(&path)?.select_child_sessions(parent_session_id, limit),
    }
}

pub(crate) fn install_rollout_compaction(
    config_path: &Path,
    session_id: &str,
    compaction: &ThreadCompaction,
) -> Result<(), String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    RolloutSqliteStore::open(&path)?.install_compaction(session_id, compaction)
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

/// 读取某会话最近若干轮的事实摘要；账本尚未建立时读作空。
///
/// 只读打开：这条路径挂在每轮收口上，不该顺带跑建表语句。
pub(crate) fn load_recent_turn_facts(
    config_path: &Path,
    session_id: &str,
    limit: u32,
) -> Result<Vec<super::RecentTurnFact>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    RolloutSqliteStore::open_read_only(&path)?.recent_turn_facts(session_id, limit)
}
