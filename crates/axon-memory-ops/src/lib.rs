//! Memory 导入、导出与一致性修复执行面。
//!
//! CLI 只负责解析子命令并把输出转交给用户；本 crate 负责 ref-only
//! import/export、同步审计、签名加密和物理投影一致性修复。

use std::path::Path;

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig as AxonMemoryConfig, MemoryOrchestrator,
};
use axon_config::AxonConfig;
pub(crate) use axon_display::display_path;
use axon_time::unix_now_ms;
use serde::Serialize;

#[path = "operations/consistency.rs"]
mod consistency;
#[path = "operations/export.rs"]
mod export;
#[path = "operations/import.rs"]
mod import;
#[path = "operations/sync_audit.rs"]
mod sync_audit;
#[path = "operations/sync_encryption.rs"]
mod sync_encryption;
#[path = "operations/sync_signature.rs"]
mod sync_signature;
#[path = "operations/view.rs"]
mod view;

pub use consistency::{MemoryConsistencyCommand, run_memory_consistency};
pub use export::{MemoryExportCommand, run_memory_export};
pub use import::{MemoryImportCommand, run_memory_import};

const MEMORY_OPS_DEFAULT_LIMIT: usize = 16;
const MEMORY_EXPORT_SCHEMA_VERSION: u32 = 3;
const MEMORY_EXPORT_DIGEST_ALGORITHM: &str = "sha256:records-v2";

/// Memory ops 输出格式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MemoryOpsOutputFormat {
    /// 人类可读文本。
    #[default]
    Text,
    /// JSON 输出。
    Json,
}

/// Memory ops 执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryOpsResult {
    /// 进程退出码。
    pub exit_code: u8,
    /// stdout 内容。
    pub stdout: String,
    /// stderr 内容。
    pub stderr: String,
}

/// Memory ops 运行上下文。
pub struct MemoryOpsContext {
    pub(crate) orchestrator: MemoryOrchestrator,
}

/// 打开 Memory ops 上下文。
pub fn open_memory_ops_context(
    config_path: &Path,
    config: &AxonConfig,
) -> Result<MemoryOpsContext, String> {
    let axon_root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let memory_config = AxonMemoryConfig::from_config_dir(
        &axon_root,
        config.memory.embedding.enabled,
        memory_embedding_dimension(config),
    )
    .map_err(|error| format!("{error:?}"))?;
    let orchestrator =
        MemoryOrchestrator::init(memory_config).map_err(|error| format!("{error:?}"))?;
    Ok(MemoryOpsContext { orchestrator })
}

fn cli_error(message: impl Into<String>) -> MemoryOpsResult {
    MemoryOpsResult {
        exit_code: 2,
        stdout: String::new(),
        stderr: format!("error: {}\n", message.into()),
    }
}

fn json_result(value: &impl Serialize) -> MemoryOpsResult {
    match serde_json::to_string(value) {
        Ok(line) => MemoryOpsResult {
            exit_code: 0,
            stdout: format!("{line}\n"),
            stderr: String::new(),
        },
        Err(_) => cli_error("JSON 序列化失败"),
    }
}

fn bounded_limit(limit: Option<usize>) -> Result<usize, &'static str> {
    let limit = limit.unwrap_or(MEMORY_OPS_DEFAULT_LIMIT);
    if limit == 0 {
        return Err("memory limit 必须大于 0");
    }
    Ok(limit)
}

fn now_ms() -> u64 {
    unix_now_ms()
}

fn memory_embedding_dimension(config: &AxonConfig) -> usize {
    if config.memory.embedding.enabled {
        config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
