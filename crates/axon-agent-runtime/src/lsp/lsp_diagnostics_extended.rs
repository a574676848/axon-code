//! LSP Diagnostics 扩展。
//!
//! 从 document diagnostics 扩展到 workspace/symbol 级诊断。
//! 支持按改动文件预热 LSP，并输出可定位的诊断结果。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::lsp_diagnostics_projection::project_report_diagnostics;
use crate::{LspDiagnosticsMode, LspDiagnosticsQuery, LspDiagnosticsRuntime};

/// LSP 诊断范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticScope {
    /// 单文档级别
    Document,
    /// Workspace 级别
    Workspace,
    /// Symbol 级别
    Symbol,
}

/// LSP 诊断请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRequest {
    /// 诊断范围
    pub scope: DiagnosticScope,
    /// 目标文件列表（用于预热）
    pub target_files: Vec<PathBuf>,
    /// 是否包含警告
    pub include_warnings: bool,
    /// 是否包含提示
    pub include_hints: bool,
}

impl Default for DiagnosticRequest {
    fn default() -> Self {
        Self {
            scope: DiagnosticScope::Document,
            target_files: Vec::new(),
            include_warnings: true,
            include_hints: false,
        }
    }
}

/// LSP 诊断项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticItem {
    /// 文件路径
    pub file_path: PathBuf,
    /// 行号（从 1 开始）
    pub line: u64,
    /// 列号（从 1 开始）
    pub column: Option<u64>,
    /// 严重性
    pub severity: String,
    /// 错误码
    pub code: Option<String>,
    /// 消息
    pub message: String,
    /// 来源（如 rustc、typescript）
    pub source: Option<String>,
    /// 相关信息
    pub related_info: Vec<RelatedInfo>,
}

/// 相关信息（如错误的引用位置）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedInfo {
    /// 文件路径
    pub file_path: PathBuf,
    /// 行号
    pub line: u64,
    /// 列号
    pub column: Option<u64>,
    /// 消息
    pub message: String,
}

/// LSP 诊断响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticResponse {
    /// 诊断项列表
    pub diagnostics: Vec<LspDiagnosticItem>,
    /// 错误数
    pub error_count: usize,
    /// 警告数
    pub warning_count: usize,
    /// 提示数
    pub hint_count: usize,
    /// 已预热的文件列表
    pub warmed_files: Vec<PathBuf>,
}

impl DiagnosticResponse {
    /// 创建空响应。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            diagnostics: Vec::new(),
            error_count: 0,
            warning_count: 0,
            hint_count: 0,
            warmed_files: Vec::new(),
        }
    }

    /// 从诊断项列表构建响应。
    #[must_use]
    pub fn from_items(items: Vec<LspDiagnosticItem>, warmed_files: Vec<PathBuf>) -> Self {
        let error_count = items.iter().filter(|d| d.severity == "error").count();
        let warning_count = items.iter().filter(|d| d.severity == "warning").count();
        let hint_count = items
            .iter()
            .filter(|d| d.severity == "hint" || d.severity == "info")
            .count();

        Self {
            diagnostics: items,
            error_count,
            warning_count,
            hint_count,
            warmed_files,
        }
    }

    /// 按文件分组。
    #[must_use]
    pub fn group_by_file(&self) -> HashMap<PathBuf, Vec<&LspDiagnosticItem>> {
        let mut grouped: HashMap<PathBuf, Vec<&LspDiagnosticItem>> = HashMap::new();

        for item in &self.diagnostics {
            grouped
                .entry(item.file_path.clone())
                .or_default()
                .push(item);
        }

        grouped
    }

    /// 生成人类可读摘要。
    #[must_use]
    pub fn to_summary(&self) -> String {
        format!(
            "LSP 诊断：{} 个错误，{} 个警告，{} 个提示（已预热 {} 个文件）",
            self.error_count,
            self.warning_count,
            self.hint_count,
            self.warmed_files.len()
        )
    }
}

/// LSP 诊断扩展运行时。
pub struct LspDiagnosticsExtended;

impl LspDiagnosticsExtended {
    /// 从改动文件列表提取 LSP 诊断。
    pub async fn extract_diagnostics(
        request: DiagnosticRequest,
    ) -> Result<DiagnosticResponse, String> {
        match request.scope {
            DiagnosticScope::Document => extract_document_diagnostics(&request),
            DiagnosticScope::Workspace => extract_workspace_diagnostics(&request),
            DiagnosticScope::Symbol => extract_symbol_diagnostics_from_request(&request),
        }
    }

    /// 预热 LSP 服务器（打开文档）。
    pub async fn warmup_lsp_servers(files: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        let request = DiagnosticRequest {
            scope: DiagnosticScope::Document,
            target_files: files.to_vec(),
            include_warnings: true,
            include_hints: true,
        };
        extract_document_diagnostics(&request).map(|response| response.warmed_files)
    }

    /// 从 workspace 级别拉取所有诊断。
    pub async fn extract_workspace_diagnostics() -> Result<DiagnosticResponse, String> {
        extract_workspace_diagnostics(&DiagnosticRequest {
            scope: DiagnosticScope::Workspace,
            ..DiagnosticRequest::default()
        })
    }

    /// 按符号位置拉取诊断。
    pub async fn extract_symbol_diagnostics(
        symbol_locations: &[(PathBuf, u64)],
    ) -> Result<DiagnosticResponse, String> {
        let request = DiagnosticRequest {
            scope: DiagnosticScope::Symbol,
            target_files: symbol_locations
                .iter()
                .map(|(path, _line)| path.clone())
                .collect(),
            include_warnings: true,
            include_hints: false,
        };
        let mut response = extract_symbol_diagnostics_from_request(&request)?;
        response.diagnostics.retain(|diagnostic| {
            symbol_locations.iter().any(|(path, line)| {
                diagnostic.file_path == *path && line_distance(diagnostic.line, *line) <= 1
            })
        });
        Ok(DiagnosticResponse::from_items(
            response.diagnostics,
            response.warmed_files,
        ))
    }
}

fn extract_document_diagnostics(request: &DiagnosticRequest) -> Result<DiagnosticResponse, String> {
    let mut diagnostics = Vec::new();
    let mut warmed_files = Vec::new();
    for file in request.target_files.iter().filter(|file| file.is_file()) {
        let workspace_root = workspace_root_for(file)?;
        let file_uri = path_to_file_uri(file);
        let query = LspDiagnosticsQuery {
            mode: LspDiagnosticsMode::Document,
            workspace_root: workspace_root.display().to_string(),
            text_document_uri: Some(file_uri.clone()),
        };
        let report = LspDiagnosticsRuntime::query(&query).map_err(|error| format!("{error:?}"))?;
        warmed_files.push(file.clone());
        diagnostics.extend(project_report_diagnostics(
            &report.hits,
            Some(file),
            Some(&file_uri),
            request,
        ));
    }
    Ok(DiagnosticResponse::from_items(diagnostics, warmed_files))
}

fn extract_workspace_diagnostics(
    request: &DiagnosticRequest,
) -> Result<DiagnosticResponse, String> {
    let workspace_root = workspace_root_from_request(request)?;
    let query = LspDiagnosticsQuery {
        mode: LspDiagnosticsMode::Workspace,
        workspace_root: workspace_root.display().to_string(),
        text_document_uri: None,
    };
    let report = LspDiagnosticsRuntime::query(&query).map_err(|error| format!("{error:?}"))?;
    let diagnostics = project_report_diagnostics(&report.hits, None, None, request);
    Ok(DiagnosticResponse::from_items(
        diagnostics,
        request.target_files.clone(),
    ))
}

fn extract_symbol_diagnostics_from_request(
    request: &DiagnosticRequest,
) -> Result<DiagnosticResponse, String> {
    let document_request = DiagnosticRequest {
        scope: DiagnosticScope::Document,
        target_files: request.target_files.clone(),
        include_warnings: request.include_warnings,
        include_hints: request.include_hints,
    };
    extract_document_diagnostics(&document_request)
}

fn workspace_root_for(file: &Path) -> Result<PathBuf, String> {
    file.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("无法解析 LSP workspace root：{}", file.display()))
}

fn workspace_root_from_request(request: &DiagnosticRequest) -> Result<PathBuf, String> {
    request
        .target_files
        .first()
        .map(|path| {
            if path.is_dir() {
                path.clone()
            } else {
                path.parent()
                    .map_or_else(|| path.clone(), Path::to_path_buf)
            }
        })
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "无法解析 LSP workspace root".to_owned())
}

fn path_to_file_uri(path: &Path) -> String {
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}

fn line_distance(left: u64, right: u64) -> u64 {
    left.max(right) - left.min(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostic_response_from_items() {
        let items = vec![
            LspDiagnosticItem {
                file_path: PathBuf::from("src/main.rs"),
                line: 10,
                column: Some(5),
                severity: "error".to_owned(),
                code: Some("E0425".to_owned()),
                message: "cannot find value".to_owned(),
                source: Some("rustc".to_owned()),
                related_info: Vec::new(),
            },
            LspDiagnosticItem {
                file_path: PathBuf::from("src/lib.rs"),
                line: 20,
                column: Some(10),
                severity: "warning".to_owned(),
                code: None,
                message: "unused variable".to_owned(),
                source: Some("rustc".to_owned()),
                related_info: Vec::new(),
            },
        ];

        let response = DiagnosticResponse::from_items(items, vec![PathBuf::from("src/main.rs")]);

        assert_eq!(response.error_count, 1);
        assert_eq!(response.warning_count, 1);
        assert_eq!(response.diagnostics.len(), 2);
        assert_eq!(response.warmed_files.len(), 1);
    }

    #[test]
    fn test_diagnostic_response_group_by_file() {
        let items = vec![
            LspDiagnosticItem {
                file_path: PathBuf::from("src/main.rs"),
                line: 10,
                column: Some(5),
                severity: "error".to_owned(),
                code: Some("E0425".to_owned()),
                message: "error 1".to_owned(),
                source: None,
                related_info: Vec::new(),
            },
            LspDiagnosticItem {
                file_path: PathBuf::from("src/main.rs"),
                line: 20,
                column: Some(10),
                severity: "error".to_owned(),
                code: Some("E0308".to_owned()),
                message: "error 2".to_owned(),
                source: None,
                related_info: Vec::new(),
            },
        ];

        let response = DiagnosticResponse::from_items(items, Vec::new());
        let grouped = response.group_by_file();

        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[&PathBuf::from("src/main.rs")].len(), 2);
    }
}
