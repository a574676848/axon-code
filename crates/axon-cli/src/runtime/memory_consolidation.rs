//! CLI 共享 Memory 巩固 Cron 执行入口。

use std::path::Path;

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig as AxonMemoryConfig,
    MemoryConsolidationInput, MemoryConsolidationRuntime, MemoryOrchestrator,
};
use axon_config::ConfigLoader;

use crate::runtime_paths::runtime_axon_root_path;

/// 执行一次由常驻 Cron 调度的 Memory 巩固。
pub(crate) fn run_memory_consolidation_once(config_path: &Path) -> Result<String, String> {
    let loaded = ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("Memory 巩固读取配置失败：{error:?}"))?;
    let axon_root = runtime_axon_root_path(config_path)?;
    let dimension = if loaded.config.memory.embedding.enabled {
        loaded.config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    };
    let memory_config = AxonMemoryConfig::from_config_dir(
        &axon_root,
        loaded.config.memory.embedding.enabled,
        dimension,
    )
    .map_err(|error| format!("Memory 巩固初始化配置失败：{error:?}"))?;
    let orchestrator = MemoryOrchestrator::init(memory_config)
        .map_err(|error| format!("Memory 巩固打开存储失败：{error:?}"))?;
    let report = MemoryConsolidationRuntime
        .consolidate(&MemoryConsolidationInput {
            orchestrator: &orchestrator,
            now_ms: axon_time::unix_now_ms(),
        })
        .map_err(|error| format!("Memory 巩固执行失败：{error:?}"))?;
    Ok(format!(
        "ttl_cleaned={} archived={} duplicates={} conflicts={} projection_pruned={}",
        report.ttl_cleaned,
        report.low_activity_archived,
        report.duplicate_tombstones,
        report.conflict_archived,
        report.projection_pruned_records
    ))
}
