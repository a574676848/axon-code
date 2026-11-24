//! hnsw_rs 向量检索索引与本机镜像持久化。

use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use axon_domain::{AxonError, AxonResult};
use hnsw_rs::prelude::{DistCosine, Hnsw};

use crate::MemoryConfig;
use crate::MemoryFileLock;

/// HNSW 镜像 schema 版本。
pub const MEMORY_HNSW_SCHEMA_VERSION: u32 = 1;
/// HNSW 每层最大连接数。
pub const MEMORY_HNSW_MAX_NB_CONNECTION: usize = 24;
/// HNSW 构建搜索宽度。
pub const MEMORY_HNSW_EF_CONSTRUCTION: usize = 200;
/// HNSW 最大层数。
pub const MEMORY_HNSW_MAX_LAYER: usize = 16;
/// HNSW 查询搜索宽度。
pub const MEMORY_HNSW_SEARCH_EF: usize = 32;

const MEMORY_HNSW_MAGIC: &[u8; 8] = b"AXONHNSW";
const U32_BYTES: usize = 4;
const U64_BYTES: usize = 8;
const I64_BYTES: usize = 8;
const F32_BYTES: usize = 4;
const HNSW_HEADER_BYTES: usize = MEMORY_HNSW_MAGIC.len() + U32_BYTES + U64_BYTES + U64_BYTES;

/// Memory 向量写入文档。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryVectorDocument {
    /// SQLite 中关联的 vector_id。
    pub vector_id: i64,
    /// embedding 向量。
    pub embedding: Vec<f32>,
}

/// Memory 向量命中。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryVectorHit {
    /// SQLite 中关联的 vector_id。
    pub vector_id: i64,
    /// hnsw_rs 返回的距离，数值越小越相近。
    pub distance: f32,
}

/// hnsw_rs 向量索引封装。
pub struct MemoryVectorIndex {
    path: std::path::PathBuf,
    dimension: usize,
    records: Vec<MemoryVectorDocument>,
    hnsw: Hnsw<'static, f32, DistCosine>,
}

impl MemoryVectorIndex {
    /// 打开本机 HNSW 镜像；不存在时创建空索引。
    pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
        let records = load_records(&config.hnsw_index_path, config.dimension)?;
        let hnsw = build_hnsw(&records);
        Ok(Self {
            path: config.hnsw_index_path.clone(),
            dimension: config.dimension,
            records,
            hnsw,
        })
    }

    /// 写入或替换向量，并同步本机镜像。
    pub fn upsert_vector(&mut self, document: MemoryVectorDocument) -> AxonResult<()> {
        self.upsert_vectors([document])
    }

    /// 批量写入或替换向量，并只重建和持久化一次本机镜像。
    pub fn upsert_vectors(
        &mut self,
        documents: impl IntoIterator<Item = MemoryVectorDocument>,
    ) -> AxonResult<()> {
        let documents = documents.into_iter().collect::<Vec<_>>();
        if documents.is_empty() {
            return Ok(());
        }
        for document in &documents {
            validate_document(document, self.dimension)?;
        }
        let _lock = MemoryFileLock::acquire(&self.path)?;
        self.records = load_records(&self.path, self.dimension)?;
        for document in documents {
            if let Some(existing) = self
                .records
                .iter_mut()
                .find(|record| record.vector_id == document.vector_id)
            {
                *existing = document;
            } else {
                self.records.push(document);
            }
        }
        self.hnsw = build_hnsw(&self.records);
        persist_records(&self.path, self.dimension, &self.records)
    }

    /// 用完整向量投影替换本机镜像；用于一致性修复和批量恢复。
    pub fn replace_vectors(
        &mut self,
        documents: impl IntoIterator<Item = MemoryVectorDocument>,
    ) -> AxonResult<()> {
        let documents = documents.into_iter().collect::<Vec<_>>();
        for document in &documents {
            validate_document(document, self.dimension)?;
        }
        let _lock = MemoryFileLock::acquire(&self.path)?;
        self.records = documents;
        self.hnsw = build_hnsw(&self.records);
        persist_records(&self.path, self.dimension, &self.records)
    }

    /// 批量移除向量，并只重建和持久化一次本机镜像。
    pub fn remove_vectors(&mut self, vector_ids: &[i64]) -> AxonResult<()> {
        if vector_ids.is_empty() {
            return Ok(());
        }
        if vector_ids.iter().any(|vector_id| *vector_id <= 0) {
            return Err(AxonError::memory_persistence(
                "Memory HNSW 删除 vector_id 无效",
            ));
        }
        let _lock = MemoryFileLock::acquire(&self.path)?;
        self.records = load_records(&self.path, self.dimension)?;
        self.records
            .retain(|record| !vector_ids.contains(&record.vector_id));
        self.hnsw = build_hnsw(&self.records);
        persist_records(&self.path, self.dimension, &self.records)
    }

    /// 按 embedding 查询近邻。
    pub fn search(&self, embedding: &[f32], limit: usize) -> AxonResult<Vec<MemoryVectorHit>> {
        validate_embedding(embedding, self.dimension)?;
        if limit == 0 || self.records.is_empty() {
            return Ok(Vec::new());
        }
        let ef = MEMORY_HNSW_SEARCH_EF.max(limit);
        self.hnsw
            .search(embedding, limit, ef)
            .into_iter()
            .map(|neighbour| {
                Ok(MemoryVectorHit {
                    vector_id: i64::try_from(neighbour.d_id)
                        .map_err(|_| AxonError::memory_persistence("Memory HNSW vector_id 溢出"))?,
                    distance: neighbour.distance,
                })
            })
            .collect()
    }

    /// 当前镜像记录数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 当前索引是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 当前镜像是否包含指定 vector_id。
    #[must_use]
    pub fn contains_vector_id(&self, vector_id: i64) -> bool {
        self.records
            .iter()
            .any(|record| record.vector_id == vector_id)
    }
}

fn build_hnsw(records: &[MemoryVectorDocument]) -> Hnsw<'static, f32, DistCosine> {
    let max_elements = records.len().saturating_add(1).max(1);
    let hnsw = Hnsw::new(
        MEMORY_HNSW_MAX_NB_CONNECTION,
        max_elements,
        MEMORY_HNSW_MAX_LAYER,
        MEMORY_HNSW_EF_CONSTRUCTION,
        DistCosine {},
    );
    for record in records {
        if let Ok(vector_id) = usize::try_from(record.vector_id) {
            hnsw.insert((record.embedding.as_slice(), vector_id));
        }
    }
    hnsw
}

fn load_records(path: &Path, dimension: usize) -> AxonResult<Vec<MemoryVectorDocument>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|_| AxonError::memory_persistence("Memory HNSW 镜像读取失败"))?;
    parse_records(&bytes, dimension)
}

fn parse_records(bytes: &[u8], expected_dimension: usize) -> AxonResult<Vec<MemoryVectorDocument>> {
    let mut cursor = 0usize;
    let magic = read_exact(bytes, &mut cursor, MEMORY_HNSW_MAGIC.len())?;
    if magic != MEMORY_HNSW_MAGIC {
        return Err(AxonError::memory_persistence("Memory HNSW 镜像魔数无效"));
    }
    let version = read_u32(bytes, &mut cursor)?;
    if version != MEMORY_HNSW_SCHEMA_VERSION {
        return Err(AxonError::memory_persistence(
            "Memory HNSW schema 版本不兼容",
        ));
    }
    let dimension = usize_from_u64(read_u64(bytes, &mut cursor)?)?;
    if dimension != expected_dimension {
        return Err(AxonError::memory_persistence(
            "Memory HNSW 维度与配置不一致",
        ));
    }
    let count = usize_from_u64(read_u64(bytes, &mut cursor)?)?;
    validate_record_layout(bytes.len(), dimension, count)?;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let vector_id = read_i64(bytes, &mut cursor)?;
        let mut embedding = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            embedding.push(read_f32(bytes, &mut cursor)?);
        }
        let document = MemoryVectorDocument {
            vector_id,
            embedding,
        };
        validate_document(&document, dimension)?;
        records.push(document);
    }
    if cursor != bytes.len() {
        return Err(AxonError::memory_persistence(
            "Memory HNSW 镜像存在尾部脏数据",
        ));
    }
    Ok(records)
}

fn validate_record_layout(file_len: usize, dimension: usize, count: usize) -> AxonResult<()> {
    let vector_bytes = dimension
        .checked_mul(F32_BYTES)
        .and_then(|bytes| bytes.checked_add(I64_BYTES))
        .ok_or(AxonError::memory_persistence(
            "Memory HNSW 镜像记录长度溢出",
        ))?;
    let expected_len = vector_bytes
        .checked_mul(count)
        .and_then(|bytes| bytes.checked_add(HNSW_HEADER_BYTES))
        .ok_or(AxonError::memory_persistence(
            "Memory HNSW 镜像文件长度溢出",
        ))?;
    if expected_len != file_len {
        return Err(AxonError::memory_persistence(
            "Memory HNSW 镜像文件长度不匹配",
        ));
    }
    Ok(())
}

fn persist_records(
    path: &Path,
    dimension: usize,
    records: &[MemoryVectorDocument],
) -> AxonResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| AxonError::memory_persistence("无法创建 Memory HNSW 目录"))?;
    }
    let temp_path = path.with_extension("idx.tmp");
    let mut file = fs::File::create(&temp_path)
        .map_err(|_| AxonError::memory_persistence("Memory HNSW 临时镜像创建失败"))?;
    let dimension_u64 = u64_from_usize(dimension)?;
    let record_count = u64_from_usize(records.len())?;
    file.write_all(MEMORY_HNSW_MAGIC)
        .and_then(|()| file.write_all(&MEMORY_HNSW_SCHEMA_VERSION.to_le_bytes()))
        .and_then(|()| file.write_all(&dimension_u64.to_le_bytes()))
        .and_then(|()| file.write_all(&record_count.to_le_bytes()))
        .map_err(|_| AxonError::memory_persistence("Memory HNSW 镜像头写入失败"))?;
    for record in records {
        validate_document(record, dimension)?;
        file.write_all(&record.vector_id.to_le_bytes())
            .map_err(|_| AxonError::memory_persistence("Memory HNSW vector_id 写入失败"))?;
        for value in &record.embedding {
            file.write_all(&value.to_le_bytes())
                .map_err(|_| AxonError::memory_persistence("Memory HNSW 向量写入失败"))?;
        }
    }
    file.sync_all()
        .map_err(|_| AxonError::memory_persistence("Memory HNSW 镜像同步失败"))?;
    fs::rename(&temp_path, path)
        .map_err(|_| AxonError::memory_persistence("Memory HNSW 镜像替换失败"))
}

fn validate_document(document: &MemoryVectorDocument, dimension: usize) -> AxonResult<()> {
    if document.vector_id <= 0 {
        return Err(AxonError::memory_persistence("Memory HNSW vector_id 无效"));
    }
    validate_embedding(&document.embedding, dimension)
}

fn validate_embedding(embedding: &[f32], dimension: usize) -> AxonResult<()> {
    if embedding.len() != dimension {
        return Err(AxonError::memory_persistence("Memory HNSW 向量维度无效"));
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(AxonError::memory_persistence(
            "Memory HNSW 向量包含非有限值",
        ));
    }
    if embedding.iter().all(|value| *value == 0.0) {
        return Err(AxonError::memory_persistence("Memory HNSW 向量不能全为零"));
    }
    Ok(())
}

fn read_exact<'a>(bytes: &'a [u8], cursor: &mut usize, len: usize) -> AxonResult<&'a [u8]> {
    let end = cursor
        .checked_add(len)
        .ok_or(AxonError::memory_persistence("Memory HNSW 镜像游标溢出"))?;
    let slice = bytes
        .get(*cursor..end)
        .ok_or(AxonError::memory_persistence("Memory HNSW 镜像截断"))?;
    *cursor = end;
    Ok(slice)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> AxonResult<u32> {
    let mut value = [0u8; U32_BYTES];
    value.copy_from_slice(read_exact(bytes, cursor, U32_BYTES)?);
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> AxonResult<u64> {
    let mut value = [0u8; U64_BYTES];
    value.copy_from_slice(read_exact(bytes, cursor, U64_BYTES)?);
    Ok(u64::from_le_bytes(value))
}

fn read_i64(bytes: &[u8], cursor: &mut usize) -> AxonResult<i64> {
    let mut value = [0u8; I64_BYTES];
    value.copy_from_slice(read_exact(bytes, cursor, I64_BYTES)?);
    Ok(i64::from_le_bytes(value))
}

fn read_f32(bytes: &[u8], cursor: &mut usize) -> AxonResult<f32> {
    let mut value = [0u8; F32_BYTES];
    value.copy_from_slice(read_exact(bytes, cursor, F32_BYTES)?);
    Ok(f32::from_le_bytes(value))
}

fn usize_from_u64(value: u64) -> AxonResult<usize> {
    usize::try_from(value).map_err(|_| AxonError::memory_persistence("Memory HNSW 数值溢出"))
}

fn u64_from_usize(value: usize) -> AxonResult<u64> {
    u64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory HNSW 数值溢出"))
}
