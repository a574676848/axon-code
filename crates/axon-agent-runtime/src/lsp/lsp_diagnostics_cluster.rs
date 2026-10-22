//! LSP 诊断聚类与注入 (AXON-WF-007)。
//!
//! 升级 LSP Diagnostics 从"读取诊断"到"按文件/范围/严重级别聚类"，
//! 并能精准注入下一轮修复提示，对齐 Codex diagnostics clustering。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::lsp_diagnostics_extended::{DiagnosticResponse, LspDiagnosticItem};

/// 诊断聚类键（用于分组）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum ClusterKey {
    /// 按文件聚类。
    ByFile(PathBuf),
    /// 按严重性聚类。
    BySeverity(String),
    /// 按错误码聚类。
    ByCode(String),
    /// 按来源聚类（如 rustc、typescript）。
    BySource(String),
    /// 按文件和严重性聚类。
    ByFileSeverity(PathBuf, String),
}

/// 诊断聚类（一组相关的诊断项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticCluster {
    /// 聚类键。
    pub key: ClusterKey,
    /// 聚类内的诊断项。
    pub items: Vec<LspDiagnosticItem>,
    /// 错误数。
    pub error_count: usize,
    /// 警告数。
    pub warning_count: usize,
    /// 聚类摘要。
    pub summary: String,
}

impl DiagnosticCluster {
    /// 创建新聚类。
    #[must_use]
    pub fn new(key: ClusterKey) -> Self {
        Self {
            key,
            items: Vec::new(),
            error_count: 0,
            warning_count: 0,
            summary: String::new(),
        }
    }

    /// 添加诊断项。
    pub fn add_item(&mut self, item: LspDiagnosticItem) {
        if item.severity == "error" {
            self.error_count += 1;
        } else if item.severity == "warning" {
            self.warning_count += 1;
        }
        self.items.push(item);
    }

    /// 生成聚类摘要。
    pub fn generate_summary(&mut self) {
        self.summary = match &self.key {
            ClusterKey::ByFile(path) => {
                format!(
                    "{}: {} 个错误，{} 个警告",
                    path.display(),
                    self.error_count,
                    self.warning_count
                )
            }
            ClusterKey::BySeverity(severity) => {
                format!("{severity} 级别：{} 个", self.items.len())
            }
            ClusterKey::ByCode(code) => {
                format!("错误码 {code}：{} 个", self.items.len())
            }
            ClusterKey::BySource(source) => {
                format!("{source} 诊断：{} 个", self.items.len())
            }
            ClusterKey::ByFileSeverity(path, severity) => {
                format!("{} [{}]：{} 个", path.display(), severity, self.items.len())
            }
        };
    }

    /// 转换为修复提示文本。
    #[must_use]
    pub fn to_fix_prompt(&self) -> String {
        let mut prompt = format!("## {}\n\n", self.summary);

        for (idx, item) in self.items.iter().enumerate() {
            prompt.push_str(&format!(
                "{}. [{}:{}] {}: {}\n",
                idx + 1,
                item.file_path.display(),
                item.line,
                item.severity,
                item.message
            ));

            if let Some(code) = &item.code {
                prompt.push_str(&format!("   错误码: {code}\n"));
            }

            if !item.related_info.is_empty() {
                prompt.push_str("   相关位置:\n");
                for rel in &item.related_info {
                    prompt.push_str(&format!(
                        "     - {}:{} - {}\n",
                        rel.file_path.display(),
                        rel.line,
                        rel.message
                    ));
                }
            }
        }

        prompt
    }
}

/// 诊断聚类策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ClusterStrategy {
    /// 按文件聚类。
    #[default]
    ByFile,
    /// 按严重性聚类。
    BySeverity,
    /// 按错误码聚类。
    ByCode,
    /// 按来源聚类。
    BySource,
    /// 按文件和严重性聚类。
    ByFileSeverity,
}

/// 诊断聚类结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClusteredDiagnostics {
    /// 聚类策略。
    pub strategy: ClusterStrategy,
    /// 聚类列表。
    pub clusters: Vec<DiagnosticCluster>,
    /// 总错误数。
    pub total_errors: usize,
    /// 总警告数。
    pub total_warnings: usize,
}

impl ClusteredDiagnostics {
    /// 从诊断响应创建聚类。
    #[must_use]
    pub fn from_response(response: &DiagnosticResponse, strategy: ClusterStrategy) -> Self {
        let mut clusters_map: BTreeMap<ClusterKey, DiagnosticCluster> = BTreeMap::new();

        for item in &response.diagnostics {
            let key = match strategy {
                ClusterStrategy::ByFile => ClusterKey::ByFile(item.file_path.clone()),
                ClusterStrategy::BySeverity => ClusterKey::BySeverity(item.severity.clone()),
                ClusterStrategy::ByCode => {
                    ClusterKey::ByCode(item.code.clone().unwrap_or_else(|| "unknown".to_owned()))
                }
                ClusterStrategy::BySource => ClusterKey::BySource(
                    item.source.clone().unwrap_or_else(|| "unknown".to_owned()),
                ),
                ClusterStrategy::ByFileSeverity => {
                    ClusterKey::ByFileSeverity(item.file_path.clone(), item.severity.clone())
                }
            };

            clusters_map
                .entry(key.clone())
                .or_insert_with(|| DiagnosticCluster::new(key))
                .add_item(item.clone());
        }

        let mut clusters: Vec<DiagnosticCluster> = clusters_map.into_values().collect();
        for cluster in &mut clusters {
            cluster.generate_summary();
        }

        // 按错误数降序排序
        clusters.sort_by_key(|cluster| std::cmp::Reverse(cluster.error_count));

        Self {
            strategy,
            clusters,
            total_errors: response.error_count,
            total_warnings: response.warning_count,
        }
    }

    /// 生成全局修复提示。
    #[must_use]
    pub fn to_global_fix_prompt(&self) -> String {
        let mut prompt = format!(
            "# LSP 诊断报告\n\n总计：{} 个错误，{} 个警告\n\n",
            self.total_errors, self.total_warnings
        );

        for (idx, cluster) in self.clusters.iter().enumerate() {
            if idx > 0 {
                prompt.push_str("\n---\n\n");
            }
            prompt.push_str(&cluster.to_fix_prompt());
        }

        prompt
    }

    /// 获取优先级最高的聚类（错误数最多）。
    #[must_use]
    pub fn get_top_priority_cluster(&self) -> Option<&DiagnosticCluster> {
        self.clusters.first()
    }

    /// 按文件过滤聚类。
    #[must_use]
    pub fn filter_by_files(&self, files: &[PathBuf]) -> Self {
        let filtered_clusters: Vec<DiagnosticCluster> = self
            .clusters
            .iter()
            .filter(|cluster| match &cluster.key {
                ClusterKey::ByFile(path) | ClusterKey::ByFileSeverity(path, _) => {
                    files.contains(path)
                }
                _ => true,
            })
            .cloned()
            .collect();

        let total_errors = filtered_clusters.iter().map(|c| c.error_count).sum();
        let total_warnings = filtered_clusters.iter().map(|c| c.warning_count).sum();

        Self {
            strategy: self.strategy,
            clusters: filtered_clusters,
            total_errors,
            total_warnings,
        }
    }
}

/// 诊断注入器（将诊断转换为下一轮 prompt）。
pub struct DiagnosticInjector;

impl DiagnosticInjector {
    /// 注入聚类诊断到修复提示。
    #[must_use]
    pub fn inject_clustered(clustered: &ClusteredDiagnostics) -> String {
        let mut prompt = String::from("以下是 LSP 诊断的聚类结果，请根据这些信息修复代码：\n\n");
        prompt.push_str(&clustered.to_global_fix_prompt());
        prompt.push_str("\n请优先修复错误数最多的聚类。\n");
        prompt
    }

    /// 注入单个聚类诊断。
    #[must_use]
    pub fn inject_single_cluster(cluster: &DiagnosticCluster) -> String {
        let mut prompt = String::from("请修复以下诊断问题：\n\n");
        prompt.push_str(&cluster.to_fix_prompt());
        prompt
    }

    /// 注入最高优先级聚类。
    #[must_use]
    pub fn inject_top_priority(clustered: &ClusteredDiagnostics) -> String {
        if let Some(top) = clustered.get_top_priority_cluster() {
            let mut prompt = String::from("最高优先级问题（错误数最多）：\n\n");
            prompt.push_str(&top.to_fix_prompt());
            prompt.push_str("\n请先修复这个聚类中的问题。\n");
            prompt
        } else {
            String::from("没有诊断问题。\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_diagnostics_extended::RelatedInfo;

    fn create_test_item(
        path: &str,
        line: u64,
        severity: &str,
        code: Option<&str>,
    ) -> LspDiagnosticItem {
        LspDiagnosticItem {
            file_path: PathBuf::from(path),
            line,
            column: Some(5),
            severity: severity.to_owned(),
            code: code.map(String::from),
            message: format!("{severity} message"),
            source: Some("test".to_owned()),
            related_info: Vec::new(),
        }
    }

    #[test]
    fn cluster_by_file() {
        let items = vec![
            create_test_item("src/main.rs", 10, "error", Some("E001")),
            create_test_item("src/main.rs", 20, "warning", Some("W001")),
            create_test_item("src/lib.rs", 30, "error", Some("E002")),
        ];

        let response = DiagnosticResponse::from_items(items, Vec::new());
        let clustered = ClusteredDiagnostics::from_response(&response, ClusterStrategy::ByFile);

        assert_eq!(clustered.clusters.len(), 2);
        assert_eq!(clustered.total_errors, 2);
        assert_eq!(clustered.total_warnings, 1);
    }

    #[test]
    fn cluster_by_severity() {
        let items = vec![
            create_test_item("src/main.rs", 10, "error", None),
            create_test_item("src/main.rs", 20, "error", None),
            create_test_item("src/lib.rs", 30, "warning", None),
        ];

        let response = DiagnosticResponse::from_items(items, Vec::new());
        let clustered = ClusteredDiagnostics::from_response(&response, ClusterStrategy::BySeverity);

        assert_eq!(clustered.clusters.len(), 2);

        let error_cluster = clustered
            .clusters
            .iter()
            .find(|c| matches!(&c.key, ClusterKey::BySeverity(s) if s == "error"))
            .unwrap();
        assert_eq!(error_cluster.items.len(), 2);
    }

    #[test]
    fn cluster_top_priority() {
        let items = vec![
            create_test_item("src/main.rs", 10, "error", None),
            create_test_item("src/main.rs", 20, "error", None),
            create_test_item("src/lib.rs", 30, "error", None),
        ];

        let response = DiagnosticResponse::from_items(items, Vec::new());
        let clustered = ClusteredDiagnostics::from_response(&response, ClusterStrategy::ByFile);

        let top = clustered.get_top_priority_cluster().unwrap();
        assert_eq!(top.error_count, 2);
    }

    #[test]
    fn cluster_filter_by_files() {
        let items = vec![
            create_test_item("src/main.rs", 10, "error", None),
            create_test_item("src/lib.rs", 20, "error", None),
        ];

        let response = DiagnosticResponse::from_items(items, Vec::new());
        let clustered = ClusteredDiagnostics::from_response(&response, ClusterStrategy::ByFile);

        let filtered = clustered.filter_by_files(&[PathBuf::from("src/main.rs")]);

        assert_eq!(filtered.clusters.len(), 1);
        assert_eq!(filtered.total_errors, 1);
    }

    #[test]
    fn cluster_to_fix_prompt_includes_related_info() {
        let mut item = create_test_item("src/main.rs", 10, "error", Some("E001"));
        item.related_info.push(RelatedInfo {
            file_path: PathBuf::from("src/lib.rs"),
            line: 20,
            column: Some(5),
            message: "defined here".to_owned(),
        });

        let response = DiagnosticResponse::from_items(vec![item], Vec::new());
        let clustered = ClusteredDiagnostics::from_response(&response, ClusterStrategy::ByFile);

        let prompt = DiagnosticInjector::inject_clustered(&clustered);
        assert!(prompt.contains("LSP 诊断的聚类结果"));
        assert!(prompt.contains("src/main.rs"));

        let prompt = clustered.clusters[0].to_fix_prompt();

        assert!(prompt.contains("相关位置"));
        assert!(prompt.contains("src/lib.rs:20"));
        assert!(prompt.contains("defined here"));
    }
}
