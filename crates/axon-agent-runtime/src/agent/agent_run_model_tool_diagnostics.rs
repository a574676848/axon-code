//! apply_patch 执行后的 LSP diagnostics 投影。

use std::collections::BTreeMap;

pub(crate) fn apply_patch_diagnostics_summary(lsp: &serde_json::Value) -> serde_json::Value {
    let diagnostics = lsp
        .get("documents")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|document| {
            let path = document
                .get("workspacePath")
                .or_else(|| document.get("filePath"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            document
                .get("diagnostics")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .map(move |diagnostic| {
                    serde_json::json!({
                        "path": path,
                        "severity": diagnostic.get("severity").cloned().unwrap_or(serde_json::Value::Null),
                        "source": diagnostic.get("source").cloned().unwrap_or(serde_json::Value::Null),
                        "code": diagnostic.get("code").cloned().unwrap_or(serde_json::Value::Null),
                        "message": diagnostic.get("message").cloned().unwrap_or(serde_json::Value::Null),
                        "range": diagnostic.get("range").cloned().unwrap_or(serde_json::Value::Null),
                    })
                })
        })
        .collect::<Vec<_>>();
    serde_json::json!(diagnostics)
}

pub(crate) fn apply_patch_diagnostics_clusters(
    diagnostics_summary: &serde_json::Value,
) -> serde_json::Value {
    let mut clusters = BTreeMap::<String, DiagnosticCluster>::new();
    let mut total = 0usize;
    for diagnostic in diagnostics_summary.as_array().into_iter().flatten() {
        total = total.saturating_add(1);
        let cluster_key = diagnostic_cluster_key(diagnostic);
        clusters
            .entry(cluster_key)
            .and_modify(|cluster| cluster.count = cluster.count.saturating_add(1))
            .or_insert_with(|| DiagnosticCluster::from_diagnostic(diagnostic));
    }
    let mut clusters = clusters.into_values().collect::<Vec<_>>();
    clusters.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.source.cmp(&right.source))
            .then(left.code.cmp(&right.code))
    });
    let original_cluster_count = clusters.len();
    let output = clusters
        .into_iter()
        .map(|cluster| {
            serde_json::json!({
                "path": cluster.path,
                "source": cluster.source,
                "code": cluster.code,
                "severity": cluster.severity,
                "count": cluster.count,
                "representative": cluster.representative,
                "fixHint": cluster.fix_hint,
                "truncation": {
                    "originalCount": total,
                    "retainedClusters": original_cluster_count,
                    "truncatedClusters": 0,
                }
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!(output)
}

pub(crate) fn post_apply_diagnostics_errors(
    diagnostics_clusters: &serde_json::Value,
) -> serde_json::Value {
    let errors = diagnostics_clusters
        .as_array()
        .into_iter()
        .flatten()
        .filter(|diagnostic| {
            diagnostic
                .get("severity")
                .and_then(serde_json::Value::as_i64)
                .is_none_or(|severity| severity == 1)
        })
        .cloned()
        .collect::<Vec<_>>();
    serde_json::json!(errors)
}

fn diagnostic_cluster_key(diagnostic: &serde_json::Value) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        json_string(diagnostic, "path"),
        json_string(diagnostic, "source"),
        json_string(diagnostic, "code")
    )
}

fn json_string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiagnosticCluster {
    path: String,
    source: String,
    code: String,
    severity: serde_json::Value,
    count: usize,
    representative: serde_json::Value,
    fix_hint: String,
}

impl DiagnosticCluster {
    fn from_diagnostic(diagnostic: &serde_json::Value) -> Self {
        let source = json_string(diagnostic, "source");
        let code = json_string(diagnostic, "code");
        Self {
            path: json_string(diagnostic, "path"),
            source: source.clone(),
            code: code.clone(),
            severity: diagnostic
                .get("severity")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            count: 1,
            representative: diagnostic.clone(),
            fix_hint: diagnostic_fix_hint(&source, &code),
        }
    }
}

fn diagnostic_fix_hint(source: &str, code: &str) -> String {
    match (source, code) {
        ("rust-analyzer", "E0425") => {
            "检查名称是否拼写正确，或补充对应作用域内的定义/import。".to_owned()
        }
        ("rust-analyzer", _) => {
            "优先按 rust-analyzer 诊断位置修复类型、名称或借用关系。".to_owned()
        }
        ("typescript" | "tsserver", _) => {
            "优先检查 TypeScript 类型、导入路径和声明是否与调用点一致。".to_owned()
        }
        ("pyright", _) => "优先检查 Python 类型标注、导入和可选值处理。".to_owned(),
        _ => "优先修复代表项所在文件，再复查同类 diagnostics 是否一起收敛。".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::apply_patch_diagnostics_clusters;
    use serde_json::json;

    #[test]
    fn diagnostics_clusters_preserve_more_than_five_groups() {
        let diagnostics = json!(
            (0..7)
                .map(|index| {
                    json!({
                        "path": format!("src/file_{index}.rs"),
                        "source": "rust-analyzer",
                        "code": format!("E{index:04}"),
                        "severity": 1,
                        "message": format!("diagnostic {index}"),
                        "range": null
                    })
                })
                .collect::<Vec<_>>()
        );

        let clusters = apply_patch_diagnostics_clusters(&diagnostics);
        let clusters = clusters.as_array().expect("diagnostics clusters");

        assert_eq!(clusters.len(), 7);
        assert_eq!(clusters[0]["truncation"]["retainedClusters"], 7);
        assert_eq!(clusters[0]["truncation"]["truncatedClusters"], 0);
    }
}
