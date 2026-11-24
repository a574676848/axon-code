//! LSP 诊断 JSON 投影。

use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::lsp_diagnostics_extended::{DiagnosticRequest, LspDiagnosticItem, RelatedInfo};

const LSP_DIAGNOSTIC_SEVERITY_ERROR: i64 = 1;
const LSP_DIAGNOSTIC_SEVERITY_WARNING: i64 = 2;
const LSP_DIAGNOSTIC_SEVERITY_INFO: i64 = 3;
const LSP_DIAGNOSTIC_SEVERITY_HINT: i64 = 4;

pub(crate) fn project_report_diagnostics(
    hits: &[crate::LspDiagnosticsHit],
    document_path: Option<&Path>,
    document_uri: Option<&str>,
    request: &DiagnosticRequest,
) -> Vec<LspDiagnosticItem> {
    hits.iter()
        .flat_map(|hit| diagnostic_values(&hit.response, document_uri))
        .filter_map(|(uri, item)| project_diagnostic(document_path, uri.as_deref(), &item))
        .filter(|item| include_diagnostic(item, request))
        .collect()
}

fn diagnostic_values(response: &Value, document_uri: Option<&str>) -> Vec<(Option<String>, Value)> {
    let Some(result) = response.get("result") else {
        return Vec::new();
    };
    let mut values = Vec::new();
    if let Some(items) = result.get("items").and_then(Value::as_array) {
        values.extend(items.iter().cloned().map(|item| {
            (
                item_uri(&item).or_else(|| document_uri.map(str::to_owned)),
                item,
            )
        }));
    }
    if let Some(reports) = result.get("items").and_then(Value::as_array) {
        for report in reports {
            let report_uri = report
                .get("uri")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| document_uri.map(str::to_owned));
            if let Some(items) = report.get("items").and_then(Value::as_array) {
                values.extend(
                    items
                        .iter()
                        .cloned()
                        .map(|item| (item_uri(&item).or_else(|| report_uri.clone()), item)),
                );
            }
        }
    }
    values
}

fn project_diagnostic(
    document_path: Option<&Path>,
    uri: Option<&str>,
    item: &Value,
) -> Option<LspDiagnosticItem> {
    let range = item.get("range")?;
    let start = range.get("start")?;
    let line = start.get("line").and_then(Value::as_u64)?.saturating_add(1);
    let column = start
        .get("character")
        .and_then(Value::as_u64)
        .map(|value| value.saturating_add(1));
    Some(LspDiagnosticItem {
        file_path: diagnostic_path(document_path, uri),
        line,
        column,
        severity: severity_label(item.get("severity").and_then(Value::as_i64)),
        code: diagnostic_code(item.get("code")),
        message: item.get("message").and_then(Value::as_str)?.to_owned(),
        source: item
            .get("source")
            .and_then(Value::as_str)
            .map(str::to_owned),
        related_info: related_information(item),
    })
}

fn related_information(item: &Value) -> Vec<RelatedInfo> {
    item.get("relatedInformation")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|related| {
            let location = related.get("location")?;
            let range = location.get("range")?;
            let start = range.get("start")?;
            Some(RelatedInfo {
                file_path: diagnostic_path(None, location.get("uri").and_then(Value::as_str)),
                line: start.get("line").and_then(Value::as_u64)?.saturating_add(1),
                column: start
                    .get("character")
                    .and_then(Value::as_u64)
                    .map(|value| value.saturating_add(1)),
                message: related.get("message").and_then(Value::as_str)?.to_owned(),
            })
        })
        .collect()
}

fn include_diagnostic(item: &LspDiagnosticItem, request: &DiagnosticRequest) -> bool {
    match item.severity.as_str() {
        "error" => true,
        "warning" => request.include_warnings,
        "info" | "hint" => request.include_hints,
        _ => request.include_hints,
    }
}

fn severity_label(severity: Option<i64>) -> String {
    match severity {
        Some(LSP_DIAGNOSTIC_SEVERITY_ERROR) | None => "error",
        Some(LSP_DIAGNOSTIC_SEVERITY_WARNING) => "warning",
        Some(LSP_DIAGNOSTIC_SEVERITY_INFO) => "info",
        Some(LSP_DIAGNOSTIC_SEVERITY_HINT) => "hint",
        Some(_) => "info",
    }
    .to_owned()
}

fn diagnostic_code(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Object(value) => value
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn diagnostic_path(document_path: Option<&Path>, uri: Option<&str>) -> PathBuf {
    uri.and_then(path_from_file_uri)
        .or_else(|| document_path.map(Path::to_path_buf))
        .unwrap_or_default()
}

fn item_uri(item: &Value) -> Option<String> {
    item.get("uri").and_then(Value::as_str).map(str::to_owned)
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    Some(PathBuf::from(
        uri.strip_prefix("file:///")?.replace('/', "\\"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn project_report_diagnostics_reads_lsp_document_items() {
        let response = json!({
            "result": {
                "items": [{
                    "uri": "file:///repo/src/main.rs",
                    "range": {"start": {"line": 9, "character": 4}},
                    "severity": 1,
                    "code": "E0425",
                    "source": "rustc",
                    "message": "cannot find value",
                    "relatedInformation": [{
                        "location": {
                            "uri": "file:///repo/src/lib.rs",
                            "range": {"start": {"line": 2, "character": 1}}
                        },
                        "message": "defined here"
                    }]
                }, {
                    "uri": "file:///repo/src/main.rs",
                    "range": {"start": {"line": 10, "character": 0}},
                    "severity": 4,
                    "message": "low value hint"
                }]
            }
        });
        let request = DiagnosticRequest::default();
        let diagnostics = project_report_diagnostics(
            &[crate::LspDiagnosticsHit {
                server_id: "rust-analyzer".to_owned(),
                response,
            }],
            None,
            None,
            &request,
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].file_path,
            PathBuf::from("repo\\src\\main.rs")
        );
        assert_eq!(diagnostics[0].line, 10);
        assert_eq!(diagnostics[0].column, Some(5));
        assert_eq!(diagnostics[0].severity, "error");
        assert_eq!(diagnostics[0].code.as_deref(), Some("E0425"));
        assert_eq!(diagnostics[0].related_info.len(), 1);
    }

    #[test]
    fn project_report_diagnostics_reads_workspace_reports() {
        let response = json!({
            "result": {
                "items": [{
                    "uri": "file:///repo/src/main.rs",
                    "items": [{
                        "range": {"start": {"line": 0, "character": 0}},
                        "severity": 2,
                        "message": "unused import"
                    }]
                }]
            }
        });
        let request = DiagnosticRequest {
            include_warnings: true,
            ..DiagnosticRequest::default()
        };
        let diagnostics = project_report_diagnostics(
            &[crate::LspDiagnosticsHit {
                server_id: "rust-analyzer".to_owned(),
                response,
            }],
            None,
            None,
            &request,
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, "warning");
        assert_eq!(diagnostics[0].line, 1);
    }
}
