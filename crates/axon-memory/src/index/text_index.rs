//! Memory 文本检索索引 feature 边界。

#[cfg(feature = "memory-text-index")]
#[path = "text_index_tantivy.rs"]
mod implementation;

#[cfg(feature = "memory-text-index")]
pub use implementation::*;

#[cfg(not(feature = "memory-text-index"))]
mod implementation {
    use axon_domain::{AxonError, AxonResult, MemoryId};

    use crate::MemoryConfig;

    /// Tantivy writer 默认内存预算。
    pub const MEMORY_TANTIVY_WRITER_HEAP_BYTES: usize = 50_000_000;
    /// Tantivy schema 版本。
    pub const MEMORY_TANTIVY_SCHEMA_VERSION: u32 = 2;
    /// Tantivy 默认召回数量。
    pub const MEMORY_TANTIVY_DEFAULT_LIMIT: usize = 16;

    /// 文本索引写入文档。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct MemoryTextDocument {
        /// Memory ID。
        pub memory_id: MemoryId,
        /// 内容引用。
        pub content_ref: Option<String>,
        /// 证据引用。
        pub evidence_ref: Option<String>,
        /// 墓碑引用。
        pub tombstone_ref: Option<String>,
        /// 可检索的脱敏记忆摘要。
        pub memory_summary: Option<String>,
    }

    /// 文本索引命中。
    #[derive(Debug, Clone, PartialEq)]
    pub struct MemoryTextHit {
        /// Memory ID。
        pub memory_id: MemoryId,
        /// 文本分数。
        pub score: f32,
    }

    /// 关闭 `memory-text-index` feature 时的 no-op 文本索引。
    #[derive(Debug, Default, Clone, Copy)]
    pub struct MemoryTextIndex;

    impl MemoryTextIndex {
        /// 创建 no-op 文本索引，不触碰 Tantivy 目录。
        pub fn open(_config: &MemoryConfig) -> AxonResult<Self> {
            Ok(Self)
        }

        /// no-op 写入单条文档，保留输入校验。
        pub fn upsert_document(&self, document: &MemoryTextDocument) -> AxonResult<()> {
            validate_document(document)
        }

        /// no-op 批量写入文档，保留输入校验。
        pub fn upsert_documents<'a>(
            &self,
            documents: impl IntoIterator<Item = &'a MemoryTextDocument>,
        ) -> AxonResult<()> {
            for document in documents {
                validate_document(document)?;
            }
            Ok(())
        }

        /// no-op 删除，保留 Memory ID 校验。
        pub fn delete_documents(&self, memory_ids: &[MemoryId]) -> AxonResult<()> {
            if memory_ids.iter().any(|memory_id| memory_id.0 == 0) {
                return Err(AxonError::memory_persistence("Memory 文本索引删除 ID 无效"));
            }
            Ok(())
        }

        /// no-op 查询，调用方会回退到 SQLite 有界召回。
        pub fn search(&self, _query_text: &str, _limit: usize) -> AxonResult<Vec<MemoryTextHit>> {
            Ok(Vec::new())
        }

        /// 未编译文本索引时不存在任何文档，反向核对结果恒为空。
        pub fn indexed_memory_ids(&self, _limit: usize) -> AxonResult<Vec<MemoryId>> {
            Ok(Vec::new())
        }
    }

    fn validate_document(document: &MemoryTextDocument) -> AxonResult<()> {
        if document.memory_id.0 == 0 {
            return Err(AxonError::memory_persistence(
                "Memory 文本索引 memory_id 无效",
            ));
        }
        if [
            document.content_ref.as_deref(),
            document.evidence_ref.as_deref(),
            document.tombstone_ref.as_deref(),
            document.memory_summary.as_deref(),
        ]
        .into_iter()
        .flatten()
        .all(|value| value.trim().is_empty())
        {
            return Err(AxonError::memory_persistence(
                "Memory 文本索引文档必须包含可检索引用",
            ));
        }
        Ok(())
    }
}

#[cfg(not(feature = "memory-text-index"))]
pub use implementation::*;
