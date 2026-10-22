//! Memory 向量检索索引 feature 边界。

#[cfg(feature = "memory-vector-index")]
#[path = "vector_index_hnsw.rs"]
mod implementation;

#[cfg(feature = "memory-vector-index")]
pub use implementation::*;

#[cfg(not(feature = "memory-vector-index"))]
mod implementation {
    use axon_domain::{AxonError, AxonResult};

    use crate::MemoryConfig;

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
        /// 向量距离。
        pub distance: f32,
    }

    /// 关闭 `memory-vector-index` feature 时的 no-op 向量索引。
    #[derive(Debug, Clone)]
    pub struct MemoryVectorIndex {
        dimension: usize,
    }

    impl MemoryVectorIndex {
        /// 创建 no-op 向量索引，不触碰 HNSW 镜像。
        pub fn open(config: &MemoryConfig) -> AxonResult<Self> {
            Ok(Self {
                dimension: config.dimension,
            })
        }

        /// no-op 写入单条向量，保留输入校验。
        pub fn upsert_vector(&mut self, document: MemoryVectorDocument) -> AxonResult<()> {
            self.upsert_vectors([document])
        }

        /// no-op 批量写入向量，保留输入校验。
        pub fn upsert_vectors(
            &mut self,
            documents: impl IntoIterator<Item = MemoryVectorDocument>,
        ) -> AxonResult<()> {
            for document in documents {
                validate_document(&document, self.dimension)?;
            }
            Ok(())
        }

        /// no-op 全量替换，保留输入校验。
        pub fn replace_vectors(
            &mut self,
            documents: impl IntoIterator<Item = MemoryVectorDocument>,
        ) -> AxonResult<()> {
            self.upsert_vectors(documents)
        }

        /// no-op 移除，保留 vector_id 校验。
        pub fn remove_vectors(&mut self, vector_ids: &[i64]) -> AxonResult<()> {
            if vector_ids.iter().any(|vector_id| *vector_id <= 0) {
                return Err(AxonError::memory_persistence(
                    "Memory 向量索引删除 vector_id 无效",
                ));
            }
            Ok(())
        }

        /// no-op 查询，调用方会回退到 SQLite 有界召回。
        pub fn search(&self, embedding: &[f32], _limit: usize) -> AxonResult<Vec<MemoryVectorHit>> {
            validate_embedding(embedding, self.dimension)?;
            Ok(Vec::new())
        }

        /// 当前镜像记录数；core-only 构建没有向量镜像。
        #[must_use]
        pub const fn len(&self) -> usize {
            0
        }

        /// 当前索引是否为空。
        #[must_use]
        pub const fn is_empty(&self) -> bool {
            true
        }

        /// 当前镜像是否包含指定 vector_id；core-only 构建恒为 false。
        #[must_use]
        pub const fn contains_vector_id(&self, _vector_id: i64) -> bool {
            false
        }
    }

    fn validate_document(document: &MemoryVectorDocument, dimension: usize) -> AxonResult<()> {
        if document.vector_id <= 0 {
            return Err(AxonError::memory_persistence(
                "Memory 向量索引 vector_id 无效",
            ));
        }
        validate_embedding(&document.embedding, dimension)
    }

    fn validate_embedding(embedding: &[f32], dimension: usize) -> AxonResult<()> {
        if embedding.len() != dimension {
            return Err(AxonError::memory_persistence("Memory 向量索引维度无效"));
        }
        if embedding.iter().any(|value| !value.is_finite()) {
            return Err(AxonError::memory_persistence("Memory 向量索引包含非有限值"));
        }
        if embedding.iter().all(|value| *value == 0.0) {
            return Err(AxonError::memory_persistence(
                "Memory 向量索引不能写入全零向量",
            ));
        }
        Ok(())
    }
}

#[cfg(not(feature = "memory-vector-index"))]
pub use implementation::*;
