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

/// 一条会话最新一轮的模型事实。
///
/// 只保留界面显示与运行时选型需要的三个字段，不把整条轮次记录暴露给车道读数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionModelFacts {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) reasoning_effort: Option<String>,
}

/// 读取一条会话**最新一轮**的模型事实（含 provider、model 与 reasoning effort）。
///
/// 只读打开加一条 `ORDER BY sequence DESC LIMIT 1`：界面显示「这条会话现在用哪个
/// 模型」时读它，不遍历历史轮次，也不解析事件载荷。
pub(crate) fn load_latest_session_model_from_root(
    root: &Path,
    session_id: &str,
) -> Result<Option<SessionModelFacts>, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("会话标识不能为空".to_owned());
    }
    let path = axon_session::session_store_path(root).join(CLI_ROLLOUT_DB_FILE);
    if !path.exists() {
        return Ok(None);
    }
    Ok(RolloutSqliteStore::open_read_only(&path)?
        .load_by_id(session_id)?
        .map(|record| SessionModelFacts {
            provider: record.provider,
            model: record.model,
            reasoning_effort: record.reasoning_effort,
        }))
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

/// 读取某一轮在会话内的轮序，供生成 `trace:<session>:<sequence>` 使用。
///
/// 取的是 `rollout_turns.sequence`，即同一会话内的轮次下标，不是
/// `replay_chain` 的会话祖先链下标。治理队列是异步消费的：等某轮的 payload 被
/// 处理时，同一会话可能已经落了更多轮次，所以任何「按当前链长回推本轮下标」的
/// 写法都会指向别的轮次。必须按 `assistant_part_id` 点查。
pub(crate) fn load_turn_sequence(
    config_path: &Path,
    session_id: &str,
    assistant_part_id: &str,
) -> Result<Option<u64>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Ok(None);
    }
    RolloutSqliteStore::open(&path)?.turn_sequence(session_id, assistant_part_id)
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

/// 读上一轮的记忆反思提示；任何读取失败都读作「没有」。
///
/// 这条路径挂在每轮收口的输出构造上，不能因为它自身的问题让整轮失败。
pub(crate) fn load_previous_reflection_notice(
    config_path: &Path,
    session_id: &str,
    current_part_id: &str,
) -> Option<String> {
    let path = resolved_rollout_db_path(config_path).ok()?;
    if !path.exists() {
        return None;
    }
    RolloutSqliteStore::open_read_only(&path)
        .ok()?
        .previous_reflection_notice(session_id, current_part_id)
        .ok()
        .flatten()
}

/// 读某会话最新一轮的记忆反思提示；任何读取失败都读作「没有」。
///
/// 供只读接口投影给桌面端：它不能因为读不到提示而让整条历史请求失败。
pub(crate) fn load_latest_reflection_notice_from_root(
    root: &Path,
    session_id: &str,
) -> Option<String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }
    let path = axon_session::session_store_path(root).join(CLI_ROLLOUT_DB_FILE);
    if !path.exists() {
        return None;
    }
    RolloutSqliteStore::open_read_only(&path)
        .ok()?
        .latest_reflection_notice(session_id)
        .ok()
        .flatten()
}
