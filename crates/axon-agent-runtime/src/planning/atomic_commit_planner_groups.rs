//! 原子提交规划器的分组与消息生成实现。

use std::collections::HashMap;

use super::{
    AtomicCommit, AtomicCommitPlanner, AtomicCommitType, CommitGroup, CommitGroupStrategy,
    FileChange,
};

impl AtomicCommitPlanner {
    /// 按策略分组变更。
    pub(super) fn group_changes(&self, changes: Vec<FileChange>) -> Vec<CommitGroup> {
        match self.strategy {
            CommitGroupStrategy::ByFileType => self.group_by_file_type(changes),
            CommitGroupStrategy::ByDirectory => self.group_by_directory(changes),
            CommitGroupStrategy::ByChangeSize => self.group_by_change_size(changes),
            CommitGroupStrategy::PerFile => self.group_per_file(changes),
        }
    }

    /// 按文件类型分组。
    pub(super) fn group_by_file_type(&self, changes: Vec<FileChange>) -> Vec<CommitGroup> {
        let mut groups: HashMap<String, Vec<FileChange>> = HashMap::new();

        for change in changes {
            let ext = change
                .path
                .rsplit('.')
                .next()
                .unwrap_or("unknown")
                .to_lowercase();
            let category = match ext.as_str() {
                "rs" => "rust",
                "js" | "ts" | "jsx" | "tsx" => "typescript",
                "py" => "python",
                "md" | "mdx" => "documentation",
                "json" | "toml" | "yaml" | "yml" => "config",
                "test" | "spec" => "test",
                _ => "other",
            };
            groups.entry(category.to_string()).or_default().push(change);
        }

        groups
            .into_iter()
            .map(|(name, files)| {
                let (adds, dels) = self.count_changes(&files);
                let file_count = files.len();
                CommitGroup {
                    name,
                    files: files.into_iter().map(|c| c.path).collect(),
                    total_additions: adds,
                    total_deletions: dels,
                    estimated_commits: (file_count / self.max_files_per_commit).max(1),
                }
            })
            .collect()
    }

    /// 按目录分组。
    pub(super) fn group_by_directory(&self, changes: Vec<FileChange>) -> Vec<CommitGroup> {
        let mut groups: HashMap<String, Vec<FileChange>> = HashMap::new();

        for change in changes {
            let dir = change.path.split('/').next().unwrap_or(".").to_string();
            groups.entry(dir).or_default().push(change);
        }

        groups
            .into_iter()
            .map(|(name, files)| {
                let (adds, dels) = self.count_changes(&files);
                let file_count = files.len();
                CommitGroup {
                    name,
                    files: files.into_iter().map(|c| c.path).collect(),
                    total_additions: adds,
                    total_deletions: dels,
                    estimated_commits: (file_count / self.max_files_per_commit).max(1),
                }
            })
            .collect()
    }

    /// 按变更大小分组（保持原子性）。
    pub(super) fn group_by_change_size(&self, changes: Vec<FileChange>) -> Vec<CommitGroup> {
        let mut groups: Vec<CommitGroup> = Vec::new();
        let mut current_group = Vec::new();
        let mut current_changes = 0u32;

        for change in changes {
            let change_size: u32 = change.hunks.iter().map(|h| h.additions + h.deletions).sum();

            if (current_group.len() >= self.max_files_per_commit
                || current_changes >= self.max_hunks_per_commit as u32)
                && !current_group.is_empty()
            {
                let (adds, dels) = self.count_changes(&current_group);
                let dir = current_group
                    .first()
                    .map(|c| c.path.split('/').next().unwrap_or("."))
                    .unwrap_or(".");
                groups.push(CommitGroup {
                    name: dir.to_string(),
                    files: current_group.into_iter().map(|c| c.path).collect(),
                    total_additions: adds,
                    total_deletions: dels,
                    estimated_commits: 1,
                });
                current_group = Vec::new();
                current_changes = 0;
            }

            current_group.push(change);
            current_changes += change_size;
        }

        if !current_group.is_empty() {
            let (adds, dels) = self.count_changes(&current_group);
            let dir = current_group
                .first()
                .map(|c| c.path.split('/').next().unwrap_or("."))
                .unwrap_or(".");
            groups.push(CommitGroup {
                name: dir.to_string(),
                files: current_group.into_iter().map(|c| c.path).collect(),
                total_additions: adds,
                total_deletions: dels,
                estimated_commits: 1,
            });
        }

        groups
    }

    /// 单文件一个提交。
    pub(super) fn group_per_file(&self, changes: Vec<FileChange>) -> Vec<CommitGroup> {
        changes
            .into_iter()
            .map(|c| {
                let (adds, dels) = self.count_changes(std::slice::from_ref(&c));
                CommitGroup {
                    name: c.path.split('/').next().unwrap_or(".").to_string(),
                    files: vec![c.path],
                    total_additions: adds,
                    total_deletions: dels,
                    estimated_commits: 1,
                }
            })
            .collect()
    }

    /// 统计变更行数。
    pub(super) fn count_changes(&self, changes: &[FileChange]) -> (u32, u32) {
        let mut additions = 0u32;
        let mut deletions = 0u32;

        for change in changes {
            for hunk in &change.hunks {
                additions += hunk.additions;
                deletions += hunk.deletions;
            }
        }

        (additions, deletions)
    }

    /// 从分组创建提交。
    pub(super) fn create_commits_from_groups(&self, groups: Vec<CommitGroup>) -> Vec<AtomicCommit> {
        groups
            .into_iter()
            .map(|group| {
                let commit_type = self.infer_commit_type(&group.files);
                let scope = self.extract_scope(&group.files);
                let message = self.generate_message(&group, commit_type, scope.as_deref());

                AtomicCommit {
                    files: group.files,
                    message,
                    commit_type,
                    scope,
                }
            })
            .collect()
    }

    /// 推断提交类型。
    pub(super) fn infer_commit_type(&self, files: &[String]) -> AtomicCommitType {
        let has_test = files
            .iter()
            .any(|f| f.contains("test") || f.contains("_test"));
        let has_docs = files
            .iter()
            .any(|f| f.ends_with(".md") || f.contains("readme"));
        let has_config = files
            .iter()
            .any(|f| f.ends_with(".json") || f.ends_with(".toml") || f.ends_with(".yaml"));

        if has_test && !has_docs && !has_config {
            return AtomicCommitType::Test;
        }
        if has_docs && !has_test && !has_config {
            return AtomicCommitType::Docs;
        }
        if has_config && !has_test && !has_docs {
            return AtomicCommitType::Config;
        }

        AtomicCommitType::Feature
    }

    /// 提取提交范围。
    pub(super) fn extract_scope(&self, files: &[String]) -> Option<String> {
        files
            .first()
            .and_then(|f| f.split('/').next().map(String::from))
    }

    /// 生成提交消息。
    pub(super) fn generate_message(
        &self,
        group: &CommitGroup,
        commit_type: AtomicCommitType,
        scope: Option<&str>,
    ) -> String {
        let type_str = commit_type.as_str();
        let scope_str = scope.map(|s| format!("({s})")).unwrap_or_default();

        match group.files.len() {
            1 => format!("{type_str}{scope_str}: {}", group.name),
            n if n <= 3 => {
                let files = group.files.join(", ");
                format!("{type_str}{scope_str}: update {}", files)
            }
            _ => format!("{type_str}{scope_str}: {} files", group.files.len()),
        }
    }
}
