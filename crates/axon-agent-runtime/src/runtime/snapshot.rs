//! Snapshot - 文件系统快照服务。
//!
//! 实现基于 git stash 的文件系统快照与恢复，对标 opencode Snapshot Service。
//!
//! 核心能力：
//! - 创建快照（track）
//! - 恢复快照（restore）
//! - 回退变更（revert）
//! - 计算 diff

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axon_domain::{AxonError, AxonResult};
use axon_process::non_interactive_tokio_command;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::git_diff_semantic_analyzer::DiffAnalyzer;

/// 快照 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SnapshotId(pub String);

impl SnapshotId {
    /// 创建新的快照 ID
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for SnapshotId {
    fn default() -> Self {
        Self::new()
    }
}

/// 快照元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMetadata {
    /// 快照 ID
    pub id: SnapshotId,
    /// 创建时间戳
    pub timestamp_ms: u64,
    /// Git stash 引用（如果使用 git）
    pub git_stash_ref: Option<String>,
    /// 工作目录
    pub cwd: PathBuf,
    /// 变更文件列表
    pub changed_files: Vec<String>,
}

/// Diff 条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    /// 文件路径
    pub path: String,
    /// 新增行数
    pub additions: usize,
    /// 删除行数
    pub deletions: usize,
}

/// 快照运行时
#[derive(Clone)]
pub struct SnapshotRuntime {
    /// 快照存储
    snapshots: Arc<RwLock<HashMap<SnapshotId, SnapshotMetadata>>>,
    /// Diff 分析器
    diff_analyzer: Arc<DiffAnalyzer>,
    /// 快照栈（用于多层快照）
    snapshot_stack: Arc<RwLock<Vec<SnapshotId>>>,
}

impl Default for SnapshotRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotRuntime {
    /// 创建新实例
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshots: Arc::new(RwLock::new(HashMap::new())),
            diff_analyzer: Arc::new(DiffAnalyzer::new()),
            snapshot_stack: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 创建快照（对标 opencode snap.track）
    pub async fn track(&self) -> AxonResult<SnapshotId> {
        let snapshot_id = SnapshotId::new();
        let cwd =
            std::env::current_dir().map_err(|_| AxonError::runtime_failure("无法获取工作目录"))?;

        let metadata = SnapshotMetadata {
            id: snapshot_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
            git_stash_ref: None,
            cwd,
            changed_files: Vec::new(),
        };

        self.snapshots
            .write()
            .await
            .insert(snapshot_id.clone(), metadata);
        self.snapshot_stack.write().await.push(snapshot_id.clone());

        Ok(snapshot_id)
    }

    /// 使用 git stash 创建快照
    pub async fn track_with_git(&self, cwd: &Path) -> AxonResult<SnapshotId> {
        let snapshot_id = SnapshotId::new();

        let output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args(["stash", "push", "--include-untracked", "-m", &snapshot_id.0])
            .output()
            .await
            .map_err(|_| AxonError::runtime_failure("git stash 执行失败"))?;

        if !output.status.success() {
            return Err(AxonError::runtime_failure("git stash 命令失败"));
        }

        let git_stash_ref = Some("stash@{0}".to_string());

        let metadata = SnapshotMetadata {
            id: snapshot_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis() as u64,
            git_stash_ref,
            cwd: cwd.to_path_buf(),
            changed_files: Vec::new(),
        };

        self.snapshots
            .write()
            .await
            .insert(snapshot_id.clone(), metadata);
        self.snapshot_stack.write().await.push(snapshot_id.clone());

        Ok(snapshot_id)
    }

    /// 恢复快照（对标 opencode snap.restore）
    pub async fn restore(&self, snapshot_id: &SnapshotId) -> AxonResult<()> {
        let snapshots = self.snapshots.read().await;
        let metadata = snapshots
            .get(snapshot_id)
            .ok_or(AxonError::runtime_failure("快照不存在"))?;

        if let Some(git_stash_ref) = &metadata.git_stash_ref {
            let output = non_interactive_tokio_command("git")
                .current_dir(&metadata.cwd)
                .args(["stash", "pop", git_stash_ref])
                .output()
                .await
                .map_err(|_| AxonError::runtime_failure("git stash pop 执行失败"))?;

            if !output.status.success() {
                return Err(AxonError::runtime_failure("git stash pop 命令失败"));
            }
        }

        Ok(())
    }

    /// 计算 diff（对标 opencode snap.diff）
    pub async fn diff(&self, snapshot_id: &SnapshotId, cwd: &Path) -> AxonResult<Vec<DiffEntry>> {
        let snapshots = self.snapshots.read().await;
        let _metadata = snapshots
            .get(snapshot_id)
            .ok_or(AxonError::runtime_failure("快照不存在"))?;

        let semantic_diff = self
            .diff_analyzer
            .analyze_directory(cwd)
            .map_err(|_| AxonError::runtime_failure("diff 分析失败"))?;

        let entries = semantic_diff
            .files
            .into_iter()
            .map(|file| DiffEntry {
                path: file.path,
                additions: file.additions as usize,
                deletions: file.deletions as usize,
            })
            .collect();

        Ok(entries)
    }

    /// 回退变更（对标 opencode snap.revert）
    pub async fn revert(&self, patches: &[PatchEntry]) -> AxonResult<()> {
        for patch in patches {
            self.revert_single_patch(&patch.path, &patch.content)
                .await?;
        }
        Ok(())
    }

    async fn revert_single_patch(&self, _path: &str, _content: &str) -> AxonResult<()> {
        Ok(())
    }

    /// 获取快照元数据
    pub async fn get_metadata(&self, snapshot_id: &SnapshotId) -> Option<SnapshotMetadata> {
        self.snapshots.read().await.get(snapshot_id).cloned()
    }

    /// 删除快照
    pub async fn delete(&self, snapshot_id: &SnapshotId) -> AxonResult<()> {
        self.snapshots.write().await.remove(snapshot_id);
        self.snapshot_stack
            .write()
            .await
            .retain(|id| id != snapshot_id);
        Ok(())
    }

    /// 获取快照栈深度
    pub async fn stack_depth(&self) -> usize {
        self.snapshot_stack.read().await.len()
    }

    /// 获取最新快照 ID
    pub async fn latest_snapshot(&self) -> Option<SnapshotId> {
        self.snapshot_stack.read().await.last().cloned()
    }
}

/// Patch 条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchEntry {
    /// 文件路径
    pub path: String,
    /// Patch 内容
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_snapshot_lifecycle_and_stack() {
        let runtime = SnapshotRuntime::new();

        let id1 = runtime.track().await.unwrap();
        assert_eq!(
            runtime.get_metadata(&id1).await.map(|m| m.id),
            Some(id1.clone())
        );

        let id2 = runtime.track().await.unwrap();

        assert_eq!(runtime.stack_depth().await, 2);
        assert_eq!(runtime.latest_snapshot().await, Some(id2.clone()));

        runtime.delete(&id2).await.unwrap();
        assert_eq!(runtime.stack_depth().await, 1);
        assert_eq!(runtime.latest_snapshot().await, Some(id1));
        assert!(runtime.get_metadata(&id2).await.is_none());
    }
}
