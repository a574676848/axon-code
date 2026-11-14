//! SQLite 结构化 rollout 会话存储，对齐 Codex 的结构化回放/resume。
//!
//! 本模块在 SQLite 中结构化持久化 rollout 会话：主字段独立成列以便查询，
//! tool_receipts 等复杂结构以 JSON 列存储。提供：
//! - `record_session`：结构化写入一次会话
//! - `load_by_id`：按 session_id 取回历史 rollout
//! - `replay_chain`：沿 parent_session_id 回溯，按时间正序回放整条会话链
//! - `list_sessions`：列出会话摘要

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use axon_agent_runtime::ContentArchiveRuntime;
use axon_session::{ThreadCompaction, session_store_path};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::run_session_store::{RunSessionRecord, resolved_axon_root};

#[path = "run_rollout_store/api.rs"]
mod api;
#[path = "run_rollout_store/rollout_events.rs"]
mod rollout_events;
#[path = "run_rollout_store/session_usage.rs"]
mod session_usage;
#[path = "run_rollout_store/transaction.rs"]
mod transaction;

#[cfg(test)]
pub(crate) use api::append_rollout_event_for_session;
pub(crate) use api::{
    commit_session_to_rollout, ensure_current_turn_to_rollout, install_active_rollout_window,
    install_rollout_compaction, load_latest_reflection_notice_from_root,
    load_latest_session_model_from_root, load_previous_reflection_notice, load_recent_turn_facts,
    load_rollout_history_window, load_rollout_history_window_from_root, load_session_turn,
    load_session_usage, load_session_usage_totals_from_root, load_turn_sequence,
    read_rollout_tool_output, replay_rollout_events, restore_rollout_compaction,
    select_rollout_session,
};
#[cfg(test)]
pub(crate) use rollout_events::RolloutEventAppendInput;
use rollout_events::events_for_session;
pub(crate) use rollout_events::{RolloutEventRecord, RolloutEventType};
pub(crate) use session_usage::SessionUsageTotals;

#[path = "run_rollout_store/retention.rs"]
mod retention;
#[path = "run_rollout_store/schema.rs"]
mod schema;

/// rollout SQLite 文件名。
pub(crate) const CLI_ROLLOUT_DB_FILE: &str = "rollout.sqlite3";
/// rollout SQLite schema 版本。
pub(crate) const ROLLOUT_SQLITE_SCHEMA_VERSION: i64 = 6;
/// 当前 Thread 选择表的单例键。
const CURRENT_SESSION_SINGLETON_KEY: i64 = 1;

pub(crate) struct RolloutHistoryWindow {
    pub(crate) compaction: Option<ThreadCompaction>,
    pub(crate) turns: Vec<(u64, RunSessionRecord)>,
}

/// 一轮的零模型事实摘要，供人格规则层与信号采集使用。
///
/// 只保留阈值判定需要的量：轮序、时间、输入长度、成败与既有状态序列。
/// 正文与工具输出一律不进来，避免把历史整条搬进收口路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentTurnFact {
    /// 会话内轮序。
    pub(crate) sequence: u64,
    /// 落盘时间，Unix 毫秒。
    pub(crate) recorded_at_ms: u64,
    /// 该轮用户输入的字符数。
    pub(crate) prompt_chars: u32,
    /// 该轮是否以失败收场。
    pub(crate) failed: bool,
    /// 该轮轨迹里记录的用户状态序列；未采集过时为空。
    pub(crate) affect_sequence: Vec<String>,
}

/// resume 链回放视图（可序列化，不泄漏内部 RunSessionRecord 结构）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolloutReplayView {
    /// 链中位置（0 为根）。
    pub(crate) depth: usize,
    /// 会话 ID。
    pub(crate) session_id: String,
    /// 父会话 ID。
    pub(crate) parent_session_id: Option<String>,
    /// 创建时间。
    pub(crate) created_at_ms: u64,
    /// provider 名称。
    pub(crate) provider: String,
    /// 模型 ID。
    pub(crate) model: String,
    /// prompt 字符数。
    pub(crate) prompt_chars: usize,
    /// response 字符数。
    pub(crate) response_chars: usize,
    pub(crate) startup_timeline: super::run_startup_snapshot::RunStartupTimeline,
}

/// rollout 会话摘要。
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RolloutSessionSummary {
    /// 会话 ID。
    pub(crate) session_id: String,
    /// 父会话 ID。
    pub(crate) parent_session_id: Option<String>,
    /// 创建时间。
    pub(crate) created_at_ms: u64,
    /// provider 名称。
    pub(crate) provider: String,
    /// 模型 ID。
    pub(crate) model: String,
}

/// SQLite 结构化 rollout store。
pub(crate) struct RolloutSqliteStore {
    connection: Connection,
}

impl RolloutSqliteStore {
    /// 打开并初始化 rollout store。
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let connection = schema::open_connection(path)?;
        let store = Self { connection };
        store.initialize_schema()?;
        Ok(store)
    }

    /// 以只读方式打开既有 rollout store，供 dry-run 候选统计使用。
    pub(crate) fn open_read_only(path: &Path) -> Result<Self, String> {
        let connection = schema::open_read_only_connection(path)?;
        Ok(Self { connection })
    }

    /// 原子写入会话最新状态、turn、事件、标题与 wire resume snapshot。
    #[cfg(test)]
    pub(crate) fn record_session(&self, record: &RunSessionRecord) -> Result<(), String> {
        self.commit_session(record, None)
    }

    /// 按 session_id 取回会话。
    pub(crate) fn load_by_id(&self, session_id: &str) -> Result<Option<RunSessionRecord>, String> {
        let json = self
            .connection
            .query_row(
                "SELECT record_json FROM rollout_turns
                 WHERE session_id = ?1
                 ORDER BY sequence DESC
                 LIMIT 1",
                params![session_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| "rollout 会话读取失败".to_owned())?;
        json.map(|value| {
            serde_json::from_str(&value).map_err(|_| "rollout 会话 JSON 无效".to_owned())
        })
        .transpose()
    }

    /// 按 session 与 assistant part 的治理幂等键精确取回单个 turn。
    pub(crate) fn load_turn_by_id(
        &self,
        session_id: &str,
        assistant_part_id: &str,
    ) -> Result<Option<RunSessionRecord>, String> {
        let json = self
            .connection
            .query_row(
                "SELECT record_json FROM rollout_turns
                 WHERE session_id = ?1 AND assistant_part_id = ?2
                 LIMIT 1",
                params![session_id, assistant_part_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| "rollout turn 读取失败".to_owned())?;
        json.map(|value| {
            serde_json::from_str(&value).map_err(|_| "rollout turn JSON 无效".to_owned())
        })
        .transpose()
    }

    /// 按会话内轮序读取单轮记录。
    ///
    /// 这里的 `sequence` 是 `rollout_turns.sequence`，即同一会话内的轮次下标，
    /// 与 `replay_chain` 的会话祖先链下标是两套编号，不可互换。
    pub(crate) fn load_turn_by_sequence(
        &self,
        session_id: &str,
        sequence: u64,
    ) -> Result<Option<RunSessionRecord>, String> {
        let json = self
            .connection
            .query_row(
                "SELECT record_json FROM rollout_turns
                 WHERE session_id = ?1 AND sequence = ?2
                 LIMIT 1",
                params![session_id, i64::try_from(sequence).unwrap_or(i64::MAX)],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| "rollout turn 读取失败".to_owned())?;
        json.map(|value| {
            serde_json::from_str(&value).map_err(|_| "rollout turn JSON 无效".to_owned())
        })
        .transpose()
    }

    /// 读取某一轮在会话内的轮序；该轮未落盘时返回 `None`。
    pub(crate) fn turn_sequence(
        &self,
        session_id: &str,
        assistant_part_id: &str,
    ) -> Result<Option<u64>, String> {
        let sequence = self
            .connection
            .query_row(
                "SELECT sequence FROM rollout_turns
                 WHERE session_id = ?1 AND assistant_part_id = ?2
                 LIMIT 1",
                params![session_id, assistant_part_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|_| "rollout turn 轮序读取失败".to_owned())?;
        Ok(sequence.and_then(|value| u64::try_from(value).ok()))
    }

    /// 按轮序倒序读取最近若干轮的事实摘要。
    ///
    /// 只做标量投影，不把 `record_json` 整条反序列化：这条路径每轮收口都跑一次，
    /// 而单条 record 含完整工具回执与事件流，几十条就是数百 KB，解析成本远超它
    /// 提供的信息量。`json_extract` 直接在 SQLite 侧取到需要的那几个字段。
    ///
    /// 坏行跳过而不是整体失败：一条历史记录损坏不该让本轮信号全部归零。
    pub(crate) fn recent_turn_facts(
        &self,
        session_id: &str,
        limit: u32,
    ) -> Result<Vec<RecentTurnFact>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT sequence, recorded_at_ms,
                        COALESCE(length(json_extract(record_json, '$.prompt')), 0),
                        json_extract(record_json, '$.failure') IS NOT NULL,
                        COALESCE(json_extract(record_json, '$.turnTrace.signals.affectSequence'), '[]')
                 FROM rollout_turns
                 WHERE session_id = ?1 ORDER BY sequence DESC LIMIT ?2",
            )
            .map_err(|_| "rollout 轮次事实查询准备失败".to_owned())?;
        let rows = statement
            .query_map(params![session_id, i64::from(limit)], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|_| "rollout 轮次事实查询失败".to_owned())?;
        let mut facts = Vec::new();
        for row in rows {
            let (sequence, recorded_at_ms, prompt_chars, failed, affect_json) =
                row.map_err(|_| "rollout 轮次事实行读取失败".to_owned())?;
            facts.push(RecentTurnFact {
                sequence: u64::try_from(sequence).unwrap_or(0),
                recorded_at_ms: u64::try_from(recorded_at_ms).unwrap_or(0),
                prompt_chars: u32::try_from(prompt_chars).unwrap_or(0),
                failed,
                affect_sequence: serde_json::from_str(&affect_json).unwrap_or_default(),
            });
        }
        Ok(facts)
    }

    /// 读同一会话里「本轮之外最近一轮」的记忆反思提示。
    ///
    /// 反思是轮后异步治理的产物，本轮运行期拿不到自己那条；能确定看到的只有上一轮。
    /// 只投影 `reflectionNotice` 一个标量，不反序列化整条 record。
    pub(crate) fn previous_reflection_notice(
        &self,
        session_id: &str,
        current_part_id: &str,
    ) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT json_extract(record_json, '$.reflectionNotice')
                 FROM rollout_turns
                 WHERE session_id = ?1 AND assistant_part_id <> ?2
                 ORDER BY sequence DESC LIMIT 1",
                params![session_id, current_part_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| "rollout 反思提示读取失败".to_owned())
            .map(Option::flatten)
    }

    /// 读某会话**最新一轮**的记忆反思提示；该轮没有提示时返回 `None`。
    ///
    /// 与 [`Self::previous_reflection_notice`] 的差别只在取舍：这里要「这条会话现在
    /// 能对外展示的最新鲜结果」，供只读接口投影给桌面；那里要「上一轮」，供本轮收口
    /// 输出携带。两者都只投影一个标量，不反序列化整条 record。
    pub(crate) fn latest_reflection_notice(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT json_extract(record_json, '$.reflectionNotice')
                 FROM rollout_turns
                 WHERE session_id = ?1
                 ORDER BY sequence DESC LIMIT 1",
                params![session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|_| "rollout 反思提示读取失败".to_owned())
            .map(Option::flatten)
    }

    /// 仅允许读取指定 session 中已持久化 provider receipt 关联的完整工具输出。
    pub(crate) fn read_tool_output(
        &self,
        archive_root: &Path,
        session_id: &str,
        provider_call_id: &str,
    ) -> Result<Vec<u8>, String> {
        let record = self
            .load_by_id(session_id)?
            .ok_or_else(|| format!("rollout 中未找到 session：{session_id}"))?;
        let mut receipts = record
            .tool_receipts
            .iter()
            .filter(|receipt| receipt.provider_call_id == provider_call_id);
        let receipt = receipts
            .next()
            .ok_or_else(|| format!("session 中未找到 provider receipt：{provider_call_id}"))?;
        if receipts.next().is_some() {
            return Err(format!(
                "session 中 provider receipt 不唯一，拒绝读取：{provider_call_id}"
            ));
        }
        let persisted_ref = receipt
            .output_ref
            .as_ref()
            .ok_or_else(|| "provider receipt 未包含 OutputRef".to_owned())?;
        ContentArchiveRuntime::default()
            .read_output(archive_root, persisted_ref)
            .map_err(|error| format!("工具输出归档读取失败：{error}"))
    }

    /// 取当前会话；旧库尚未选择时回退到最新根会话。
    pub(crate) fn load_latest(&self) -> Result<Option<RunSessionRecord>, String> {
        let selected_json = self
            .connection
            .query_row(
                r"
                SELECT turns.record_json
                FROM rollout_current_session AS current
                INNER JOIN rollout_sessions AS sessions
                    ON sessions.session_id = current.session_id
                INNER JOIN rollout_turns AS turns
                    ON turns.session_id = sessions.session_id
                WHERE current.singleton_key = ?1
                ORDER BY turns.sequence DESC
                LIMIT 1
                ",
                params![CURRENT_SESSION_SINGLETON_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|_| "rollout 当前会话读取失败".to_owned())?;
        let json = match selected_json {
            Some(json) => Some(json),
            None => self
                .connection
                .query_row(
                    r"
                    SELECT turns.record_json
                    FROM rollout_sessions AS sessions
                    INNER JOIN rollout_turns AS turns
                        ON turns.session_id = sessions.session_id
                    WHERE sessions.parent_session_id IS NULL
                    ORDER BY sessions.updated_at_ms DESC, sessions.rowid DESC, turns.sequence DESC
                    LIMIT 1
                    ",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| "rollout 最新根会话读取失败".to_owned())?,
        };
        json.map(|value| {
            serde_json::from_str(&value).map_err(|_| "rollout 会话 JSON 无效".to_owned())
        })
        .transpose()
    }

    /// 显式选择后续 `--continue` 使用的 Thread。
    pub(crate) fn select_session(&self, session_id: &str) -> Result<(), String> {
        let changed = self
            .connection
            .execute(
                r"
                INSERT INTO rollout_current_session (singleton_key, session_id)
                SELECT ?1, session_id
                FROM rollout_sessions
                WHERE session_id = ?2
                ON CONFLICT(singleton_key) DO UPDATE SET session_id = excluded.session_id
                ",
                params![CURRENT_SESSION_SINGLETON_KEY, session_id],
            )
            .map_err(|_| "rollout 当前会话选择失败".to_owned())?;
        if changed == 0 {
            return Err(format!("rollout 中未找到会话：{session_id}"));
        }
        Ok(())
    }

    /// 回放整条 resume 链：从给定 session 沿 parent 回溯到根，按时间正序返回。
    pub(crate) fn replay_chain(&self, session_id: &str) -> Result<Vec<RunSessionRecord>, String> {
        let mut chain = Vec::new();
        let mut current = Some(session_id.to_owned());
        let mut visited = HashSet::new();
        while let Some(id) = current {
            if !visited.insert(id.clone()) {
                return Err("rollout resume 链存在环".to_owned());
            }
            let Some(record) = self.load_by_id(&id)? else {
                break;
            };
            let parent = record.parent_session_id.clone();
            chain.push(record);
            current = parent;
        }
        // 回溯得到的是「子→父」顺序，反转为按时间正序（根在前）。
        chain.reverse();
        Ok(chain)
    }

    /// 列出会话摘要，按创建时间倒序。
    #[cfg(test)]
    pub(crate) fn list_sessions(&self) -> Result<Vec<RolloutSessionSummary>, String> {
        let mut statement = self
            .connection
            .prepare(
                r"
                SELECT session_id, parent_session_id, updated_at_ms, provider, model
                FROM rollout_sessions
                ORDER BY updated_at_ms DESC, rowid DESC
                ",
            )
            .map_err(|_| "rollout 会话摘要查询准备失败".to_owned())?;
        let rows = statement
            .query_map([], |row| {
                let updated_at_ms = u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0);
                Ok(RolloutSessionSummary {
                    session_id: row.get(0)?,
                    parent_session_id: row.get(1)?,
                    created_at_ms: updated_at_ms,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                })
            })
            .map_err(|_| "rollout 会话摘要查询失败".to_owned())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| "rollout 会话摘要解析失败".to_owned())
    }
}

/// 解析 rollout SQLite 文件路径。
pub(crate) fn resolved_rollout_db_path(config_path: &Path) -> Result<PathBuf, &'static str> {
    Ok(session_store_path(&resolved_axon_root(config_path)?).join(CLI_ROLLOUT_DB_FILE))
}

/// 回放 resume 链，返回按时间正序（根在前）的可序列化视图。
pub(crate) fn replay_session_chain(
    config_path: &Path,
    session_id: &str,
) -> Result<Vec<RolloutReplayView>, String> {
    let path = resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    if !path.exists() {
        return Err("rollout SQLite 存储尚未建立".to_owned());
    }
    let store = RolloutSqliteStore::open(&path)?;
    let chain = store.replay_chain(session_id)?;
    if chain.is_empty() {
        return Err(format!("未找到会话：{session_id}"));
    }
    Ok(chain
        .into_iter()
        .enumerate()
        .map(|(depth, record)| RolloutReplayView {
            depth,
            session_id: record.session_id,
            parent_session_id: record.parent_session_id,
            created_at_ms: record.created_at_ms,
            provider: record.provider,
            model: record.model,
            prompt_chars: record.prompt.chars().count(),
            response_chars: record.response.chars().count(),
            startup_timeline: record.startup_timeline,
        })
        .collect())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_output_tests;
