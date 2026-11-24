//! SQLite 结构化 Memory store。

#[cfg(test)]
use std::collections::BTreeSet;

use axon_domain::{AxonError, AxonResult, MemoryId, MemoryKind, MemoryRecord};
#[cfg(test)]
use axon_domain::{CellId, MemoryCandidateId, SessionId};
use rusqlite::{Connection, OptionalExtension as _, params_from_iter};

use crate::MemoryConfig;

mod store_codec;
pub(crate) use store_codec::{
    bool_from_i64, i64_from_u64, i64_from_usize, optional_i64_from_u64, origin_code, row_to_record,
    sqlite_bool, state_code, u64_from_i64, unique_positive_i64s, unique_positive_i64s_from_i64,
    usize_from_i64, validate_record,
};

mod store_lifecycle;
mod store_schema;

/// SQLite schema 版本。
pub const MEMORY_SQLITE_SCHEMA_VERSION: i64 = 5;
/// SQLite busy timeout，单位毫秒。
pub const MEMORY_SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
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

/// 时间轴与类型计数的归档状态筛选。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryArchiveStatus {
    /// 仅未归档记录。
    Active,
    /// 仅已归档记录。
    Archived,
    /// 不筛选。
    All,
}

impl MemoryArchiveStatus {
    /// 返回白名单化的 SQL 谓词。
    pub(crate) const fn predicate(self) -> &'static str {
        match self {
            Self::Active => "archived_at_ms IS NULL",
            Self::Archived => "archived_at_ms IS NOT NULL",
            Self::All => "1 = 1",
        }
    }

    /// 对外状态代码。
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::All => "all",
        }
    }

    /// 解析请求传入的状态代码；未知值返回 `None`。
    #[must_use]
    pub fn from_code(value: &str) -> Option<Self> {
        match value.trim() {
            "active" => Some(Self::Active),
            "archived" => Some(Self::Archived),
            "all" => Some(Self::All),
            _ => None,
        }
    }
}

/// SQLite 结构化 Memory store。
pub struct MemorySqliteStore {
    pub(crate) connection: Connection,
}

impl MemorySqliteStore {
    /// 打开并配置 SQLite store。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        store_schema::open_connection(&config.db_path).map(|connection| Self { connection })
    }

    /// 初始化 schema 与 WAL。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        store_schema::initialize_schema(&self.connection)
    }

    /// 列出升级时归档的旧记录表，用于向运维与界面显式提示。
    pub fn legacy_record_tables(&self) -> AxonResult<Vec<String>> {
        store_schema::legacy_record_tables(&self.connection)
    }

    /// 写入或替换治理记录。
    pub fn upsert_record(&self, record: &MemorySqliteRecord) -> AxonResult<()> {
        crate::store_write::upsert_record_on_connection(&self.connection, record)
    }

    /// 原子更新一组既有记录的治理状态与排序元数据。
    pub fn update_governance_records(&self, records: &[MemorySqliteRecord]) -> AxonResult<()> {
        store_lifecycle::update_governance_records(&self.connection, records)
    }

    /// 原子删除一组过期 Memory 记录。
    pub fn delete_records(&self, memory_ids: &[MemoryId]) -> AxonResult<usize> {
        store_lifecycle::delete_records(&self.connection, memory_ids)
    }

    /// 原子归档一组 Memory：只写 `archived_at_ms`，不动内容与召回状态。
    ///
    /// 与淘汰不同：淘汰写 tombstone 并建立 supersession 链，归档只是让记录退出召回，
    /// 事后仍能按 archived 过滤回看，属可逆动作。
    pub fn archive_records(&self, memory_ids: &[MemoryId], now_ms: u64) -> AxonResult<usize> {
        store_lifecycle::archive_records(&self.connection, memory_ids, now_ms)
    }

    /// 撤销归档：只清 `archived_at_ms`，不动正文与状态机。
    ///
    /// 这是归档「可逆」的实现面：归档只是让记录退出召回，撤销后按原状态回到召回集合。
    /// 已被淘汰（tombstone）的记录不会因为撤销归档复活，这里同样按状态排除。
    pub fn unarchive_records(&self, memory_ids: &[MemoryId], now_ms: u64) -> AxonResult<usize> {
        store_lifecycle::unarchive_records(&self.connection, memory_ids, now_ms)
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

    /// 探测 `memory_records` 现有列集合；仅供 schema 迁移测试取证。
    #[cfg(test)]
    pub(crate) fn record_table_columns(&self) -> AxonResult<BTreeSet<String>> {
        store_schema::record_table_columns(&self.connection)
    }

    /// 探测指定表的索引名；仅供 schema 迁移测试取证。
    #[cfg(test)]
    pub(crate) fn table_index_names(&self, table: &str) -> AxonResult<Vec<String>> {
        store_schema::table_index_names(&self.connection, table)
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
