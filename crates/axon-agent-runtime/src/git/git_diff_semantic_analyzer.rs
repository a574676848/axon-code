//! Git Diff Semantic Analyzer - Git差异语义分析器。
//!
//! 分析 Git diff 的语义信息，提取变更类型、影响范围和风险评估。
//!
//! 核心能力：
//! - `SemanticDiff` - 语义化差异结构
//! - `ChangeSemantic` - 变更语义类型
//! - `RiskLevel` - 风险等级
//! - `DiffAnalyzer` - 分析器
//! - 异步执行
//! - 语义化错误类型（对标 codex GitToolingError）

use std::collections::HashMap;
use std::path::Path;

use axon_process::{non_interactive_command, non_interactive_tokio_command};

pub use crate::git_diff_semantic_types::{
    BaselineFileChange, ChangeSemantic, FileSemanticAnalysis, FileStats, GitAnalyzerError,
    GitAnalyzerResult, GitBaseline, GitBaselineChange, ModuleImpact, RiskLevel, RiskThresholds,
    SemanticDiff,
};

#[path = "git_diff_semantic_report.rs"]
mod report;

/// 差异语义分析器。
#[derive(Debug, Clone)]
pub struct DiffAnalyzer {
    risk_thresholds: RiskThresholds,
}

impl Default for DiffAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl DiffAnalyzer {
    /// 创建分析器。
    pub fn new() -> Self {
        Self {
            risk_thresholds: RiskThresholds {
                high_risk_extensions: vec!["rs", "go", "java", "cs", "ts", "js"],
                critical_paths: vec![
                    "src/main", "lib/main", "api", "auth", "security", "payment", "core",
                ],
            },
        }
    }

    /// 检查路径是否为有效 Git 仓库
    pub fn validate_git_repo(&self, cwd: &Path) -> GitAnalyzerResult<()> {
        if !cwd.exists() {
            return Err(GitAnalyzerError::PathNotFound {
                path: cwd.display().to_string(),
            });
        }
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["rev-parse", "--git-dir"])
            .output()
            .map_err(|e| GitAnalyzerError::Other {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::NotARepository {
                path: cwd.display().to_string(),
            });
        }
        Ok(())
    }

    /// 分析目录的 diff。
    pub fn analyze_directory(&self, cwd: &Path) -> Result<SemanticDiff, String> {
        self.analyze_directory_inner(cwd).map_err(|e| e.to_string())
    }

    /// 分析目录的 diff（内部实现，使用语义化错误类型）。
    fn analyze_directory_inner(&self, cwd: &Path) -> GitAnalyzerResult<SemanticDiff> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["diff", "--stat", "--patch"])
            .output()
            .map_err(|e| GitAnalyzerError::CommandFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        self.parse_and_analyze(&output_str)
            .map_err(|e| GitAnalyzerError::ParseError { message: e })
    }

    /// 分析暂存区的 diff。
    pub fn analyze_staged(&self, cwd: &Path) -> Result<SemanticDiff, String> {
        self.analyze_staged_inner(cwd).map_err(|e| e.to_string())
    }

    /// 分析暂存区的 diff（内部实现，使用语义化错误类型）。
    fn analyze_staged_inner(&self, cwd: &Path) -> GitAnalyzerResult<SemanticDiff> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["diff", "--cached", "--stat", "--patch"])
            .output()
            .map_err(|e| GitAnalyzerError::CommandFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        self.parse_and_analyze(&output_str)
            .map_err(|e| GitAnalyzerError::ParseError { message: e })
    }

    /// 异步分析目录的 diff。
    pub async fn analyze_directory_async(&self, cwd: &Path) -> GitAnalyzerResult<SemanticDiff> {
        let output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args(["diff", "--stat", "--patch"])
            .output()
            .await
            .map_err(|error| GitAnalyzerError::CommandFailed {
                message: error.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        self.parse_and_analyze(&String::from_utf8_lossy(&output.stdout))
            .map_err(|message| GitAnalyzerError::ParseError { message })
    }

    /// 异步分析暂存区的 diff。
    pub async fn analyze_staged_async(&self, cwd: &Path) -> GitAnalyzerResult<SemanticDiff> {
        let output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args(["diff", "--cached", "--stat", "--patch"])
            .output()
            .await
            .map_err(|error| GitAnalyzerError::CommandFailed {
                message: error.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        self.parse_and_analyze(&String::from_utf8_lossy(&output.stdout))
            .map_err(|message| GitAnalyzerError::ParseError { message })
    }

    /// 获取当前分支名（对标 codex branch.rs）。
    pub fn get_current_branch(&self, cwd: &Path) -> GitAnalyzerResult<String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .map_err(|e| GitAnalyzerError::CommandFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() {
            return Err(GitAnalyzerError::Other {
                message: "Failed to get current branch".to_string(),
            });
        }
        Ok(branch)
    }

    /// 获取 HEAD commit SHA（对标 codex baseline.rs）。
    pub fn get_head_sha(&self, cwd: &Path) -> GitAnalyzerResult<String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["rev-parse", "HEAD"])
            .output()
            .map_err(|e| GitAnalyzerError::CommandFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(sha)
    }

    /// 计算与另一个分支的 merge base（对标 codex merge_base_with_head）。
    pub fn merge_base_with_branch(&self, cwd: &Path, upstream: &str) -> GitAnalyzerResult<String> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["merge-base", "HEAD", upstream])
            .output()
            .map_err(|e| GitAnalyzerError::CommandFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: format!(
                    "Failed to compute merge-base with '{}': {}",
                    upstream,
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(sha)
    }

    /// 获取 diff_since_latest_init 风格的 diff（对标 codex gix baseline）。
    pub fn diff_since_baseline(
        &self,
        cwd: &Path,
        base_sha: &str,
    ) -> GitAnalyzerResult<SemanticDiff> {
        let output = non_interactive_command("git")
            .current_dir(cwd)
            .args(["diff", base_sha, "HEAD", "--stat", "--patch"])
            .output()
            .map_err(|e| GitAnalyzerError::CommandFailed {
                message: e.to_string(),
            })?;

        if !output.status.success() {
            return Err(GitAnalyzerError::CommandFailed {
                message: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }

        let output_str = String::from_utf8_lossy(&output.stdout);
        self.parse_and_analyze(&output_str)
            .map_err(|e| GitAnalyzerError::ParseError { message: e })
    }

    /// 解析并分析 diff。
    fn parse_and_analyze(&self, output: &str) -> Result<SemanticDiff, String> {
        let mut diff = SemanticDiff::default();
        let mut current_path: Option<String> = None;
        let mut current_additions = 0u32;
        let mut current_deletions = 0u32;
        let mut current_hunks = 0u32;

        for line in output.lines() {
            if line.starts_with("diff --git") {
                if let Some(path) = current_path.take() {
                    let analysis = self.analyze_file(
                        &mut diff,
                        &path,
                        current_additions,
                        current_deletions,
                        current_hunks,
                    );
                    diff.files.push(analysis);
                }

                if let Some(name) = line.strip_prefix("diff --git a/") {
                    current_path = name.split_once(" b/").map(|(_, b)| b.to_string());
                }
                current_additions = 0;
                current_deletions = 0;
                current_hunks = 0;
            } else if let Some(path) = &current_path {
                if line.starts_with("@@") {
                    current_hunks += 1;
                }
                if line.starts_with('+') && !line.starts_with("+++") {
                    current_additions += 1;
                }
                if line.starts_with('-') && !line.starts_with("---") {
                    current_deletions += 1;
                }
                if line.starts_with("Binary files") {
                    let analysis = self.analyze_file(
                        &mut diff,
                        path,
                        current_additions,
                        current_deletions,
                        current_hunks,
                    );
                    diff.files.push(analysis);
                    current_path = None;
                }
            } else if line.contains("changed, ") || line.contains("changed,") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                for (i, part) in parts.iter().enumerate() {
                    if *part == "changed,"
                        && let Some(changed) = parts.get(i.saturating_sub(1))
                        && let Ok(n) = changed.parse::<u32>()
                    {
                        diff.stats.files_changed = n;
                    }
                }
            }
        }

        if let Some(path) = current_path {
            let analysis = self.analyze_file(
                &mut diff,
                &path,
                current_additions,
                current_deletions,
                current_hunks,
            );
            diff.files.push(analysis);
        }

        self.compute_summary(&mut diff);

        Ok(diff)
    }

    /// 分析单个文件。
    fn analyze_file(
        &self,
        diff: &mut SemanticDiff,
        path: &str,
        additions: u32,
        deletions: u32,
        hunks: u32,
    ) -> FileSemanticAnalysis {
        let semantic = self.classify_semantic(path);
        let risk_level = self.assess_risk(path, &semantic, additions, deletions);
        let affected_modules = self.extract_affected_modules(path);
        let impact_score = self.calculate_impact_score(additions, deletions, hunks, &risk_level);

        diff.stats.additions += additions;
        diff.stats.deletions += deletions;
        diff.stats.hunks += hunks;

        FileSemanticAnalysis {
            path: path.to_string(),
            semantic,
            risk_level,
            additions,
            deletions,
            hunks,
            affected_modules,
            impact_score,
        }
    }

    /// 分类变更语义。
    fn classify_semantic(&self, path: &str) -> ChangeSemantic {
        let path_lower = path.to_lowercase();

        if path_lower.contains("test") || path_lower.contains("_test") {
            return ChangeSemantic::TestChange;
        }
        if path_lower.ends_with(".md")
            || path_lower.contains("readme")
            || path_lower.contains("changelog")
        {
            return ChangeSemantic::DocumentationUpdate;
        }
        if path_lower.contains("cargo")
            || path_lower.contains("package.json")
            || path_lower.contains("requirements")
            || path_lower.contains("go.mod")
            || path_lower.contains("pom.xml")
        {
            return ChangeSemantic::DependencyChange;
        }
        if path_lower.contains("config")
            || path_lower.contains(".env")
            || path_lower.contains("settings")
        {
            return ChangeSemantic::ConfigurationChange;
        }
        if path_lower.contains("css")
            || path_lower.contains("style")
            || path_lower.contains("theme")
            || path_lower.ends_with(".scss")
        {
            return ChangeSemantic::UiStyleChange;
        }
        if path_lower.contains("fix") || path_lower.contains("bug") || path_lower.contains("hotfix")
        {
            return ChangeSemantic::BugFix;
        }
        if path_lower.contains("perf")
            || path_lower.contains("optimize")
            || path_lower.contains("speed")
        {
            return ChangeSemantic::PerformanceOptimization;
        }
        if path_lower.contains("refactor") || path_lower.contains("rename") {
            return ChangeSemantic::Refactoring;
        }

        ChangeSemantic::FeatureAddition
    }

    /// 评估风险等级。
    fn assess_risk(
        &self,
        path: &str,
        semantic: &ChangeSemantic,
        additions: u32,
        deletions: u32,
    ) -> RiskLevel {
        let path_lower = path.to_lowercase();
        let total_changes = additions + deletions;

        // 检查关键路径
        for critical in &self.risk_thresholds.critical_paths {
            if path_lower.contains(critical) {
                return RiskLevel::Critical;
            }
        }

        // 检查高风险扩展名
        if let Some(ext) = path.rsplit('.').next()
            && self.risk_thresholds.is_high_risk_extension(ext)
            && total_changes > 30
        {
            return RiskLevel::High;
        }

        if semantic == &ChangeSemantic::DependencyChange && total_changes > 20 {
            return RiskLevel::High;
        }

        if semantic == &ChangeSemantic::ConfigurationChange && total_changes > 10 {
            return RiskLevel::High;
        }

        if total_changes > 100 {
            return RiskLevel::High;
        }

        if total_changes > 50 {
            return RiskLevel::Medium;
        }

        RiskLevel::Low
    }

    /// 提取受影响的模块。
    fn extract_affected_modules(&self, path: &str) -> Vec<String> {
        let mut modules = Vec::new();

        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            modules.push(parts[0].to_string());
        }
        if parts.len() >= 3 {
            modules.push(format!("{}/{}", parts[0], parts[1]));
        }

        modules
    }

    /// 计算影响分数。
    fn calculate_impact_score(
        &self,
        additions: u32,
        deletions: u32,
        hunks: u32,
        risk_level: &RiskLevel,
    ) -> f32 {
        let change_weight = (additions + deletions) as f32;
        let hunk_weight = hunks as f32 * 0.1;
        let risk_weight = match risk_level {
            RiskLevel::Critical => 4.0,
            RiskLevel::High => 2.0,
            RiskLevel::Medium => 1.0,
            RiskLevel::Low => 0.5,
        };

        (change_weight + hunk_weight) * risk_weight / 10.0
    }

    /// 计算汇总信息。
    fn compute_summary(&self, diff: &mut SemanticDiff) {
        diff.overall_risk = diff
            .files
            .iter()
            .map(|f| f.risk_level)
            .max()
            .unwrap_or(RiskLevel::Low);

        let mut module_map: HashMap<String, ModuleImpact> = HashMap::new();

        for file in &diff.files {
            for module in &file.affected_modules {
                let entry = module_map
                    .entry(module.clone())
                    .or_insert_with(|| ModuleImpact {
                        module: module.clone(),
                        files: Vec::new(),
                        total_additions: 0,
                        total_deletions: 0,
                        impact_score: 0.0,
                    });
                entry.files.push(file.path.clone());
                entry.total_additions += file.additions;
                entry.total_deletions += file.deletions;
                entry.impact_score += file.impact_score;
            }
        }

        diff.module_impacts = module_map.into_values().collect();
        diff.module_impacts
            .sort_by(|a, b| b.impact_score.total_cmp(&a.impact_score));

        diff.summary = format!(
            "{} files changed, +{} -{} lines, {} hunks, risk: {}",
            diff.stats.files_changed,
            diff.stats.additions,
            diff.stats.deletions,
            diff.stats.hunks,
            diff.overall_risk
        );
    }
}

#[cfg(test)]
#[path = "git_diff_semantic_analyzer_tests.rs"]
mod tests;
