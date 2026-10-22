//! SQLite 结构化 Memory store。

use std::{collections::BTreeSet, path::Path, thread, time::Duration};

use axon_domain::{
    AxonError, AxonResult, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin, MemoryState,
};
#[cfg(test)]
use axon_domain::{CellId, MemoryCandidateId, SessionId};
use rusqlite::{Connection, OptionalExtension as _, params, params_from_iter};

use crate::MemoryConfig;

mod store_codec;
pub(crate) use store_codec::{
    bool_from_i64, i64_from_u64, i64_from_usize, optional_i64_from_u64, origin_code, row_to_record,
    sqlite_bool, state_code, u64_from_i64, unique_positive_i64s, unique_positive_i64s_from_i64,
    usize_from_i64, validate_record,
};

/// SQLite schema 版本。
pub const MEMORY_SQLITE_SCHEMA_VERSION: i64 = 4;
/// SQLite busy timeout，单位毫秒。
pub const MEMORY_SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
const MEMORY_SQLITE_WAL_INIT_RETRIES: usize = 5;
const MEMORY_SQLITE_WAL_INIT_BACKOFF_MS: u64 = 20;
/// SQLite bool true。
const SQLITE_TRUE: i64 = 1;
/// SQLite bool false。
const SQLITE_FALSE: i64 = 0;
/// SQLite basis point 最大值。
const MAX_BASIS_POINTS: u16 = 10_000;
const MEMORY_SQLITE_QUERY_BATCH_SIZE: usize = 256;
const FIELD_MEMORY_ID: &str = "memory_id";
const FIELD_VECTOR_ID: &str = "vector_id";
const REQUIRED_MEMORY_RECORD_COLUMNS: [&str; 5] = [
    "memory_summary",
    "namespace_ref",
    "repo_ref",
    "user_ref",
    "memory_kind",
];
/// 旧 schema 归档表名前缀；升级不再删除既有记录。
pub const LEGACY_MEMORY_RECORD_TABLE_PREFIX: &str = "memory_records_legacy_";

/// 管理页可见范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryVisibility<'a> {
    /// 全库可见，不做 owner 过滤（CLI 运维视图）。
    All,
    /// 仅 Workspace 记录（`user_ref IS NULL`）。
    Workspace,
    /// 仅指定 owner 的 Personal 记录。
    Personal(&'a str),
    /// 管理页可见面：Workspace 记录 + 指定 owner 的 Personal 记录。
    OwnerScoped(&'a str),
}

impl<'a> MemoryVisibility<'a> {
    /// 返回白名单化的 SQL 谓词；owner 以 `:owner` 具名参数绑定。
    pub(crate) const fn predicate(self) -> &'static str {
        match self {
            Self::All => "1 = 1",
            Self::Workspace => "user_ref IS NULL",
            Self::Personal(_) => "user_ref = :owner",
            Self::OwnerScoped(_) => "(user_ref IS NULL OR user_ref = :owner)",
        }
    }

    /// 返回需要绑定的 owner 引用。
    pub(crate) const fn owner(self) -> Option<&'a str> {
        match self {
            Self::All | Self::Workspace => None,
            Self::Personal(owner) | Self::OwnerScoped(owner) => Some(owner),
        }
    }
}

/// 单个 scope 的记录聚合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryScopeFacet {
    /// scope namespace 引用。
    pub namespace_ref: String,
    /// `personal` 或 `workspace`。
    pub scope_kind: &'static str,
    /// 记录数量。
    pub record_count: usize,
}

/// SQLite 结构化 Memory 记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySqliteRecord {
    /// 治理记录。
    pub record: MemoryRecord,
    /// memory_write 的稳定五值类型。
    pub memory_kind: MemoryKind,
    /// namespace scope 引用。
    pub namespace_ref: Option<String>,
    /// repo scope 引用。
    pub repo_ref: Option<String>,
    /// user scope 引用。
    pub user_ref: Option<String>,
    /// 写入时间。
    pub created_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 最近活跃时间。
    pub last_active_at_ms: u64,
    /// TTL 过期时间。
    pub ttl_expires_at_ms: Option<u64>,
    /// 置信度，单位 basis point。
    pub confidence_basis_points: u16,
    /// 重要度，单位 basis point。
    pub importance_basis_points: u16,
    /// 向量索引 ID；无 embedding 时为 None。
    pub vector_id: Option<i64>,
    /// 归档时间；None 表示活跃。
    pub archived_at_ms: Option<u64>,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化 provider/远控原始 payload。
    pub raw_payload_persisted: bool,
    /// 是否允许远控直接写入。
    pub remote_write_allowed: bool,
}

/// SQLite store 汇总。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySqliteSummary {
    /// 总记录数。
    pub total_records: usize,
    /// 活跃记录数。
    pub active_records: usize,
    /// 归档记录数。
    pub archived_records: usize,
    /// 是否有向量索引 ID。
    pub vector_records: usize,
    /// 是否持久化 prompt 原文。
    pub prompt_text_persisted: bool,
    /// 是否持久化原始 payload。
    pub raw_payload_persisted: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
}

/// 时间轴分页游标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryTimelineCursor {
    /// 上一页最后一条记录的更新时间。
    pub updated_at_ms: u64,
    /// 上一页最后一条记录的 ID。
    pub memory_id: u64,
}

/// 按 MemoryKind 聚合的记录数量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryKindCount {
    /// 记忆类型。
    pub memory_kind: MemoryKind,
    /// 记录数量。
    pub count: usize,
}

/// SQLite 记录列表筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySqliteRecordFilter {
    /// 不筛选。
    All,
    /// 未归档记录。
    Active,
    /// 已归档记录。
    Archived,
    /// Strong 记录。
    Strong,
    /// Weak 记录。
    Weak,
    /// Quarantined 记录。
    Quarantined,
    /// Tombstone 记录。
    Tombstone,
}

/// SQLite 结构化 Memory store。
pub struct MemorySqliteStore {
    pub(crate) connection: Connection,
}

impl MemorySqliteStore {
    /// 打开并配置 SQLite store。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        open_connection(&config.db_path).map(|connection| Self { connection })
    }

    /// 初始化 schema 与 WAL。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        self.archive_incompatible_record_schema()?;
        self.connection
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
            .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 初始化失败"))?;
        self.connection
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
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引初始化失败"))?;
        crate::store_supersession::initialize_supersession_schema(&self.connection)?;
        self.connection
            .pragma_update(None, "user_version", MEMORY_SQLITE_SCHEMA_VERSION)
            .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 版本写入失败"))
    }

    /// 归档缺少必需列的旧 `memory_records`，返回归档表名。
    ///
    /// 旧库没有可信 `memory_kind` 来源，不能猜测填充；但既有记录属于用户数据，
    /// 升级只重命名旧表并释放其索引名，绝不删除任何一行。
    fn archive_incompatible_record_schema(&self) -> AxonResult<Option<String>> {
        let columns = self.record_table_columns()?;
        if columns.is_empty()
            || REQUIRED_MEMORY_RECORD_COLUMNS
                .iter()
                .all(|column| columns.contains(*column))
        {
            return Ok(None);
        }
        let legacy_table = format!("{LEGACY_MEMORY_RECORD_TABLE_PREFIX}{}", archiving_now_ms());
        self.connection
            .execute_batch(&format!(
                "ALTER TABLE memory_records RENAME TO \"{legacy_table}\""
            ))
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 旧 schema 归档失败"))?;
        for index in self.table_index_names(&legacy_table)? {
            self.connection
                .execute_batch(&format!("DROP INDEX IF EXISTS \"{index}\""))
                .map_err(|_| AxonError::memory_persistence("Memory SQLite 旧索引释放失败"))?;
        }
        Ok(Some(legacy_table))
    }

    fn record_table_columns(&self) -> AxonResult<BTreeSet<String>> {
        let mut statement = self
            .connection
            .prepare("PRAGMA table_info(memory_records)")
            .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 探测失败"))?;
        statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 探测失败"))?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 探测失败"))
    }

    fn table_index_names(&self, table: &str) -> AxonResult<Vec<String>> {
        let mut statement = self
            .connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1")
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引探测失败"))?;
        statement
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引探测失败"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 索引探测失败"))
    }

    /// 列出升级时归档的旧记录表，用于向运维与界面显式提示。
    pub fn legacy_record_tables(&self) -> AxonResult<Vec<String>> {
        let mut statement = self
            .connection
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

    /// 写入或替换治理记录。
    pub fn upsert_record(&self, record: &MemorySqliteRecord) -> AxonResult<()> {
        crate::store_write::upsert_record_on_connection(&self.connection, record)
    }

    /// 原子更新一组既有记录的治理状态与排序元数据。
    pub fn update_governance_records(&self, records: &[MemorySqliteRecord]) -> AxonResult<()> {
        if records.is_empty() {
            return Ok(());
        }
        for record in records {
            validate_record(record)?;
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 治理事务启动失败"))?;
        for record in records {
            transaction
                .execute(
                    r"
                    UPDATE memory_records SET
                        state = ?2,
                        content_ref = ?3,
                        tombstone_blocked_ref = ?4,
                        tombstone_reason = ?5,
                        updated_at_ms = ?6,
                        last_active_at_ms = ?7,
                        confidence_basis_points = ?8,
                        importance_basis_points = ?9,
                        archived_at_ms = ?10
                    WHERE memory_id = ?1
                    ",
                    params![
                        i64_from_u64(record.record.id.0)?,
                        state_code(record.record.state),
                        record.record.content_ref.as_deref(),
                        record
                            .record
                            .tombstone
                            .as_ref()
                            .map(|item| item.blocked_ref.as_str()),
                        record
                            .record
                            .tombstone
                            .as_ref()
                            .map(|item| item.reason.as_str()),
                        i64_from_u64(record.updated_at_ms)?,
                        i64_from_u64(record.last_active_at_ms)?,
                        i64::from(record.confidence_basis_points),
                        i64::from(record.importance_basis_points),
                        optional_i64_from_u64(record.archived_at_ms)?,
                    ],
                )
                .map_err(|_| AxonError::memory_persistence("Memory SQLite 治理记录更新失败"))?;
        }
        transaction
            .commit()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 治理事务提交失败"))
    }

    /// 原子删除一组过期 Memory 记录。
    pub fn delete_records(&self, memory_ids: &[MemoryId]) -> AxonResult<usize> {
        let values = unique_positive_i64s(memory_ids.iter().map(|id| id.0))?;
        if values.is_empty() {
            return Ok(0);
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 删除事务启动失败"))?;
        let mut deleted = 0usize;
        for memory_id in values {
            deleted = deleted.saturating_add(
                transaction
                    .execute(
                        "DELETE FROM memory_records WHERE memory_id = ?1",
                        [memory_id],
                    )
                    .map_err(|_| AxonError::memory_persistence("Memory SQLite 记录删除失败"))?,
            );
        }
        transaction
            .commit()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 删除事务提交失败"))?;
        Ok(deleted)
    }

    /// 按 ID 读取治理记录。
    pub fn get_record(&self, memory_id: MemoryId) -> AxonResult<Option<MemorySqliteRecord>> {
        self.connection
            .query_row(
                "SELECT * FROM memory_records WHERE memory_id = ?1",
                [i64_from_u64(memory_id.0)?],
                row_to_record,
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 记录读取失败"))
    }

    /// 按向量 ID 读取治理记录。
    pub fn get_record_by_vector_id(
        &self,
        vector_id: i64,
    ) -> AxonResult<Option<MemorySqliteRecord>> {
        if vector_id <= 0 {
            return Err(AxonError::memory_persistence(
                "Memory SQLite vector_id 无效",
            ));
        }
        self.connection
            .query_row(
                "SELECT * FROM memory_records WHERE vector_id = ?1",
                [vector_id],
                row_to_record,
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 向量记录读取失败"))
    }

    /// 按 Memory ID 批量读取治理记录。
    pub fn records_by_memory_ids(
        &self,
        memory_ids: &[MemoryId],
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        let values = unique_positive_i64s(memory_ids.iter().map(|id| id.0))?;
        self.records_by_field(FIELD_MEMORY_ID, &values, "Memory SQLite 批量记录读取失败")
    }

    /// 按向量 ID 批量读取治理记录。
    pub fn records_by_vector_ids(&self, vector_ids: &[i64]) -> AxonResult<Vec<MemorySqliteRecord>> {
        let values = unique_positive_i64s_from_i64(vector_ids.iter().copied());
        self.records_by_field(
            FIELD_VECTOR_ID,
            &values,
            "Memory SQLite 批量向量记录读取失败",
        )
    }

    /// 按 Personal scope 读取可发布的稳定个人上下文，并按摘要内容确定性排序。
    pub fn personal_context_records_by_scope(
        &self,
        user_ref: &str,
        namespace_ref: &str,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if user_ref.trim().is_empty() || namespace_ref.trim().is_empty() || limit == 0 {
            return Err(AxonError::memory_persistence(
                "Personal Memory 快照查询参数无效",
            ));
        }
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE user_ref = ?1 AND namespace_ref = ?2 AND source_origin IN (?3, ?4) AND state IN (?5, ?6) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?7) AND memory_summary IS NOT NULL AND TRIM(memory_summary) <> '' ORDER BY TRIM(memory_summary) ASC, memory_id ASC LIMIT ?8",
            params![
                user_ref,
                namespace_ref,
                origin_code(MemorySourceOrigin::AgentUserContext),
                origin_code(MemorySourceOrigin::ExplicitUserPreference),
                state_code(MemoryState::Weak),
                state_code(MemoryState::Strong),
                i64::try_from(now_ms).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?,
                i64_from_usize(limit)?,
            ],
        )
    }

    /// 按 Personal scope 有界读取可召回记录，避免依赖工作目录或全表扫描。
    pub fn recallable_records_by_scope(
        &self,
        user_ref: &str,
        namespace_ref: &str,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if user_ref.trim().is_empty() || namespace_ref.trim().is_empty() || limit == 0 {
            return Err(AxonError::memory_persistence("Memory scope 查询参数无效"));
        }
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE user_ref = ?1 AND namespace_ref = ?2 AND state IN (?3, ?4) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?5) AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY importance_basis_points DESC, updated_at_ms DESC, memory_id ASC LIMIT ?6",
            params![
                user_ref,
                namespace_ref,
                state_code(MemoryState::Weak),
                state_code(MemoryState::Strong),
                i64::try_from(now_ms).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?,
                i64_from_usize(limit)?,
            ],
        )
    }

    /// 按 Workspace namespace 读取可召回记录；Workspace 不绑定 user_ref。
    pub fn recallable_records_by_namespace(
        &self,
        namespace_ref: &str,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        if namespace_ref.trim().is_empty() || limit == 0 {
            return Err(AxonError::memory_persistence(
                "Memory namespace 查询参数无效",
            ));
        }
        self.query_limited_records(
            "SELECT * FROM memory_records WHERE user_ref IS NULL AND namespace_ref = ?1 AND state IN (?2, ?3) AND archived_at_ms IS NULL AND (ttl_expires_at_ms IS NULL OR ttl_expires_at_ms > ?4) AND content_ref IS NOT NULL AND TRIM(content_ref) <> '' ORDER BY importance_basis_points DESC, updated_at_ms DESC, memory_id ASC LIMIT ?5",
            params![
                namespace_ref,
                state_code(MemoryState::Weak),
                state_code(MemoryState::Strong),
                i64::try_from(now_ms).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))?,
                i64_from_usize(limit)?,
            ],
        )
    }

    /// 按更新时间倒序读取治理记录。
    pub fn records(&self) -> AxonResult<Vec<MemorySqliteRecord>> {
        let mut stmt = self
            .connection
            .prepare(
                r"
                SELECT * FROM memory_records
                ORDER BY updated_at_ms DESC, memory_id ASC
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 记录列表查询失败"))?;
        stmt.query_map([], row_to_record)
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 记录列表读取失败"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AxonError::memory_persistence("Memory SQLite 记录列表解析失败"))
    }

    /// 读取 SQLite schema 版本。
    pub fn schema_version(&self) -> AxonResult<i64> {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|_| AxonError::memory_persistence("Memory SQLite schema 版本读取失败"))
    }

    fn records_by_field(
        &self,
        field: &'static str,
        values: &[i64],
        error_message: &'static str,
    ) -> AxonResult<Vec<MemorySqliteRecord>> {
        let mut records = Vec::new();
        for batch in values.chunks(MEMORY_SQLITE_QUERY_BATCH_SIZE) {
            let placeholders = std::iter::repeat_n("?", batch.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT * FROM memory_records WHERE {field} IN ({placeholders})");
            let mut stmt = self
                .connection
                .prepare(&sql)
                .map_err(|_| AxonError::memory_persistence(error_message))?;
            let batch_records = stmt
                .query_map(params_from_iter(batch.iter()), row_to_record)
                .map_err(|_| AxonError::memory_persistence(error_message))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| AxonError::memory_persistence(error_message))?;
            records.extend(batch_records);
        }
        Ok(records)
    }
}

/// 归档表名使用的时间戳，单位毫秒；仅用于命名，不参与业务判定。
fn archiving_now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

fn open_connection(path: &Path) -> AxonResult<Connection> {
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

#[cfg(test)]
#[path = "store_batch_tests.rs"]
mod batch_tests;

#[cfg(test)]
#[path = "store_visibility_tests.rs"]
mod visibility_tests;

#[cfg(test)]
#[path = "store_migration_tests.rs"]
mod migration_tests;

#[cfg(test)]
#[path = "store_test_fixtures.rs"]
mod test_fixtures;
