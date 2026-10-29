//! Axon Memory 的嵌入式配置拓扑与编排入口。

#[path = "workspace/artifacts.rs"]
mod artifacts;
#[path = "citation/citation.rs"]
mod citation;
#[path = "embedding/provider.rs"]
mod embedding_provider;
#[path = "store/file_lock.rs"]
mod file_lock;
#[path = "index/index_rebuild.rs"]
mod index_rebuild;
#[path = "pipeline/pipeline_state.rs"]
mod pipeline_state;
#[path = "usage/quality.rs"]
mod quality;
#[path = "retrieval/retrieval.rs"]
mod retrieval;
#[path = "store/store.rs"]
mod store;
#[path = "store/store_query.rs"]
mod store_query;
#[path = "store/store_supersession.rs"]
mod store_supersession;
#[path = "store/store_write.rs"]
mod store_write;
#[path = "index/text_index.rs"]
mod text_index;
#[path = "usage/usage.rs"]
mod usage;
#[path = "index/vector_index.rs"]
mod vector_index;
#[path = "projection/vector_projection.rs"]
mod vector_projection;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axon_domain::{AxonError, AxonResult};

pub use artifacts::{
    MEMORY_ARTIFACTS_EXTENSIONS_DIR, MEMORY_ARTIFACTS_PHASE2_DIFF_FILE,
    MEMORY_ARTIFACTS_RAW_MEMORIES_FILE, MEMORY_ARTIFACTS_ROLLOUT_SUMMARIES_DIR,
    MemoryArtifactsWorkspace, MemoryRolloutSummaryArtifact,
};
pub use citation::{
    MEMORY_CITATION_END_TAG, MEMORY_CITATION_START_TAG, MemoryCitation, MemoryCitationParseIssue,
    MemoryCitationParseReport, MemoryCitationParser, project_memory_citation_text,
};
pub use embedding_provider::{
    MEMORY_EMBEDDING_ARK_MULTIMODAL_PATH, MEMORY_EMBEDDING_DASHSCOPE_PATH,
    MEMORY_EMBEDDING_OPENAI_PATH, MEMORY_EMBEDDING_PROVIDER_DASHSCOPE,
    MEMORY_EMBEDDING_PROVIDER_OPENAI, MEMORY_EMBEDDING_PROVIDER_OPENAI_CHAT,
    MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL,
    MEMORY_EMBEDDING_REASON_RAW_RESPONSE_CLOSED, MEMORY_EMBEDDING_REASON_REMOTE_BODY_CLOSED,
    MEMORY_EMBEDDING_REASON_REQUEST_PLANNED, MemoryEmbeddingAuthHeaderPlan,
    MemoryEmbeddingCredentialPlan, MemoryEmbeddingHttpHeader, MemoryEmbeddingMultimodalContent,
    MemoryEmbeddingMultimodalRequestInput, MemoryEmbeddingProviderRuntime,
    MemoryEmbeddingRequestInput, MemoryEmbeddingRequestPlan, MemoryEmbeddingResponseVectors,
    multimodal_embedding_provider_supported,
};
pub use file_lock::{MEMORY_FILE_LOCK_TIMEOUT_MS, MemoryFileLock};
pub use index_rebuild::{MemoryIndexRebuildReport, MemoryIndexRebuildRuntime};
pub use pipeline_state::{
    MEMORY_PIPELINE_KIND_EXPLICIT_WRITE, MEMORY_PIPELINE_KIND_PHASE1, MEMORY_PIPELINE_KIND_PHASE2,
    MemoryPipelineJob, MemoryPipelineJobKind, MemoryPipelineJobStatus, MemoryPipelineStateStore,
};
pub use quality::{
    MemoryRuntimeQualityObservation, MemoryRuntimeQualityReport, MemoryRuntimeQualityStore,
};
pub use retrieval::{
    MEMORY_HYBRID_CANDIDATE_MULTIPLIER, MEMORY_HYBRID_GOVERNANCE_WEIGHT_BASIS_POINTS,
    MEMORY_HYBRID_TEXT_WEIGHT_BASIS_POINTS, MEMORY_HYBRID_VECTOR_WEIGHT_BASIS_POINTS,
    MemoryHybridExplain, MemoryHybridHit, MemoryHybridQuery, MemoryHybridScope,
};
pub use store::{
    LEGACY_MEMORY_RECORD_TABLE_PREFIX, MEMORY_SQLITE_BUSY_TIMEOUT_MS, MEMORY_SQLITE_SCHEMA_VERSION,
    MemoryArchiveStatus, MemoryKindCount, MemoryScopeFacet, MemorySqliteRecord,
    MemorySqliteRecordFilter, MemorySqliteStore, MemorySqliteSummary, MemoryTimelineCursor,
    MemoryVisibility,
};
pub use store_supersession::{MEMORY_SUPERSESSION_REASON, MemorySupersession};
pub use text_index::{
    MEMORY_TANTIVY_DEFAULT_LIMIT, MEMORY_TANTIVY_SCHEMA_VERSION, MEMORY_TANTIVY_WRITER_HEAP_BYTES,
    MemoryTextDocument, MemoryTextHit, MemoryTextIndex,
};
pub use usage::{
    MEMORY_USAGE_KIND_CITED, MEMORY_USAGE_KIND_INJECTED, MEMORY_USAGE_KIND_RECALLED,
    MEMORY_USAGE_KIND_STALE, MemoryUsageEvent, MemoryUsageKind, MemoryUsageStore,
};
pub use vector_index::{
    MEMORY_HNSW_EF_CONSTRUCTION, MEMORY_HNSW_MAX_LAYER, MEMORY_HNSW_MAX_NB_CONNECTION,
    MEMORY_HNSW_SCHEMA_VERSION, MEMORY_HNSW_SEARCH_EF, MemoryVectorDocument, MemoryVectorHit,
    MemoryVectorIndex,
};
pub use vector_projection::{
    MEMORY_VECTOR_PROJECTION_SCHEMA_VERSION, MemoryVectorProjection, MemoryVectorProjectionStore,
    memory_embedding_hash,
};

/// Personal Memory 的稳定 namespace 前缀。
pub const MEMORY_PERSONAL_NAMESPACE_PREFIX: &str = "personal";

/// Global Memory 的默认 namespace。
pub const MEMORY_GLOBAL_NAMESPACE_REF: &str = "memory-scope:local";

/// 基于 owner principal 构造 Personal Memory namespace。
#[must_use]
pub fn personal_memory_namespace(user_ref: &str) -> String {
    format!("{MEMORY_PERSONAL_NAMESPACE_PREFIX}:{user_ref}")
}

/// Axon 主目录（`~/.axon`）的 Workspace Memory namespace。
///
/// Home 目录树内不注册普通 Workspace，但「在 Axon 主目录里沉淀的经验、知识与流程」
/// 必须与个人画像分开存放：它们属于主目录这一固定地点，因此给一个与路径无关的稳定
/// namespace。Agent 写入、管理页写入与检索必须共用这一处取值，任何一侧偏移都会让
/// 记忆写进另一个桶。
pub const AXON_HOME_MEMORY_NAMESPACE: &str = "workspace:axon-home";

/// Memory 数据父目录名。
pub const MEMORY_DATA_PARENT_DIR: &str = "data";
/// Memory 数据目录名。
pub const MEMORY_DATA_DIR: &str = "memory";
/// SQLite 主库文件名。
pub const MEMORY_DB_FILE: &str = "memory.db";
/// Tantivy 倒排索引目录名。
pub const MEMORY_TANTIVY_DIR: &str = "tantivy";
/// HNSW 向量索引镜像文件名。
pub const MEMORY_HNSW_FILE: &str = "hnsw.idx";
/// 默认 embedding 维度；后续由 provider 配置覆盖。
pub const MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION: usize = 1_536;

/// Memory 检索运行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryRetrievalMode {
    /// 启用 embedding 时走文本候选 + 向量重排。
    Hybrid,
    /// 未启用 embedding 时走 Tantivy/文本降级路径。
    TextFallback,
    /// 未编译文本/向量索引时走 SQLite 有界召回。
    CoreFallback,
}

/// Memory 本机索引健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryIndexHealth {
    /// 文本索引是否可用。
    pub text_index_available: bool,
    /// 向量索引是否可用。
    pub vector_index_available: bool,
    /// 是否发生可观测降级。
    pub degraded: bool,
}

/// Memory 持久化物理路径配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConfig {
    /// 配置根目录。
    pub config_dir: PathBuf,
    /// Memory 数据根目录。
    pub memory_dir: PathBuf,
    /// SQLite 主库路径。
    pub db_path: PathBuf,
    /// Tantivy 索引目录。
    pub tantivy_index_dir: PathBuf,
    /// HNSW 镜像路径。
    pub hnsw_index_path: PathBuf,
    /// 是否启用 embedding。
    pub embedding_enabled: bool,
    /// embedding 维度。
    pub dimension: usize,
}

impl MemoryConfig {
    /// 从配置目录派生 Memory 数据路径。
    pub fn from_config_dir(
        config_dir: impl AsRef<Path>,
        embedding_enabled: bool,
        dimension: usize,
    ) -> AxonResult<Self> {
        if dimension == 0 {
            return Err(AxonError::invalid_input("Memory embedding 维度必须大于 0"));
        }
        let config_dir = config_dir.as_ref().to_path_buf();
        let memory_dir = config_dir
            .join(MEMORY_DATA_PARENT_DIR)
            .join(MEMORY_DATA_DIR);
        Ok(Self {
            db_path: memory_dir.join(MEMORY_DB_FILE),
            tantivy_index_dir: memory_dir.join(MEMORY_TANTIVY_DIR),
            hnsw_index_path: memory_dir.join(MEMORY_HNSW_FILE),
            memory_dir,
            config_dir,
            embedding_enabled,
            dimension,
        })
    }

    /// 当前检索运行模式。
    #[must_use]
    pub fn retrieval_mode(&self) -> MemoryRetrievalMode {
        if self.embedding_enabled && cfg!(feature = "memory-vector-index") {
            MemoryRetrievalMode::Hybrid
        } else if cfg!(feature = "memory-text-index") {
            MemoryRetrievalMode::TextFallback
        } else {
            MemoryRetrievalMode::CoreFallback
        }
    }
}

/// Memory 编排入口。
#[derive(Clone)]
pub struct MemoryOrchestrator {
    config: MemoryConfig,
    vector_index: Arc<Mutex<Option<SharedMemoryVectorIndex>>>,
}

impl std::fmt::Debug for MemoryOrchestrator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemoryOrchestrator")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl PartialEq for MemoryOrchestrator {
    fn eq(&self, other: &Self) -> bool {
        self.config == other.config
    }
}

impl Eq for MemoryOrchestrator {}

/// 进程内共享的 HNSW 索引句柄。
#[derive(Clone)]
pub struct SharedMemoryVectorIndex {
    inner: Arc<Mutex<MemoryVectorIndex>>,
}

impl SharedMemoryVectorIndex {
    /// 写入或替换单条向量。
    pub fn upsert_vector(&self, document: MemoryVectorDocument) -> AxonResult<()> {
        self.lock()?.upsert_vector(document)
    }

    /// 批量写入或替换向量。
    pub fn upsert_vectors(
        &self,
        documents: impl IntoIterator<Item = MemoryVectorDocument>,
    ) -> AxonResult<()> {
        self.lock()?.upsert_vectors(documents)
    }

    /// 用完整向量投影替换索引。
    pub fn replace_vectors(
        &self,
        documents: impl IntoIterator<Item = MemoryVectorDocument>,
    ) -> AxonResult<()> {
        self.lock()?.replace_vectors(documents)
    }

    /// 批量移除向量。
    pub fn remove_vectors(&self, vector_ids: &[i64]) -> AxonResult<()> {
        self.lock()?.remove_vectors(vector_ids)
    }

    /// 查询向量近邻。
    pub fn search(&self, embedding: &[f32], limit: usize) -> AxonResult<Vec<MemoryVectorHit>> {
        self.lock()?.search(embedding, limit)
    }

    /// 当前索引记录数。
    pub fn len(&self) -> AxonResult<usize> {
        Ok(self.lock()?.len())
    }

    /// 当前索引是否为空。
    pub fn is_empty(&self) -> AxonResult<bool> {
        Ok(self.lock()?.is_empty())
    }

    /// 当前索引是否包含指定 vector_id。
    pub fn contains_vector_id(&self, vector_id: i64) -> AxonResult<bool> {
        Ok(self.lock()?.contains_vector_id(vector_id))
    }

    fn lock(&self) -> AxonResult<std::sync::MutexGuard<'_, MemoryVectorIndex>> {
        self.inner
            .lock()
            .map_err(|_| AxonError::memory_persistence("Memory HNSW 共享索引锁已损坏"))
    }
}

impl MemoryOrchestrator {
    /// 初始化 Memory 数据目录和索引目录。
    pub fn init(config: MemoryConfig) -> AxonResult<Self> {
        fs::create_dir_all(&config.memory_dir)
            .map_err(|_| AxonError::memory_persistence("无法创建 Memory 数据目录"))?;
        let store = MemorySqliteStore::open(&config)?;
        store.initialize_schema()?;
        MemoryPipelineStateStore::open(&config)?.initialize_schema()?;
        MemoryVectorProjectionStore::open(&config)?.initialize_schema()?;
        MemoryUsageStore::open(&config)?.initialize_schema()?;
        MemoryRuntimeQualityStore::open(&config)?.initialize_schema()?;
        MemoryArtifactsWorkspace::new(&config).ensure_layout()?;
        if cfg!(feature = "memory-text-index") {
            let _ = MemoryTextIndex::open(&config);
        }
        let orchestrator = Self {
            config,
            vector_index: Arc::new(Mutex::new(None)),
        };
        if orchestrator.config.embedding_enabled && cfg!(feature = "memory-vector-index") {
            let _ = orchestrator.vector_index();
        }
        Ok(orchestrator)
    }

    /// 返回编排器配置。
    #[must_use]
    pub const fn config(&self) -> &MemoryConfig {
        &self.config
    }

    /// 当前检索运行模式。
    #[must_use]
    pub fn retrieval_mode(&self) -> MemoryRetrievalMode {
        self.config.retrieval_mode()
    }

    /// 探测本机索引健康状态，用于 CLI/TUI 显示降级。
    #[must_use]
    pub fn index_health(&self) -> MemoryIndexHealth {
        let text_expected = cfg!(feature = "memory-text-index");
        let vector_expected =
            self.config.embedding_enabled && cfg!(feature = "memory-vector-index");
        let text_index_available = !text_expected || self.text_index().is_ok();
        let vector_index_available =
            !vector_expected || self.vector_index().is_ok_and(|index| index.is_some());
        MemoryIndexHealth {
            text_index_available,
            vector_index_available,
            degraded: !text_index_available || !vector_index_available,
        }
    }

    /// 打开 SQLite 结构化 store。
    pub fn sqlite_store(&self) -> AxonResult<MemorySqliteStore> {
        MemorySqliteStore::open(&self.config)
    }

    /// 打开 Tantivy 文本索引。
    pub fn text_index(&self) -> AxonResult<MemoryTextIndex> {
        MemoryTextIndex::open(&self.config)
    }

    /// 打开 HNSW 向量索引；未启用 embedding 时返回 None。
    pub fn vector_index(&self) -> AxonResult<Option<SharedMemoryVectorIndex>> {
        if !self.config.embedding_enabled || !cfg!(feature = "memory-vector-index") {
            return Ok(None);
        }
        let mut cached = self
            .vector_index
            .lock()
            .map_err(|_| AxonError::memory_persistence("Memory HNSW 索引缓存锁已损坏"))?;
        if let Some(index) = cached.as_ref() {
            return Ok(Some(index.clone()));
        }
        let index = SharedMemoryVectorIndex {
            inner: Arc::new(Mutex::new(MemoryVectorIndex::open(&self.config)?)),
        };
        *cached = Some(index.clone());
        Ok(Some(index))
    }

    /// 打开 embedding 向量投影 store。
    pub fn vector_projection_store(&self) -> AxonResult<MemoryVectorProjectionStore> {
        MemoryVectorProjectionStore::open(&self.config)
    }
}
