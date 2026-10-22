//! Memory filesystem artifacts workspace。

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_domain::{AxonError, AxonResult};

use crate::MemoryConfig;

/// raw memories 文件。
pub const MEMORY_ARTIFACTS_RAW_MEMORIES_FILE: &str = "raw_memories.md";
/// rollout summaries 目录。
pub const MEMORY_ARTIFACTS_ROLLOUT_SUMMARIES_DIR: &str = "rollout_summaries";
/// extensions 目录。
pub const MEMORY_ARTIFACTS_EXTENSIONS_DIR: &str = "extensions";
/// Phase2 workspace diff 文件。
pub const MEMORY_ARTIFACTS_PHASE2_DIFF_FILE: &str = "phase2_workspace_diff.md";

/// Rollout summary artifact。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRolloutSummaryArtifact {
    /// 文件 stem。
    pub stem: String,
    /// Markdown 内容。
    pub markdown: String,
}

/// Memory artifacts workspace。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryArtifactsWorkspace {
    root: PathBuf,
}

impl MemoryArtifactsWorkspace {
    /// 从 Memory config 创建 workspace。
    #[must_use]
    pub fn new(config: &MemoryConfig) -> Self {
        Self {
            root: config.memory_dir.clone(),
        }
    }

    /// 返回 root。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// raw memories 路径。
    #[must_use]
    pub fn raw_memories_path(&self) -> PathBuf {
        self.root.join(MEMORY_ARTIFACTS_RAW_MEMORIES_FILE)
    }

    /// rollout summaries 路径。
    #[must_use]
    pub fn rollout_summaries_dir(&self) -> PathBuf {
        self.root.join(MEMORY_ARTIFACTS_ROLLOUT_SUMMARIES_DIR)
    }

    /// extensions 路径。
    #[must_use]
    pub fn extensions_dir(&self) -> PathBuf {
        self.root.join(MEMORY_ARTIFACTS_EXTENSIONS_DIR)
    }

    /// phase2 diff 路径。
    #[must_use]
    pub fn phase2_diff_path(&self) -> PathBuf {
        self.root.join(MEMORY_ARTIFACTS_PHASE2_DIFF_FILE)
    }

    /// 确保 workspace 目录存在。
    pub fn ensure_layout(&self) -> AxonResult<()> {
        fs::create_dir_all(&self.root)
            .and_then(|()| fs::create_dir_all(self.rollout_summaries_dir()))
            .and_then(|()| fs::create_dir_all(self.extensions_dir()))
            .map_err(|_| AxonError::memory_persistence("Memory artifacts 目录创建失败"))
    }

    /// 重写 raw memories。
    pub fn write_raw_memories(&self, markdown: &str) -> AxonResult<()> {
        self.ensure_layout()?;
        fs::write(self.raw_memories_path(), markdown)
            .map_err(|_| AxonError::memory_persistence("Memory raw memories 写入失败"))
    }

    /// 同步 rollout summaries。
    pub fn sync_rollout_summaries(
        &self,
        artifacts: &[MemoryRolloutSummaryArtifact],
    ) -> AxonResult<()> {
        self.ensure_layout()?;
        for artifact in artifacts {
            validate_artifact(artifact)?;
            fs::write(
                self.rollout_summaries_dir()
                    .join(format!("{}.md", artifact.stem)),
                &artifact.markdown,
            )
            .map_err(|_| AxonError::memory_persistence("Memory rollout summary 写入失败"))?;
        }
        Ok(())
    }
}

fn validate_artifact(artifact: &MemoryRolloutSummaryArtifact) -> AxonResult<()> {
    if artifact.stem.trim().is_empty()
        || artifact.stem.contains('/')
        || artifact.stem.contains('\\')
        || artifact.markdown.trim().is_empty()
    {
        return Err(AxonError::memory_persistence(
            "Memory rollout summary artifact 无效",
        ));
    }
    Ok(())
}
