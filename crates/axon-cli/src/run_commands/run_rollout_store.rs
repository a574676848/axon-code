//! SQLite 结构化 rollout 会话存储，对齐 Codex 的结构化回放/resume。
//!
//! 本模块在 SQLite 中结构化持久化 rollout 会话：主字段独立成列以便查询，
//! tool_receipts 等复杂结构以 JSON 列存储。提供：
//! - `record_session`：结构化写入一次会话
//! - `load_by_id`：按 session_id 取回历史 rollout
//! - `replay_chain`：沿 parent_session_id 回溯，按时间正序回放整条会话链
//! - `list_sessions`：列出会话摘要

use std::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use axon_agent_runtime::ContentArchiveRuntime;
use axon_session::{ThreadCompaction, session_store_path};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};

use super::run_session_store::{RunSessionRecord, resolved_axon_root};

#[path = "run_rollout_store/api.rs"]
mod api;
#[path = "run_rollout_store/rollout_events.rs"]
mod rollout_events;
#[path = "run_rollout_store/transaction.rs"]
mod transaction;

#[cfg(test)]
pub(crate) use api::append_rollout_event_for_session;
pub(crate) use api::{
    commit_session_to_rollout, install_active_rollout_window, install_rollout_compaction,
    load_rollout_history_window, load_rollout_history_window_from_root, load_session_turn,
    load_session_usage, read_rollout_tool_output, replay_rollout_events,
    restore_rollout_compaction, select_rollout_session,
};
#[cfg(test)]
pub(crate) use rollout_events::RolloutEventAppendInput;
use rollout_events::events_for_session;
pub(crate) use rollout_events::{RolloutEventRecord, RolloutEventType};

/// rollout SQLite 文件名。
pub(crate) const CLI_ROLLOUT_DB_FILE: &str = "rollout.sqlite3";
/// rollout SQLite schema 版本。
pub(crate) const ROLLOUT_SQLITE_SCHEMA_VERSION: i64 = 6;
/// 当前 Thread 选择表的单例键。
const CURRENT_SESSION_SINGLETON_KEY: i64 = 1;
/// rollout SQLite busy timeout，单位毫秒。
const ROLLOUT_SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
/// 每轮会话清理最多回收的 SQLite 空闲页数。
const ROLLOUT_INCREMENTAL_VACUUM_PAGE_BATCH: u32 = 256;
const ROLLOUT_CLEANUP_BATCH_LIMIT: usize = 256;
/// rollout_sessions 目标 schema 必需列。
const ROLLOUT_SESSION_COLUMNS: &[&str] = &[
    "session_id",
    "parent_session_id",
    "created_at_ms",
    "updated_at_ms",
    "provider",
    "model",
    "title",
    "history_checkpoint_json",
];

pub(super) struct RolloutHistoryWindow {
    pub(super) compaction: Option<ThreadCompaction>,
    pub(super) turns: Vec<(u64, RunSessionRecord)>,
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

/// rollout 会话清理结果。
#[derive(Debug, Default)]
pub(crate) struct RolloutCleanupReport {
    /// 已删除的会话 ID。
    pub(crate) session_ids: Vec<String>,
    /// 是否完成空间回收。
    pub(crate) vacuumed: bool,
}

#[derive(Debug)]
struct RolloutSessionMetadata {
    session_id: String,
    parent_session_id: Option<String>,
    updated_at_ms: i64,
}

impl RolloutSqliteStore {
    /// 打开并初始化 rollout store。
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let connection = open_connection(path)?;
        let store = Self { connection };
        store.initialize_schema()?;
        Ok(store)
    }

    /// 以只读方式打开既有 rollout store，供 dry-run 候选统计使用。
    pub(crate) fn open_read_only(path: &Path) -> Result<Self, String> {
        let connection = open_read_only_connection(path)?;
        Ok(Self { connection })
    }

    /// 只读查找已过期且不属于当前会话树的 rollout 会话家族。
    pub(crate) fn stale_session_family_ids(
        &mut self,
        cutoff_ms: u64,
        protected_session_ids: &HashSet<String>,
    ) -> Result<Vec<String>, String> {
        let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
        let transaction = self
            .connection
            .transaction()
            .map_err(|_| "rollout 清理查询事务启动失败".to_owned())?;
        let stale_ids =
            stale_session_ids_in_transaction(&transaction, cutoff_ms, protected_session_ids)?;
        transaction
            .commit()
            .map_err(|_| "rollout 清理查询事务提交失败".to_owned())?;
        Ok(stale_ids)
    }

    /// 在附属存储清理完成后，重新校验并原子删除仍过期的候选会话家族。
    pub(crate) fn finalize_stale_session_family_removal(
        &mut self,
        candidate_session_ids: &[String],
        cutoff_ms: u64,
        protected_session_ids: &HashSet<String>,
    ) -> Result<RolloutCleanupReport, String> {
        if candidate_session_ids.is_empty() {
            return Ok(RolloutCleanupReport::default());
        }
        let cutoff_ms = i64::try_from(cutoff_ms).unwrap_or(i64::MAX);
        let candidates = candidate_session_ids.iter().collect::<HashSet<_>>();
        let mut transaction = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "rollout 清理事务启动失败".to_owned())?;
        let stale_ids =
            stale_session_ids_in_transaction(&transaction, cutoff_ms, protected_session_ids)?
                .into_iter()
                .filter(|session_id| candidates.contains(session_id))
                .collect::<Vec<_>>();
        if stale_ids.is_empty() {
            transaction
                .commit()
                .map_err(|_| "rollout 空清理事务提交失败".to_owned())?;
            return Ok(RolloutCleanupReport::default());
        }
        let first_batch_len = stale_ids.len().min(ROLLOUT_CLEANUP_BATCH_LIMIT);
        delete_rollout_session_rows(&transaction, &stale_ids[..first_batch_len])?;
        transaction
            .commit()
            .map_err(|_| "rollout 会话清理事务提交失败".to_owned())?;
        for ids in stale_ids[first_batch_len..].chunks(ROLLOUT_CLEANUP_BATCH_LIMIT) {
            transaction = self
                .connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|_| "rollout 清理事务启动失败".to_owned())?;
            delete_rollout_session_rows(&transaction, ids)?;
            transaction
                .commit()
                .map_err(|_| "rollout 会话清理事务提交失败".to_owned())?;
        }
        reclaim_rollout_space(&self.connection)?;
        Ok(RolloutCleanupReport {
            session_ids: stale_ids,
            vacuumed: true,
        })
    }

    fn initialize_schema(&self) -> Result<(), String> {
        self.connection
            .pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .map_err(|_| "rollout SQLite 自动空间回收配置失败".to_owned())?;
        if self.rollout_sessions_needs_rebuild()? {
            self.connection
                .execute_batch(
                    r"
                    DROP TABLE IF EXISTS rollout_events;
                    DROP TABLE IF EXISTS rollout_turns;
                    DROP TABLE IF EXISTS rollout_sessions;
                    ",
                )
                .map_err(|_| "rollout SQLite 旧 schema 清理失败".to_owned())?;
        }
        self.connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS rollout_sessions (
                    session_id TEXT PRIMARY KEY NOT NULL,
                    parent_session_id TEXT,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    provider TEXT NOT NULL,
                    model TEXT NOT NULL,
                    title TEXT,
                    history_checkpoint_json TEXT
                );
                CREATE TABLE IF NOT EXISTS rollout_turns (
                    session_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    assistant_part_id TEXT NOT NULL,
                    recorded_at_ms INTEGER NOT NULL,
                    record_json TEXT NOT NULL,
                    PRIMARY KEY (session_id, sequence),
                    UNIQUE (session_id, assistant_part_id),
                    FOREIGN KEY (session_id) REFERENCES rollout_sessions(session_id) ON DELETE CASCADE
                );
                CREATE TABLE IF NOT EXISTS rollout_events (
                    session_id TEXT NOT NULL,
                    sequence INTEGER NOT NULL,
                    event_type TEXT NOT NULL,
                    recorded_at_ms INTEGER NOT NULL,
                    event_json TEXT NOT NULL,
                    PRIMARY KEY (session_id, sequence)
                );
                CREATE TABLE IF NOT EXISTS rollout_current_session (
                    singleton_key INTEGER PRIMARY KEY NOT NULL CHECK (singleton_key = 1),
                    session_id TEXT NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES rollout_sessions(session_id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_rollout_created_at
                    ON rollout_sessions (updated_at_ms);
                CREATE INDEX IF NOT EXISTS idx_rollout_parent
                    ON rollout_sessions (parent_session_id);
                ",
            )
            .map_err(|_| "rollout SQLite schema 初始化失败".to_owned())?;
        self.connection
            .pragma_update(None, "user_version", ROLLOUT_SQLITE_SCHEMA_VERSION)
            .map_err(|_| "rollout SQLite schema 版本配置失败".to_owned())?;
        Ok(())
    }

    fn rollout_sessions_needs_rebuild(&self) -> Result<bool, String> {
        let current_version: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| "rollout SQLite schema 版本读取失败".to_owned())?;
        let columns = self.rollout_session_columns()?;
        if columns.is_empty() {
            return Ok(current_version != ROLLOUT_SQLITE_SCHEMA_VERSION);
        }
        Ok(current_version != ROLLOUT_SQLITE_SCHEMA_VERSION
            || ROLLOUT_SESSION_COLUMNS
                .iter()
                .any(|required| !columns.iter().any(|column| column == required)))
    }

    fn rollout_session_columns(&self) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare("PRAGMA table_info(rollout_sessions)")
            .map_err(|_| "rollout SQLite schema 检查准备失败".to_owned())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|_| "rollout SQLite schema 检查失败".to_owned())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| "rollout SQLite schema 列解析失败".to_owned())
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

fn session_root(session_id: &str, parents: &HashMap<String, Option<String>>) -> String {
    let mut current = session_id.to_owned();
    let mut visited = HashSet::new();
    while let Some(Some(parent)) = parents.get(&current) {
        if !visited.insert(current.clone()) || !parents.contains_key(parent) {
            break;
        }
        current.clone_from(parent);
    }
    current
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

fn open_connection(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "无法创建 rollout SQLite 目录".to_owned())?;
    }
    let connection =
        Connection::open(path).map_err(|_| "无法打开 rollout SQLite 数据库".to_owned())?;
    connection
        .busy_timeout(Duration::from_millis(ROLLOUT_SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|_| "rollout SQLite busy timeout 配置失败".to_owned())?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(|_| "rollout SQLite WAL 配置失败".to_owned())?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| "rollout SQLite foreign_keys 配置失败".to_owned())?;
    Ok(connection)
}

fn open_read_only_connection(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| "无法只读打开 rollout SQLite 数据库".to_owned())?;
    connection
        .busy_timeout(Duration::from_millis(ROLLOUT_SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|_| "rollout SQLite busy timeout 配置失败".to_owned())?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| "rollout SQLite foreign_keys 配置失败".to_owned())?;
    Ok(connection)
}

fn read_rollout_session_metadata(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<RolloutSessionMetadata>, String> {
    let mut statement = transaction
        .prepare("SELECT session_id, parent_session_id, updated_at_ms FROM rollout_sessions")
        .map_err(|_| "rollout 会话清理查询失败".to_owned())?;
    let rows = statement
        .query_map([], |row| {
            Ok(RolloutSessionMetadata {
                session_id: row.get(0)?,
                parent_session_id: row.get(1)?,
                updated_at_ms: row.get(2)?,
            })
        })
        .map_err(|_| "rollout 会话清理查询失败".to_owned())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "rollout 会话清理记录读取失败".to_owned())
}

fn stale_session_ids_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    cutoff_ms: i64,
    protected_session_ids: &HashSet<String>,
) -> Result<Vec<String>, String> {
    let current_session_id = transaction
        .query_row(
            "SELECT session_id FROM rollout_current_session WHERE singleton_key = ?1",
            [CURRENT_SESSION_SINGLETON_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| "rollout 当前会话读取失败".to_owned())?;
    let sessions = read_rollout_session_metadata(transaction)?;
    Ok(stale_session_ids(
        sessions,
        current_session_id.as_deref(),
        protected_session_ids,
        cutoff_ms,
    ))
}

fn stale_session_ids(
    sessions: Vec<RolloutSessionMetadata>,
    current_session_id: Option<&str>,
    protected_session_ids: &HashSet<String>,
    cutoff_ms: i64,
) -> Vec<String> {
    let parents = sessions
        .iter()
        .map(|session| {
            (
                session.session_id.clone(),
                session.parent_session_id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let protected_root = current_session_id.map(|session_id| session_root(session_id, &parents));
    let mut families = HashMap::<String, Vec<RolloutSessionMetadata>>::new();
    for session in sessions {
        let root = session_root(&session.session_id, &parents);
        families.entry(root).or_default().push(session);
    }
    let mut stale_ids = families
        .into_iter()
        .filter(|(root, members)| {
            protected_root.as_deref() != Some(root.as_str())
                && !members
                    .iter()
                    .any(|session| protected_session_ids.contains(&session.session_id))
                && members
                    .iter()
                    .all(|session| session.updated_at_ms < cutoff_ms)
        })
        .flat_map(|(_, members)| members.into_iter().map(|session| session.session_id))
        .collect::<Vec<_>>();
    stale_ids.sort();
    stale_ids
}

fn delete_rollout_session_rows(
    transaction: &rusqlite::Transaction<'_>,
    session_ids: &[String],
) -> Result<(), String> {
    for session_id in session_ids {
        for (table, message) in [
            ("rollout_events", "rollout 事件清理失败"),
            ("rollout_turns", "rollout turn 清理失败"),
            ("rollout_sessions", "rollout 会话清理失败"),
        ] {
            transaction
                .execute(
                    &format!("DELETE FROM {table} WHERE session_id = ?1"),
                    [session_id],
                )
                .map_err(|_| message.to_owned())?;
        }
    }
    Ok(())
}

fn reclaim_rollout_space(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(&format!(
            "PRAGMA incremental_vacuum({ROLLOUT_INCREMENTAL_VACUUM_PAGE_BATCH})"
        ))
        .map_err(|_| "rollout SQLite 增量空间回收失败".to_owned())
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_output_tests;
