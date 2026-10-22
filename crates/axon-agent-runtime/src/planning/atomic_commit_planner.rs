//! Atomic Commit Planner - 原子化提交规划器。
//!
//! 基于 Git 文件变更计划原子化提交。
//!
//! 核心能力：
//! - `AtomicCommit` - 原子提交单元
//! - `CommitGroup` - 按策略分组的提交组
//! - `AtomicCommitPlanner` - 规划器主结构

use std::path::Path;

use axon_process::{non_interactive_command, non_interactive_tokio_command};

#[path = "atomic_commit_planner_groups.rs"]
mod atomic_commit_planner_groups;

pub use crate::atomic_commit_planner_support::{FileChangeStatus, difference, intersection};

impl AtomicCommitPlanner {
    /// 计算与上游分支的 merge-base。
    pub fn compute_merge_base(&self, cwd: &Path, upstream: &str) -> Result<String, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["merge-base", "HEAD", upstream])
            .output()
            .map_err(|e| format!("执行 git merge-base 失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// 异步计算 merge-base。
    pub async fn compute_merge_base_async(
        &self,
        cwd: &Path,
        upstream: &str,
    ) -> Result<String, String> {
        let output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args(["merge-base", "HEAD", upstream])
            .output()
            .await
            .map_err(|error| format!("执行 git merge-base 失败: {error}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// 获取当前分支名。
    pub fn get_current_branch(&self, cwd: &Path) -> Result<String, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() {
            return Err("无法获取当前分支名".to_string());
        }
        Ok(branch)
    }

    /// 获取 HEAD commit SHA。
    pub fn get_head_sha(&self, cwd: &Path) -> Result<String, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// 获取自从 baseline 以来的变更文件。
    pub fn get_changes_since_baseline(
        &self,
        cwd: &Path,
        baseline: &str,
    ) -> Result<Vec<FileChange>, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["diff", baseline, "HEAD", "--stat", "--patch"])
            .output()
            .map_err(|e| format!("执行 git diff 失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        self.parse_diff_stat(&String::from_utf8_lossy(&output.stdout))
    }

    /// 验证是否为有效 Git 仓库。
    pub fn validate_git_repo(&self, cwd: &Path) -> Result<(), String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["rev-parse", "--git-dir"])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(format!("不是 Git 仓库: {}", cwd.display()));
        }
        Ok(())
    }

    /// 验证提交计划的完整性。
    pub fn validate_plan(&self, plan: &[AtomicCommit], all_changes: &[FileChange]) -> bool {
        let planned_files: Vec<_> = plan.iter().flat_map(|c| &c.files).collect();
        let expected_files: Vec<_> = all_changes.iter().map(|c| &c.path).collect();

        if planned_files.len() != expected_files.len() {
            return false;
        }

        for expected in &expected_files {
            if !planned_files.contains(expected) {
                return false;
            }
        }

        true
    }
}

/// 单个文件的变更信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// 文件路径
    pub path: String,
    /// 变更状态
    pub status: FileChangeStatus,
    /// 重命名前路径。
    pub old_path: Option<String>,
    /// Hunk 块列表
    pub hunks: Vec<FileHunk>,
}

/// 文件的 Hunk 块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHunk {
    /// 原始文件起始行
    pub old_start: u32,
    /// 原始行数
    pub old_count: u32,
    /// 新文件起始行
    pub new_start: u32,
    /// 新行数
    pub new_count: u32,
    /// 增加行数
    pub additions: u32,
    /// 删除行数
    pub deletions: u32,
}

/// 原子提交单元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicCommit {
    /// 变更文件列表
    pub files: Vec<String>,
    /// 提交消息
    pub message: String,
    /// 提交类型
    pub commit_type: AtomicCommitType,
    /// 提交范围
    pub scope: Option<String>,
}

/// 原子提交类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicCommitType {
    /// 功能提交
    Feature,
    /// Bug 修复
    Fix,
    /// 重构提交
    Refactor,
    /// 文档提交
    Docs,
    /// 配置提交
    Config,
    /// 测试提交
    Test,
    /// 杂项提交
    Chore,
}

impl AtomicCommitType {
    /// 获取对应的提交类型前缀。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Feature => "feat",
            Self::Fix => "fix",
            Self::Refactor => "refactor",
            Self::Docs => "docs",
            Self::Config => "config",
            Self::Test => "test",
            Self::Chore => "chore",
        }
    }
}

/// 提交分组策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitGroupStrategy {
    /// 按文件类型分组
    ByFileType,
    /// 按变更目录分组
    ByDirectory,
    /// 按变更大小分组（原子化优先）
    ByChangeSize,
    /// 单文件一个提交
    PerFile,
}

/// 提交分组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitGroup {
    /// 分组名称
    pub name: String,
    /// 文件列表
    pub files: Vec<String>,
    /// 总增加行数
    pub total_additions: u32,
    /// 总删除行数
    pub total_deletions: u32,
    /// 预估提交数
    pub estimated_commits: usize,
}

/// 原子提交规划器。
#[derive(Debug, Clone)]
pub struct AtomicCommitPlanner {
    strategy: CommitGroupStrategy,
    max_files_per_commit: usize,
    max_hunks_per_commit: usize,
}

impl Default for AtomicCommitPlanner {
    fn default() -> Self {
        Self {
            strategy: CommitGroupStrategy::ByDirectory,
            max_files_per_commit: 10,
            max_hunks_per_commit: 50,
        }
    }
}

impl AtomicCommitPlanner {
    /// 创建规划器。
    pub fn new(strategy: CommitGroupStrategy) -> Self {
        Self {
            strategy,
            max_files_per_commit: 10,
            max_hunks_per_commit: 50,
        }
    }

    /// 设置最大文件数。
    pub fn with_max_files(mut self, max: usize) -> Self {
        self.max_files_per_commit = max;
        self
    }

    /// 设置最大 hunk 数。
    pub fn with_max_hunks(mut self, max: usize) -> Self {
        self.max_hunks_per_commit = max;
        self
    }

    /// 获取当前工作目录的变更文件。
    pub fn get_staged_files(&self, cwd: &Path) -> Result<Vec<FileChange>, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["diff", "--cached", "--stat", "--patch"])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        self.parse_diff_stat(&String::from_utf8_lossy(&output.stdout))
    }

    /// 获取当前工作目录的所有变更文件（包括未暂存）。
    pub fn get_all_changes(&self, cwd: &Path) -> Result<Vec<FileChange>, String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["status", "--porcelain"])
            .output()
            .map_err(|e| format!("执行 git 命令失败: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }

        self.parse_status(&String::from_utf8_lossy(&output.stdout))
    }

    /// 从 diff 输出解析文件变更。
    fn parse_diff_stat(&self, output: &str) -> Result<Vec<FileChange>, String> {
        let mut changes = Vec::new();
        let mut current_path: Option<String> = None;
        let mut current_hunks = Vec::new();

        for line in output.lines() {
            if line.starts_with("diff --git") {
                if let Some(path) = current_path.take() {
                    changes.push(FileChange {
                        path,
                        status: FileChangeStatus::Modified,
                        old_path: None,
                        hunks: std::mem::take(&mut current_hunks),
                    });
                }
                if let Some(name) = line.strip_prefix("diff --git a/") {
                    current_path = name.split_once(" b/").map(|(_, b)| b.to_string());
                }
            } else if let Some(_path) = &current_path
                && let Some(hunk) = self.parse_hunk_header(line)
            {
                current_hunks.push(hunk);
            }
        }

        if let Some(path) = current_path {
            changes.push(FileChange {
                path,
                status: FileChangeStatus::Modified,
                old_path: None,
                hunks: current_hunks,
            });
        }

        Ok(changes)
    }

    /// 解析 hunk 头部。
    fn parse_hunk_header(&self, line: &str) -> Option<FileHunk> {
        if !line.starts_with("@@") {
            return None;
        }

        let content = line.strip_prefix("@@").unwrap_or(line);
        let parts: Vec<&str> = content.trim_start_matches('@').split_whitespace().collect();

        if parts.len() < 2 {
            return None;
        }

        // 移除符号前缀 (-, +)
        let parse_num = |s: &str| -> Option<u32> {
            s.strip_prefix('-')
                .or_else(|| s.strip_prefix('+'))
                .unwrap_or(s)
                .parse()
                .ok()
        };

        let old_info: Vec<&str> = parts[0].split(',').collect();
        let new_info: Vec<&str> = parts[1].split(',').collect();

        let old_start: u32 = parse_num(old_info.first()?)?;
        let old_count: u32 = old_info.get(1).and_then(|s| parse_num(s)).unwrap_or(1);
        let new_start: u32 = parse_num(new_info.first()?)?;
        let new_count: u32 = new_info.get(1).and_then(|s| parse_num(s)).unwrap_or(1);

        Some(FileHunk {
            old_start,
            old_count,
            new_start,
            new_count,
            additions: new_count.saturating_sub(1),
            deletions: old_count.saturating_sub(1),
        })
    }

    /// 解析 git status --porcelain 输出。
    fn parse_status(&self, output: &str) -> Result<Vec<FileChange>, String> {
        let mut changes = Vec::new();

        for line in output.lines() {
            if line.len() < 3 {
                continue;
            }

            let status_code = &line[..2];
            let path = line[3..].trim().to_string();

            let (status, old_path) = match status_code {
                "A " => (FileChangeStatus::Added, None),
                "M " | "MM" => (FileChangeStatus::Modified, None),
                "D " => (FileChangeStatus::Deleted, None),
                "R " => {
                    if let Some((old, _new)) = path.split_once(" -> ") {
                        (FileChangeStatus::Renamed, Some(old.to_string()))
                    } else {
                        (FileChangeStatus::Modified, None)
                    }
                }
                "??" => (FileChangeStatus::Added, None),
                _ => continue,
            };

            changes.push(FileChange {
                path,
                status,
                old_path,
                hunks: Vec::new(),
            });
        }

        Ok(changes)
    }

    /// 规划原子化提交。
    pub fn plan(&self, changes: Vec<FileChange>) -> Vec<AtomicCommit> {
        let groups = self.group_changes(changes);
        self.create_commits_from_groups(groups)
    }
}

#[cfg(test)]
#[path = "atomic_commit_planner_tests.rs"]
mod tests;
