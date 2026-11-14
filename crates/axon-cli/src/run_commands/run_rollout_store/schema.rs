//! rollout SQLite 连接引导与 schema 生命周期。
//!
//! 建库、版本校验与不兼容 schema 重建都收敛在这里，业务读写只使用本模块给出的连接。

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::ROLLOUT_SQLITE_SCHEMA_VERSION;

/// rollout SQLite busy timeout，单位毫秒。
const ROLLOUT_SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
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

impl super::RolloutSqliteStore {
    /// 初始化 schema：必要时重建不兼容的旧库，再建表、建索引并写入版本号。
    pub(super) fn initialize_schema(&self) -> Result<(), String> {
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
        super::session_usage::ensure_usage_columns(&self.connection)?;
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
}

pub(super) fn open_connection(path: &Path) -> Result<Connection, String> {
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

pub(super) fn open_read_only_connection(path: &Path) -> Result<Connection, String> {
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
