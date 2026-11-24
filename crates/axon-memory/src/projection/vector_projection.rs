//! Memory embedding 向量投影权威 store。

use std::{path::Path, time::Duration};

use axon_crypto::Sha256Hex;
use axon_domain::{AxonError, AxonResult, MemoryId};
use rusqlite::{Connection, OptionalExtension as _, params, params_from_iter};

use crate::MemoryConfig;

/// 向量投影 schema 版本。
pub const MEMORY_VECTOR_PROJECTION_SCHEMA_VERSION: i64 = 2;
/// 向量投影在共享 schema 版本表中的组件名。
const MEMORY_VECTOR_PROJECTION_COMPONENT: &str = "memory_vector_projections";
const MEMORY_VECTOR_PROJECTION_QUERY_BATCH_SIZE: usize = 256;
const MEMORY_VECTOR_PROJECTION_BUSY_TIMEOUT_MS: u64 = 5_000;

/// 可从 SQLite 权威重建物理向量索引的 embedding 投影。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryVectorProjection {
    /// Memory ID。
    pub memory_id: MemoryId,
    /// 物理向量索引 ID。
    pub vector_id: i64,
    /// embedding 模型引用。
    pub model_ref: String,
    /// 向量维度。
    pub dimension: usize,
    /// embedding 内容 hash。
    pub embedding_hash: String,
    /// embedding 向量。
    pub embedding: Vec<f32>,
    /// 更新时间。
    pub updated_at_ms: u64,
}

/// Memory 向量投影 store。
pub struct MemoryVectorProjectionStore {
    connection: Connection,
    dimension: usize,
}

impl MemoryVectorProjectionStore {
    /// 打开向量投影 store。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        open_connection(&config.db_path).map(|connection| Self {
            connection,
            dimension: config.dimension,
        })
    }

    /// 初始化向量投影 schema。
    pub fn initialize_schema(&self) -> AxonResult<()> {
        self.connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS memory_vector_projections (
                    memory_id INTEGER PRIMARY KEY NOT NULL,
                    vector_id INTEGER UNIQUE NOT NULL,
                    model_ref TEXT NOT NULL,
                    dimension INTEGER NOT NULL,
                    embedding_hash TEXT NOT NULL,
                    embedding_blob BLOB NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memory_vector_projection_vector_id
                    ON memory_vector_projections(vector_id);
                CREATE TABLE IF NOT EXISTS memory_schema_versions (
                    component TEXT PRIMARY KEY NOT NULL,
                    version INTEGER NOT NULL
                );
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影 schema 初始化失败"))?;
        // 同一个 memory.db 由多个 store 共用：`PRAGMA user_version` 归 memory_records
        // schema 所有，各组件版本写在自己的表里，避免互相覆盖（曾导致 v4 库被读成 2）。
        self.connection
            .execute(
                r"
                INSERT INTO memory_schema_versions(component, version)
                VALUES (?1, ?2)
                ON CONFLICT(component) DO UPDATE SET version = excluded.version
                ",
                rusqlite::params![
                    MEMORY_VECTOR_PROJECTION_COMPONENT,
                    MEMORY_VECTOR_PROJECTION_SCHEMA_VERSION
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影 schema 版本写入失败"))?;
        Ok(())
    }

    /// 读取向量投影自身的 schema 版本。
    pub fn schema_version(&self) -> AxonResult<Option<i64>> {
        self.connection
            .query_row(
                "SELECT version FROM memory_schema_versions WHERE component = ?1",
                [MEMORY_VECTOR_PROJECTION_COMPONENT],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影 schema 版本读取失败"))
    }

    /// 写入或替换向量投影。
    pub fn upsert_projection(&self, projection: &MemoryVectorProjection) -> AxonResult<()> {
        validate_projection(projection, self.dimension)?;
        self.initialize_schema()?;
        self.connection
            .execute(
                r"
                INSERT INTO memory_vector_projections (
                    memory_id, vector_id, model_ref, dimension, embedding_hash,
                    embedding_blob, updated_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(memory_id) DO UPDATE SET
                    vector_id = excluded.vector_id,
                    model_ref = excluded.model_ref,
                    dimension = excluded.dimension,
                    embedding_hash = excluded.embedding_hash,
                    embedding_blob = excluded.embedding_blob,
                    updated_at_ms = excluded.updated_at_ms
                ",
                params![
                    i64_from_u64(projection.memory_id.0)?,
                    projection.vector_id,
                    projection.model_ref.trim(),
                    i64_from_usize(projection.dimension)?,
                    projection.embedding_hash.trim(),
                    embedding_to_blob(&projection.embedding),
                    i64_from_u64(projection.updated_at_ms)?,
                ],
            )
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影写入失败"))?;
        Ok(())
    }

    /// 按 Memory ID 批量删除 embedding 投影。
    pub fn delete_projections_by_memory_ids(&self, memory_ids: &[MemoryId]) -> AxonResult<usize> {
        self.initialize_schema()?;
        let values = memory_ids
            .iter()
            .map(|memory_id| i64_from_u64(memory_id.0))
            .collect::<AxonResult<Vec<_>>>()?;
        let values = unique_positive_i64s(values.into_iter());
        if values.is_empty() {
            return Ok(0);
        }
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影删除事务启动失败"))?;
        let mut deleted = 0usize;
        for batch in values.chunks(MEMORY_VECTOR_PROJECTION_QUERY_BATCH_SIZE) {
            let placeholders = std::iter::repeat_n("?", batch.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "DELETE FROM memory_vector_projections WHERE memory_id IN ({placeholders})"
            );
            deleted = deleted.saturating_add(
                transaction
                    .execute(&sql, params_from_iter(batch.iter()))
                    .map_err(|_| AxonError::memory_persistence("Memory 向量投影删除失败"))?,
            );
        }
        transaction
            .commit()
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影删除事务提交失败"))?;
        Ok(deleted)
    }

    /// 按 vector_id 批量读取投影。
    pub fn projections_by_vector_ids(
        &self,
        vector_ids: &[i64],
    ) -> AxonResult<Vec<MemoryVectorProjection>> {
        self.initialize_schema()?;
        let values = unique_positive_i64s(vector_ids.iter().copied());
        let mut projections = Vec::new();
        for batch in values.chunks(MEMORY_VECTOR_PROJECTION_QUERY_BATCH_SIZE) {
            let placeholders = std::iter::repeat_n("?", batch.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT * FROM memory_vector_projections WHERE vector_id IN ({placeholders})"
            );
            let mut stmt = self
                .connection
                .prepare(&sql)
                .map_err(|_| AxonError::memory_persistence("Memory 向量投影批量查询失败"))?;
            let batch_projections = stmt
                .query_map(params_from_iter(batch.iter()), |row| {
                    row_to_projection(row, self.dimension)
                })
                .map_err(|_| AxonError::memory_persistence("Memory 向量投影批量读取失败"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| AxonError::memory_persistence("Memory 向量投影批量解析失败"))?;
            projections.extend(batch_projections);
        }
        Ok(projections)
    }

    /// 按 vector_id 读取单条投影。
    pub fn projection_by_vector_id(
        &self,
        vector_id: i64,
    ) -> AxonResult<Option<MemoryVectorProjection>> {
        if vector_id <= 0 {
            return Err(AxonError::memory_persistence(
                "Memory 向量投影 vector_id 无效",
            ));
        }
        self.initialize_schema()?;
        self.connection
            .query_row(
                "SELECT * FROM memory_vector_projections WHERE vector_id = ?1",
                [vector_id],
                |row| row_to_projection(row, self.dimension),
            )
            .optional()
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影读取失败"))
    }

    /// 读取最近更新的一批投影，用于物理向量索引聚合 flush。
    pub fn recent_projections(&self, limit: usize) -> AxonResult<Vec<MemoryVectorProjection>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize_schema()?;
        let mut stmt = self
            .connection
            .prepare(
                r"
                SELECT * FROM memory_vector_projections
                ORDER BY updated_at_ms DESC, vector_id DESC
                LIMIT ?1
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 向量投影最近查询失败"))?;
        stmt.query_map([i64_from_usize(limit)?], |row| {
            row_to_projection(row, self.dimension)
        })
        .map_err(|_| AxonError::memory_persistence("Memory 向量投影最近读取失败"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AxonError::memory_persistence("Memory 向量投影最近解析失败"))
    }

    /// 读取权威表中已不存在的孤儿投影，按 memory_id 升序有界返回。
    ///
    /// 投影与治理记录同库同文件，一次反连接即可精确判定；这类残留只会由「绕过
    /// 删除链路」的写入产生（例如历史版本直接重建 `memory_records`）。
    pub fn orphan_projections(&self, limit: usize) -> AxonResult<Vec<MemoryVectorProjection>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize_schema()?;
        let mut stmt = self
            .connection
            .prepare(
                r"
                SELECT p.* FROM memory_vector_projections p
                LEFT JOIN memory_records r ON r.memory_id = p.memory_id
                WHERE r.memory_id IS NULL
                ORDER BY p.memory_id ASC
                LIMIT ?1
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 孤儿投影查询失败"))?;
        stmt.query_map([i64_from_usize(limit)?], |row| {
            row_to_projection(row, self.dimension)
        })
        .map_err(|_| AxonError::memory_persistence("Memory 孤儿投影读取失败"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AxonError::memory_persistence("Memory 孤儿投影解析失败"))
    }

    /// 读取全部投影，用于把物理向量索引整体重建成与权威表一致的形状。
    pub fn all_projections(&self, limit: usize) -> AxonResult<Vec<MemoryVectorProjection>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.initialize_schema()?;
        let mut stmt = self
            .connection
            .prepare(
                r"
                SELECT * FROM memory_vector_projections
                ORDER BY memory_id ASC
                LIMIT ?1
                ",
            )
            .map_err(|_| AxonError::memory_persistence("Memory 全量投影查询失败"))?;
        stmt.query_map([i64_from_usize(limit)?], |row| {
            row_to_projection(row, self.dimension)
        })
        .map_err(|_| AxonError::memory_persistence("Memory 全量投影读取失败"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AxonError::memory_persistence("Memory 全量投影解析失败"))
    }
}

/// 计算 embedding 投影 hash。
#[must_use]
pub fn memory_embedding_hash(embedding: &[f32]) -> String {
    let mut hasher = Sha256Hex::new();
    for value in embedding {
        hasher.update(value.to_le_bytes());
    }
    hasher.finalize_tagged_hex()
}

fn open_connection(path: &Path) -> AxonResult<Connection> {
    let connection = Connection::open(path)
        .map_err(|_| AxonError::memory_persistence("无法打开 Memory 向量投影数据库"))?;
    connection
        .busy_timeout(Duration::from_millis(
            MEMORY_VECTOR_PROJECTION_BUSY_TIMEOUT_MS,
        ))
        .map_err(|_| AxonError::memory_persistence("Memory 向量投影 busy timeout 配置失败"))?;
    Ok(connection)
}

fn validate_projection(projection: &MemoryVectorProjection, dimension: usize) -> AxonResult<()> {
    if projection.memory_id.0 == 0
        || projection.vector_id <= 0
        || projection.model_ref.trim().is_empty()
        || projection.updated_at_ms == 0
    {
        return Err(AxonError::memory_persistence("Memory 向量投影基础字段无效"));
    }
    if projection.dimension != dimension || projection.embedding.len() != dimension {
        return Err(AxonError::memory_persistence(
            "Memory 向量投影维度与配置不一致",
        ));
    }
    if projection.embedding.iter().any(|value| !value.is_finite()) {
        return Err(AxonError::memory_persistence("Memory 向量投影包含非有限值"));
    }
    if projection.embedding.iter().all(|value| *value == 0.0) {
        return Err(AxonError::memory_persistence(
            "Memory 向量投影不能写入全零向量",
        ));
    }
    if projection.embedding_hash != memory_embedding_hash(&projection.embedding) {
        return Err(AxonError::memory_persistence("Memory 向量投影 hash 无效"));
    }
    Ok(())
}

fn row_to_projection(
    row: &rusqlite::Row<'_>,
    expected_dimension: usize,
) -> rusqlite::Result<MemoryVectorProjection> {
    let embedding_blob = row.get::<_, Vec<u8>>("embedding_blob")?;
    let embedding = blob_to_embedding(&embedding_blob)?;
    let projection = MemoryVectorProjection {
        memory_id: MemoryId(u64_from_i64(row.get("memory_id")?)?),
        vector_id: row.get("vector_id")?,
        model_ref: row.get("model_ref")?,
        dimension: usize_from_i64(row.get("dimension")?)?,
        embedding_hash: row.get("embedding_hash")?,
        embedding,
        updated_at_ms: u64_from_i64(row.get("updated_at_ms")?)?,
    };
    validate_projection(&projection, expected_dimension)
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(projection)
}

fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(embedding.len().saturating_mul(4));
    for value in embedding {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob
}

fn blob_to_embedding(blob: &[u8]) -> rusqlite::Result<Vec<f32>> {
    if !blob.len().is_multiple_of(4) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let (chunks, _) = blob.as_chunks::<4>();
    Ok(chunks
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect())
}

fn unique_positive_i64s(values: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut values = values.filter(|value| *value > 0).collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    values
}

fn i64_from_u64(value: u64) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory 向量投影数值溢出"))
}

fn i64_from_usize(value: usize) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory 向量投影数值溢出"))
}

fn u64_from_i64(value: i64) -> rusqlite::Result<u64> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

#[cfg(test)]
#[path = "vector_projection_tests.rs"]
mod tests;

fn usize_from_i64(value: i64) -> rusqlite::Result<usize> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}
