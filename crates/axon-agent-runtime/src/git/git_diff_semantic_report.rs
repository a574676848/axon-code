use super::{DiffAnalyzer, RiskLevel, SemanticDiff};

impl DiffAnalyzer {
    /// 生成变更报告。
    pub fn generate_report(&self, diff: &SemanticDiff) -> String {
        let mut report = String::new();

        report.push_str("# 语义化差异分析报告\n\n");

        // 如果 summary 为空（测试场景），构建一个
        let summary = if diff.summary.is_empty() {
            format!(
                "{} files changed, +{} -{} lines, {} hunks, risk: {}",
                diff.stats.files_changed,
                diff.stats.additions,
                diff.stats.deletions,
                diff.stats.hunks,
                diff.overall_risk
            )
        } else {
            diff.summary.clone()
        };

        report.push_str(&format!("## 汇总\n{summary}\n\n"));

        if !diff.module_impacts.is_empty() {
            report.push_str("## 模块影响\n\n");
            for impact in &diff.module_impacts[..diff.module_impacts.len().min(5)] {
                report.push_str(&format!(
                    "- **{}**: {} files, +{} -{} (score: {:.2})\n",
                    impact.module,
                    impact.files.len(),
                    impact.total_additions,
                    impact.total_deletions,
                    impact.impact_score
                ));
            }
            report.push('\n');
        }

        let high_risk_files: Vec<_> = diff
            .files
            .iter()
            .filter(|file| file.risk_level >= RiskLevel::High)
            .collect();

        if !high_risk_files.is_empty() {
            report.push_str("## 高风险变更\n\n");
            for file in high_risk_files {
                report.push_str(&format!(
                    "- {} ({:?}): +{} -{} {}\n",
                    file.path, file.semantic, file.additions, file.deletions, file.risk_level
                ));
            }
            report.push('\n');
        }

        report
    }
}
