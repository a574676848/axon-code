//! Commit History Analyzer - 提交历史分析器。
//!
//! 分析 Git 提交历史，提取模式和统计信息。

use std::collections::HashMap;
use std::path::Path;

use axon_process::{non_interactive_command, non_interactive_tokio_command};

/// 提交类型统计。
#[derive(Debug, Clone, Default)]
pub struct CommitTypeStats {
    /// 功能提交数
    pub feat: u32,
    /// 修复提交数
    pub fix: u32,
    /// 文档提交数
    pub docs: u32,
    /// 重构提交数
    pub refactor: u32,
    /// 测试提交数
    pub test: u32,
    /// CI 提交数
    pub ci: u32,
    /// 其他提交数
    pub other: u32,
}

/// 提交模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitPattern {
    /// 增量功能
    IncrementalFeature(String),
    /// Bug 批量修复
    BugFixBatch(Vec<String>),
    /// 热修复
    Hotfix,
    /// 发布准备
    ReleasePrep,
    /// 重构
    Refactoring,
    /// 未知模式
    Unknown,
}

/// 提交历史条目。
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// 提交 SHA
    pub sha: String,
    /// 时间戳
    pub timestamp: i64,
    /// 提交消息
    pub message: String,
    /// 提交类型
    pub commit_type: String,
    /// 提交范围
    pub scope: Option<String>,
}

/// 提交历史分析结果。
#[derive(Debug, Clone, Default)]
pub struct HistoryAnalysis {
    /// 总提交数
    pub total_commits: u32,
    /// 类型统计
    pub type_stats: CommitTypeStats,
    /// 作者统计
    pub author_stats: HashMap<String, u32>,
    /// 提交频率
    pub frequency: CommitFrequency,
    /// 模式列表
    pub patterns: Vec<CommitPattern>,
    /// 汇总信息
    pub summary: String,
}

/// 提交频率。
#[derive(Debug, Clone, Default)]
pub struct CommitFrequency {
    /// 日均提交数
    pub daily_avg: f32,
    /// 周均提交数
    pub weekly_avg: f32,
    /// 月均提交数
    pub monthly_avg: f32,
    /// 上周提交数
    pub last_week: u32,
    /// 上月提交数
    pub last_month: u32,
}

/// 历史分析器。
#[derive(Debug, Default, Clone, Copy)]
pub struct CommitHistoryAnalyzer;

impl CommitHistoryAnalyzer {
    /// 创建新实例。
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// 获取最近的提交历史。
    pub fn get_recent_history(
        &self,
        cwd: &Path,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, String> {
        self.get_history(cwd, Some(limit))
    }

    fn get_history(&self, cwd: &Path, limit: Option<usize>) -> Result<Vec<HistoryEntry>, String> {
        let mut command = non_interactive_command("git");
        command.current_dir(cwd).arg("log");
        if let Some(limit) = limit {
            command.arg(format!("-{limit}"));
        }
        let output = command
            .arg("--pretty=format:%H%x1f%ct%x1f%s")
            .output()
            .map_err(|error| format!("执行 git 命令失败: {error}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(self.parse_log_output(&String::from_utf8_lossy(&output.stdout)))
    }

    fn parse_log_output(&self, output: &str) -> Vec<HistoryEntry> {
        let mut entries = Vec::new();

        for line in output.lines() {
            let parts: Vec<&str> = line.split('\u{001f}').collect();
            if parts.len() < 3 {
                continue;
            }

            let sha = parts[0].to_string();
            let timestamp: i64 = parts[1].parse().unwrap_or(0);
            let message = parts[2].to_string();

            let (commit_type, scope) = self.parse_message_header(&message);

            entries.push(HistoryEntry {
                sha,
                timestamp,
                message,
                commit_type,
                scope,
            });
        }

        entries
    }

    fn parse_message_header(&self, message: &str) -> (String, Option<String>) {
        let first_line = message.lines().next().unwrap_or(message);
        let header = first_line.split(':').next().unwrap_or(first_line);

        if let Some(paren_pos) = header.find('(') {
            let commit_type = header[..paren_pos].to_string();
            let scope = header[paren_pos..]
                .trim_start_matches('(')
                .trim_end_matches(')')
                .to_string();
            (commit_type, Some(scope))
        } else {
            (header.to_string(), None)
        }
    }

    /// 分析提交历史。
    pub fn analyze(&self, cwd: &Path) -> Result<HistoryAnalysis, String> {
        let entries = self.get_history(cwd, None)?;
        self.analyze_entries(&entries)
    }

    fn analyze_entries(&self, entries: &[HistoryEntry]) -> Result<HistoryAnalysis, String> {
        let mut analysis = HistoryAnalysis {
            total_commits: entries.len() as u32,
            ..HistoryAnalysis::default()
        };

        for entry in entries {
            match entry.commit_type.as_str() {
                "feat" => analysis.type_stats.feat += 1,
                "fix" => analysis.type_stats.fix += 1,
                "docs" => analysis.type_stats.docs += 1,
                "refactor" => analysis.type_stats.refactor += 1,
                "test" => analysis.type_stats.test += 1,
                "ci" | "build" => analysis.type_stats.ci += 1,
                _ => analysis.type_stats.other += 1,
            }
        }

        self.compute_frequency(&mut analysis, entries);
        self.detect_patterns(&mut analysis, entries);

        analysis.summary = format!(
            "{} commits analyzed: {} feats, {} fixes, {} refactors",
            analysis.total_commits,
            analysis.type_stats.feat,
            analysis.type_stats.fix,
            analysis.type_stats.refactor
        );

        Ok(analysis)
    }

    fn compute_frequency(&self, analysis: &mut HistoryAnalysis, entries: &[HistoryEntry]) {
        if entries.is_empty() {
            return;
        }

        let now = chrono::Utc::now().timestamp();
        let mut last_week = 0u32;
        let mut last_month = 0u32;

        for entry in entries {
            let days_ago = (now - entry.timestamp) / 86400;
            if days_ago <= 7 {
                last_week += 1;
                last_month += 1;
            } else if days_ago <= 30 {
                last_month += 1;
            }
        }

        analysis.frequency.last_week = last_week;
        analysis.frequency.last_month = last_month;

        if !entries.is_empty() {
            let oldest = entries.last().map_or(now, |e| e.timestamp);
            let span_days = ((now - oldest) / 86400).max(1) as f32;
            let span_weeks = span_days / 7.0;
            let span_months = span_days / 30.0;

            analysis.frequency.daily_avg = entries.len() as f32 / span_days;
            analysis.frequency.weekly_avg = entries.len() as f32 / span_weeks;
            analysis.frequency.monthly_avg = entries.len() as f32 / span_months;
        }
    }

    fn detect_patterns(&self, analysis: &mut HistoryAnalysis, entries: &[HistoryEntry]) {
        let fix_count = analysis.type_stats.fix;
        if fix_count >= 3 {
            analysis.patterns.push(CommitPattern::BugFixBatch(
                entries
                    .iter()
                    .filter(|e| e.commit_type == "fix")
                    .map(|e| e.sha[..7].to_string())
                    .collect(),
            ));
        }

        let feat_count = analysis.type_stats.feat;
        if feat_count >= 5 {
            analysis
                .patterns
                .push(CommitPattern::IncrementalFeature(format!(
                    "{feat_count} features"
                )));
        }

        if analysis.type_stats.refactor > analysis.type_stats.feat {
            analysis.patterns.push(CommitPattern::Refactoring);
        }

        let recent_shas: Vec<_> = entries.iter().map(|e| &e.sha[..7]).collect();
        if recent_shas.windows(2).any(|w| {
            let a = entries.iter().find(|e| e.sha.starts_with(w[0]));
            let b = entries.iter().find(|e| e.sha.starts_with(w[1]));
            if let (Some(ac), Some(bc)) = (a, b) {
                (bc.timestamp - ac.timestamp).abs() < 3600
            } else {
                false
            }
        }) {
            analysis.patterns.push(CommitPattern::Hotfix);
        }
    }

    /// 生成分析报告。
    #[must_use]
    pub fn generate_report(&self, analysis: &HistoryAnalysis) -> String {
        let mut report = String::new();
        report.push_str("# 提交历史分析报告\n\n");
        report.push_str(&format!(
            "## 概览\n总提交数: {}\n\n",
            analysis.total_commits
        ));

        report.push_str("### 类型分布\n");
        report.push_str(&format!("- feat: {}\n", analysis.type_stats.feat));
        report.push_str(&format!("- fix: {}\n", analysis.type_stats.fix));
        report.push_str(&format!("- docs: {}\n", analysis.type_stats.docs));
        report.push_str(&format!("- refactor: {}\n", analysis.type_stats.refactor));
        report.push_str(&format!("- test: {}\n", analysis.type_stats.test));
        report.push_str(&format!("- ci/build: {}\n", analysis.type_stats.ci));
        report.push_str(&format!("- other: {}\n\n", analysis.type_stats.other));

        report.push_str("### 提交频率\n");
        report.push_str(&format!("- 日均: {:.2}\n", analysis.frequency.daily_avg));
        report.push_str(&format!("- 周均: {:.2}\n", analysis.frequency.weekly_avg));
        report.push_str(&format!("- 月均: {:.2}\n", analysis.frequency.monthly_avg));
        report.push_str(&format!("- 近一周: {}\n", analysis.frequency.last_week));
        report.push_str(&format!("- 近一月: {}\n\n", analysis.frequency.last_month));

        if !analysis.patterns.is_empty() {
            report.push_str("### 检测到的模式\n");
            for pattern in &analysis.patterns {
                match pattern {
                    CommitPattern::IncrementalFeature(s) => {
                        report.push_str(&format!("- 增量功能开发: {s}\n"));
                    }
                    CommitPattern::BugFixBatch(shas) => {
                        report.push_str(&format!("- Bug 修复批次: {shas:?}\n"));
                    }
                    CommitPattern::Hotfix => {
                        report.push_str("- Hotfix 模式\n");
                    }
                    CommitPattern::Refactoring => {
                        report.push_str("- 重构阶段\n");
                    }
                    CommitPattern::ReleasePrep => {
                        report.push_str("- 发布准备\n");
                    }
                    CommitPattern::Unknown => {
                        report.push_str("- 未知模式\n");
                    }
                }
            }
        }

        report
    }

    /// 异步获取最近的提交历史。
    pub async fn get_recent_history_async(
        &self,
        cwd: &Path,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, String> {
        self.get_history_async(cwd, Some(limit)).await
    }

    async fn get_history_async(
        &self,
        cwd: &Path,
        limit: Option<usize>,
    ) -> Result<Vec<HistoryEntry>, String> {
        let mut command = non_interactive_tokio_command("git");
        command.current_dir(cwd).arg("log");
        if let Some(limit) = limit {
            command.arg(format!("-{limit}"));
        }
        let output = command
            .arg("--pretty=format:%H%x1f%ct%x1f%s")
            .output()
            .await
            .map_err(|error| format!("执行 git 命令失败: {error}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(self.parse_log_output(&String::from_utf8_lossy(&output.stdout)))
    }

    /// 异步分析提交历史。
    pub async fn analyze_async(&self, cwd: &Path) -> Result<HistoryAnalysis, String> {
        let entries = self.get_history_async(cwd, None).await?;
        self.analyze_entries(&entries)
    }

    /// 异步获取特定分支的提交历史。
    pub async fn get_branch_history_async(
        &self,
        cwd: &Path,
        branch: &str,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, String> {
        let limit_arg = format!("-{limit}");
        let output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args(["log", branch, &limit_arg, "--pretty=format:%H%x1f%ct%x1f%s"])
            .output()
            .await
            .map_err(|error| format!("执行 git 命令失败: {error}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).to_string());
        }
        Ok(self.parse_log_output(&String::from_utf8_lossy(&output.stdout)))
    }

    /// 比较两个分支的提交差异。
    pub async fn compare_branches(
        &self,
        cwd: &Path,
        base_branch: &str,
        compare_branch: &str,
    ) -> Result<(Vec<HistoryEntry>, Vec<HistoryEntry>), String> {
        let base_output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args([
                "log",
                &format!("{base_branch}..{compare_branch}"),
                "--pretty=format:%H%x1f%ct%x1f%s",
            ])
            .output()
            .await
            .map_err(|error| format!("执行 git 命令失败: {error}"))?;
        let compare_output = non_interactive_tokio_command("git")
            .current_dir(cwd)
            .args([
                "log",
                &format!("{compare_branch}..{base_branch}"),
                "--pretty=format:%H%x1f%ct%x1f%s",
            ])
            .output()
            .await
            .map_err(|error| format!("执行 git 命令失败: {error}"))?;

        if !base_output.status.success() {
            return Err(String::from_utf8_lossy(&base_output.stderr).to_string());
        }
        if !compare_output.status.success() {
            return Err(String::from_utf8_lossy(&compare_output.stderr).to_string());
        }
        let base_entries = self.parse_log_output(&String::from_utf8_lossy(&base_output.stdout));
        let compare_entries =
            self.parse_log_output(&String::from_utf8_lossy(&compare_output.stdout));
        Ok((base_entries, compare_entries))
    }
}

#[cfg(test)]
mod tests {
    use super::CommitHistoryAnalyzer;

    #[test]
    fn parse_message_header_extracts_scope_without_panicking() {
        let analyzer = CommitHistoryAnalyzer::new();

        let (commit_type, scope) = analyzer.parse_message_header("feat(runtime): add scheduler");
        assert_eq!(commit_type, "feat");
        assert_eq!(scope.as_deref(), Some("runtime"));

        let (commit_type, scope) =
            analyzer.parse_message_header("fix(runtime: tolerate malformed header");
        assert_eq!(commit_type, "fix");
        assert_eq!(scope.as_deref(), Some("runtime"));
    }
}
