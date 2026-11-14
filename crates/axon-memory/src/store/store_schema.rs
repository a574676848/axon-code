//! Memory SQLite 连接引导与 schema 生命周期。
//!
//! 建库、WAL 配置、版本号与旧 schema 归档都收敛在这里；读写模块只消费已经初始化好的连接。

use std::collections::BTreeSet;
use std::path::Path;
use std::thread;
use std::time::Duration;

use axon_domain::{AxonError, AxonResult};
use rusqlite::{Connection, Transaction, TransactionBehavior};

use super::{
    LEGACY_MEMORY_RECORD_TABLE_PREFIX, MEMORY_SQLITE_BUSY_TIMEOUT_MS, MEMORY_SQLITE_SCHEMA_VERSION,
    REQUIRED_MEMORY_RECORD_COLUMNS,
};

const MEMORY_SQLITE_WAL_INIT_RETRIES: usize = 5;
const MEMORY_SQLITE_WAL_INIT_BACKOFF_MS: u64 = 20;
/// 初始化 schema 与 WAL。
pub(super) fn initialize_schema(connection: &Connection) -> AxonResult<()> {
    archive_incompatible_record_schema(connection)?;
    // 建表与建索引必须整体落在**一个 IMMEDIATE 事务**里：
    //
    // 1. 多个进程/线程同时打开同一个 Memory 库都会跑这段 DDL。自动提交模式下每条语句先
    //    取读快照再升级写锁，一旦别的连接在此期间提交了 schema 变更，SQLite 直接返回
    //    `SQLITE_BUSY_SNAPSHOT`，该错误**不进 busy handler**（无法等待，只能回滚重来），
    //    并发初始化就会随机报「schema 初始化失败」。`BEGIN IMMEDIATE` 一开始就取写锁，
    //    不存在读→写升级路径，等待由 busy_timeout 统一负责。
    // 2. 一次性提交让「表 + 索引 + 版本号」对其它进程原子可见，不会看到半成品 schema。
    let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)
        .map_err(|error| {
            AxonError::memory_persistence(format!("Memory SQLite schema 事务开启失败: {error}"))
        })?;
    transaction
        .execute_batch(
            r"
                CREATE TABLE IF NOT EXISTS memory_records (
                    memory_id INTEGER PRIMARY KEY NOT NULL,
                    candidate_id INTEGER NOT NULL,
                    memory_kind TEXT NOT NULL,
                    state TEXT NOT NULL,
                    content_ref TEXT,
                    source_origin TEXT,
                    source_session_id INTEGER,
                    source_cell_id INTEGER,
                    memory_summary TEXT,
                    evidence_ref TEXT,
                    tombstone_blocked_ref TEXT,
                    tombstone_reason TEXT,
                    namespace_ref TEXT,
                    repo_ref TEXT,
                    user_ref TEXT,
                    vector_id INTEGER UNIQUE,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    last_active_at_ms INTEGER NOT NULL,
                    ttl_expires_at_ms INTEGER,
                    confidence_basis_points INTEGER NOT NULL,
                    importance_basis_points INTEGER NOT NULL,
                    archived_at_ms INTEGER,
                    prompt_text_persisted INTEGER NOT NULL DEFAULT 0,
                    raw_payload_persisted INTEGER NOT NULL DEFAULT 0,
                    remote_write_allowed INTEGER NOT NULL DEFAULT 0
                );
                ",
        )
        .map_err(|error| {
            AxonError::memory_persistence(format!("Memory SQLite schema 初始化失败: {error}"))
        })?;
    transaction
        .execute_batch(
            r"
                CREATE INDEX IF NOT EXISTS idx_memory_records_active_score
                    ON memory_records(archived_at_ms, last_active_at_ms, confidence_basis_points);
                CREATE INDEX IF NOT EXISTS idx_memory_records_ttl
                    ON memory_records(ttl_expires_at_ms);
                CREATE INDEX IF NOT EXISTS idx_memory_records_scope
                    ON memory_records(repo_ref, namespace_ref, archived_at_ms);
                CREATE INDEX IF NOT EXISTS idx_memory_records_personal_scope
                    ON memory_records(user_ref, namespace_ref, state, archived_at_ms, ttl_expires_at_ms, updated_at_ms, memory_id);
                CREATE INDEX IF NOT EXISTS idx_memory_records_active_content_ref
                    ON memory_records(TRIM(content_ref), archived_at_ms);
                CREATE INDEX IF NOT EXISTS idx_memory_records_recallable_probe
                    ON memory_records(state, archived_at_ms, ttl_expires_at_ms, updated_at_ms, memory_id);
                CREATE INDEX IF NOT EXISTS idx_memory_records_kind_timeline
                    ON memory_records(memory_kind, updated_at_ms DESC, memory_id ASC);
                CREATE INDEX IF NOT EXISTS idx_memory_records_scope_kind_timeline
                    ON memory_records(namespace_ref, memory_kind, updated_at_ms DESC, memory_id ASC);
                ",
        )
        .map_err(|error| {
            AxonError::memory_persistence(format!("Memory SQLite 索引初始化失败: {error}"))
        })?;
    crate::store_supersession::initialize_supersession_schema(&transaction)?;
    crate::store_thread::initialize_thread_schema(&transaction)?;
    transaction
        .pragma_update(None, "user_version", MEMORY_SQLITE_SCHEMA_VERSION)
        .map_err(|error| {
            AxonError::memory_persistence(format!("Memory SQLite schema 版本写入失败: {error}"))
        })?;
    transaction.commit().map_err(|error| {
        AxonError::memory_persistence(format!("Memory SQLite schema 提交失败: {error}"))
    })
}

/// 归档缺少必需列的旧 `memory_records`，返回归档表名。
///
/// 旧库没有可信 `memory_kind` 来源，不能猜测填充；但既有记录属于用户数据，
/// 升级只重命名旧表并释放其索引名，绝不删除任何一行。
fn archive_incompatible_record_schema(connection: &Connection) -> AxonResult<Option<String>> {
    let columns = record_table_columns(connection)?;
    if columns.is_empty()
        || REQUIRED_MEMORY_RECORD_COLUMNS
            .iter()
            .all(|column| columns.contains(*column))
    {
        return Ok(None);
    }
    let legacy_table = format!("{LEGACY_MEMORY_RECORD_TABLE_PREFIX}{}", archiving_now_ms());
    connection
        .execute_batch(&format!(
            "ALTER TABLE memory_records RENAME TO \"{legacy_table}\""
        ))
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 旧 schema 归档失败"))?;
    for index in table_index_names(connection, &legacy_table)? {
        connection
            .execute_batch(&format!("DROP INDEX IF EXISTS \"{index}\""))
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 旧索引释放失败"))?;
    }
    Ok(Some(legacy_table))
}

pub(super) fn record_table_columns(connection: &Connection) -> AxonResult<BTreeSet<String>> {
    let mut statement = connection
        .prepare("PRAGMA table_info(memory_records)")
        .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 探测失败"))?;
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 探测失败"))?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 探测失败"))
}

pub(super) fn table_index_names(connection: &Connection, table: &str) -> AxonResult<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1")
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引探测失败"))?;
    statement
        .query_map([table], |row| row.get::<_, String>(0))
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引探测失败"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引探测失败"))
}

/// 列出升级时归档的旧记录表，用于向运维与界面显式提示。
pub(super) fn legacy_record_tables(connection: &Connection) -> AxonResult<Vec<String>> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE ?1 ORDER BY name ASC",
        )
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 归档表探测失败"))?;
    statement
        .query_map([format!("{LEGACY_MEMORY_RECORD_TABLE_PREFIX}%")], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 归档表探测失败"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 归档表探测失败"))
}

/// 归档表名使用的时间戳，单位毫秒；仅用于命名，不参与业务判定。
fn archiving_now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

pub(super) fn open_connection(path: &Path) -> AxonResult<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| AxonError::memory_persistence("无法创建 Memory SQLite 目录"))?;
    }
    let connection = Connection::open(path)
        .map_err(|_| AxonError::memory_persistence("无法打开 Memory SQLite 数据库"))?;
    connection
        .busy_timeout(Duration::from_millis(MEMORY_SQLITE_BUSY_TIMEOUT_MS))
        .map_err(|_| AxonError::memory_persistence("Memory SQLite busy timeout 配置失败"))?;
    let mut wal_ready = false;
    for attempt in 0..=MEMORY_SQLITE_WAL_INIT_RETRIES {
        if connection
            .pragma_update(None, "journal_mode", "WAL")
            .is_ok()
            || connection
                .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                .is_ok_and(|mode| mode.eq_ignore_ascii_case("wal"))
        {
            wal_ready = true;
            break;
        }
        if attempt < MEMORY_SQLITE_WAL_INIT_RETRIES {
            thread::sleep(Duration::from_millis(MEMORY_SQLITE_WAL_INIT_BACKOFF_MS));
        }
    }
    if !wal_ready {
        return Err(AxonError::memory_persistence("Memory SQLite WAL 配置失败"));
    }
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| AxonError::memory_persistence("Memory SQLite 外键配置失败"))?;
    Ok(connection)
}
