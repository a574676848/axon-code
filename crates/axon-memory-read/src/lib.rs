//! Axon Memory 读路径。
//!
//! 本 crate 只负责 Memory root 解析、embedding 查询计划、hybrid recall、
//! citation 解析和 usage 遥测分类，不依赖 Memory 写入流水线。

#[path = "query/embedding.rs"]
mod embedding;
#[path = "query/turn.rs"]
mod turn;
#[path = "query/usage.rs"]
mod usage;

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use axon_config::AxonConfig;
use axon_domain::MemorySourceOrigin;
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryHybridHit, MemoryHybridQuery,
    MemoryOrchestrator, personal_memory_namespace,
};

pub use axon_memory::MEMORY_GLOBAL_NAMESPACE_REF;
pub use axon_memory::MemoryHybridScope;

pub use embedding::{
    MEMORY_QUERY_EMBEDDING_DISABLED, MEMORY_QUERY_EMBEDDING_FALLBACK,
    MEMORY_QUERY_EMBEDDING_GENERATED, MemoryReadAuthHeaderPlan, MemoryReadEmbeddingRequestInput,
    MemoryReadEmbeddingRequestPlan, MemoryReadHttpHeader, QueryEmbeddingReport,
    parse_memory_embedding_response, plan_memory_embedding, prewarm_memory_embedding,
};
pub use turn::{
    MemoryTurnCitationReport, MemoryTurnContext, MemoryTurnHit, memory_turn_citation_report,
    memory_turn_cited_usage_events, parse_memory_citations,
};
pub use usage::{
    cited_usage_events, injected_usage_events, recalled_usage_events, record_read_usage,
    record_read_usage_async,
};

/// 已准备的 Memory 查询。
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedMemoryQuery {
    /// Hybrid 命中。
    pub hits: Vec<MemoryHybridHit>,
    /// embedding 状态。
    pub embedding_status: &'static str,
    /// 降级原因。
    pub reason: Option<String>,
}

/// 写入记忆时复用的 embedding 结果。
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedMemoryEmbedding {
    /// embedding 向量。
    pub embedding: Option<Vec<f32>>,
    /// embedding 模型引用。
    pub model_ref: Option<String>,
    /// 生成状态。
    pub status: &'static str,
    /// 降级原因。
    pub reason: Option<String>,
}

/// Personal Memory 快照最多承载的 canonical 记录数。
pub const PERSONAL_MEMORY_SNAPSHOT_MAX_RECORDS: usize = 64;

/// Coding Agent 会话使用的不可变 Personal Memory 快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalMemorySnapshot {
    /// 稳定快照 ID，由 owner 与内容 revision 派生。
    pub id: String,
    /// 同一 owner Personal scope 的内容修订。
    pub revision: u64,
    /// 注入模型的稳定个人信息。
    pub context: Vec<String>,
}

/// 按 Personal scope 装配与当前 prompt 无关的确定性快照。
pub fn load_personal_memory_snapshot(
    config_path: &Path,
    config: &AxonConfig,
    user_ref: &str,
    now_ms: u64,
) -> Result<PersonalMemorySnapshot, String> {
    let orchestrator = init_memory_orchestrator(config_path, config)?;
    let store = orchestrator
        .sqlite_store()
        .map_err(|error| format!("{error:?}"))?;
    let records = store
        .personal_context_records_by_scope(
            user_ref,
            &personal_memory_namespace(user_ref),
            now_ms,
            PERSONAL_MEMORY_SNAPSHOT_MAX_RECORDS,
        )
        .map_err(|error| format!("{error:?}"))?;
    let context = records
        .iter()
        .filter_map(|record| record.record.source_trace.as_ref())
        .filter(|source| {
            matches!(
                source.origin,
                MemorySourceOrigin::AgentUserContext | MemorySourceOrigin::ExplicitUserPreference
            )
        })
        .map(|source| source.summary.trim())
        .filter(|summary| !summary.is_empty())
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .take(PERSONAL_MEMORY_SNAPSHOT_MAX_RECORDS)
        .collect::<Vec<_>>();
    let revision = snapshot_revision(&context);
    Ok(PersonalMemorySnapshot {
        id: format!("personal-memory-{user_ref}-{revision}"),
        revision,
        context,
    })
}

fn snapshot_revision(context: &[String]) -> u64 {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    for summary in context {
        digest.update(summary.len().to_le_bytes());
        digest.update(summary.as_bytes());
    }
    let bytes = digest.finalize();
    u64::from_le_bytes(bytes[..8].try_into().expect("digest 长度固定")) | 1
}

/// 返回 Axon Memory root。
#[must_use]
pub fn memory_root(axon_root: impl AsRef<Path>) -> PathBuf {
    axon_root.as_ref().join("data").join("memory")
}

/// Memory embedding 维度。
#[must_use]
pub fn memory_embedding_dimension(config: &AxonConfig) -> usize {
    if config.memory.embedding.enabled {
        config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    }
}

/// 从配置路径初始化 Memory orchestrator。
pub fn init_memory_orchestrator(
    config_path: &Path,
    config: &AxonConfig,
) -> Result<MemoryOrchestrator, String> {
    let axon_root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let memory_config = axon_memory::MemoryConfig::from_config_dir(
        axon_root,
        config.memory.embedding.enabled,
        memory_embedding_dimension(config),
    )
    .map_err(|_| "memory config invalid".to_owned())?;
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, MemoryOrchestrator>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| "memory orchestrator cache unavailable".to_owned())?;
    if let Some(orchestrator) = cache.get(&memory_config.config_dir)
        && orchestrator.config() == &memory_config
    {
        return Ok(orchestrator.clone());
    }
    let orchestrator = MemoryOrchestrator::init(memory_config.clone())
        .map_err(|_| "memory orchestrator unavailable".to_owned())?;
    cache.insert(memory_config.config_dir, orchestrator.clone());
    Ok(orchestrator)
}

/// 执行 Memory hybrid recall。
#[must_use]
pub fn hybrid_query_memory(
    config: &AxonConfig,
    orchestrator: &MemoryOrchestrator,
    prompt: &str,
    limit: usize,
    now_ms: u64,
    timeout: Duration,
    scope: Option<MemoryHybridScope>,
) -> PreparedMemoryQuery {
    let embedding = memory_query_embedding(config, prompt, timeout);
    hybrid_query_memory_with_embedding(orchestrator, prompt, &embedding, limit, now_ms, scope)
}

/// 为 Memory 查询生成统一的 embedding 报告。
#[must_use]
pub fn memory_query_embedding(
    config: &AxonConfig,
    prompt: &str,
    timeout: Duration,
) -> QueryEmbeddingReport {
    embedding::generate_memory_query_embedding(config, prompt, timeout)
}

/// 执行带预计算 embedding 的 Memory hybrid recall，避免跨 namespace 重复生成 embedding。
#[must_use]
pub fn hybrid_query_memory_with_embedding(
    orchestrator: &MemoryOrchestrator,
    prompt: &str,
    embedding: &QueryEmbeddingReport,
    limit: usize,
    now_ms: u64,
    scope: Option<MemoryHybridScope>,
) -> PreparedMemoryQuery {
    let hits = orchestrator
        .hybrid_search(&MemoryHybridQuery {
            query_text: prompt.to_owned(),
            query_embedding: embedding.embedding.clone(),
            limit,
            now_ms,
            scope,
        })
        .unwrap_or_default();
    PreparedMemoryQuery {
        hits,
        embedding_status: embedding.status,
        reason: embedding.reason.clone(),
    }
}

/// 为 Memory 写入生成与查询一致的 embedding。
pub fn memory_write_embedding(
    config: &AxonConfig,
    prompt: &str,
    timeout: Duration,
) -> PreparedMemoryEmbedding {
    let report = embedding::generate_memory_query_embedding(config, prompt, timeout);
    let model_ref = report.embedding.as_ref().and_then(|_| {
        config
            .memory
            .embedding
            .enabled
            .then(|| config.memory.embedding.model.clone())
            .filter(|model| !model.trim().is_empty())
            .map(|model| format!("embedding:{model}"))
    });
    PreparedMemoryEmbedding {
        embedding: report.embedding,
        model_ref,
        status: report.status,
        reason: report.reason,
    }
}

/// 生成 Memory 查询摘要。
pub fn memory_query_summary(
    config_path: &Path,
    config: &AxonConfig,
    prompt: &str,
    limit: usize,
    now_ms: u64,
    timeout: Duration,
    scope: Option<MemoryHybridScope>,
) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("memory_search query 不能为空".to_owned());
    }
    let orchestrator = init_memory_orchestrator(config_path, config)?;
    let scoped = scope.is_some();
    let result = hybrid_query_memory(config, &orchestrator, prompt, limit, now_ms, timeout, scope);
    let hits = result.hits.iter().map(|hit| serde_json::json!({
        "memoryId": hit.record.record.id.0,
        "scoreBasisPoints": hit.score_basis_points,
        "textRank": hit.text_rank,
        "vectorRank": hit.vector_rank,
        "summary": hit.record.record.source_trace.as_ref().map(|source| source.summary.trim()),
        "contentRef": hit.record.record.content_ref,
    })).collect::<Vec<_>>();
    Ok(serde_json::json!({
        "scope": if scoped { "scoped" } else { "all" },
        "query": prompt,
        "hits": hits,
        "embeddingStatus": result.embedding_status,
        "embeddingReason": result.reason,
        "truncated": false,
    })
    .to_string())
}

#[cfg(test)]
mod personal_snapshot_tests {
    use super::snapshot_revision;

    #[test]
    fn revision_is_stable_for_identical_model_visible_content() {
        let context = vec!["默认称呼长官".to_owned(), "偏好中文".to_owned()];

        assert_eq!(snapshot_revision(&context), snapshot_revision(&context));
    }

    #[test]
    fn revision_changes_when_model_visible_content_changes() {
        let original = vec!["偏好中文".to_owned()];
        let changed = vec!["偏好简体中文".to_owned()];

        assert_ne!(snapshot_revision(&original), snapshot_revision(&changed));
    }
}
