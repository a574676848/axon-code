//! LSP diagnostics 查询 runtime。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::{LspCatalogRuntime, LspClientPoolRuntime, LspDiagnosticRequest, LspLaunchPlan};

/// diagnostics 查询模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspDiagnosticsMode {
    /// `textDocument/diagnostic`
    Document,
    /// `workspace/diagnostic`
    Workspace,
}

/// diagnostics 查询入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDiagnosticsQuery {
    /// 查询模式。
    pub mode: LspDiagnosticsMode,
    /// 工作区根目录。
    pub workspace_root: String,
    /// 文档 URI；仅 document 模式需要。
    pub text_document_uri: Option<String>,
}

/// 单个 server 返回的 diagnostics 命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDiagnosticsHit {
    /// 命中对应的 server。
    pub server_id: String,
    /// 原始 JSON-RPC 响应。
    pub response: Value,
}

/// diagnostics 查询报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDiagnosticsReport {
    /// 当前工作区可直接启动的 server。
    pub launchable_servers: Vec<String>,
    /// 查询命中集合。
    pub hits: Vec<LspDiagnosticsHit>,
}

/// diagnostics 查询 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspDiagnosticsRuntime;

impl LspDiagnosticsRuntime {
    /// 自动解析工作区可启动计划并执行 diagnostics 查询。
    pub fn query(input: &LspDiagnosticsQuery) -> AxonResult<LspDiagnosticsReport> {
        let workspace_root = std::path::PathBuf::from(&input.workspace_root);
        if !workspace_root.is_dir() {
            return Err(AxonError::tool_invalid_definition(
                "LSP workspace root 不是目录",
            ));
        }
        let launch_plans = match input.mode {
            LspDiagnosticsMode::Document => {
                let uri = input.text_document_uri.as_deref().ok_or_else(|| {
                    AxonError::tool_invalid_definition("document diagnostics 缺少 textDocumentUri")
                })?;
                LspCatalogRuntime::resolve_document_launch_plans(&workspace_root, uri)?
            }
            LspDiagnosticsMode::Workspace => {
                LspCatalogRuntime::resolve_workspace_launch_plans(&workspace_root)
            }
        };
        Self::query_with_plans(input, launch_plans)
    }

    /// 使用显式计划执行 diagnostics 查询，便于测试或上层注入自定义 server。
    pub fn query_with_plans(
        input: &LspDiagnosticsQuery,
        launch_plans: Vec<LspLaunchPlan>,
    ) -> AxonResult<LspDiagnosticsReport> {
        if matches!(input.mode, LspDiagnosticsMode::Document) && input.text_document_uri.is_none() {
            return Err(AxonError::tool_invalid_definition(
                "document diagnostics 缺少 textDocumentUri",
            ));
        }
        let mut hits = Vec::new();
        let mut failures = Vec::new();
        for (index, plan) in launch_plans.iter().enumerate() {
            let response = LspClientPoolRuntime::with_client(plan, |client| match input.mode {
                LspDiagnosticsMode::Document => {
                    let text_document_uri =
                        input.text_document_uri.as_deref().ok_or_else(|| {
                            AxonError::tool_invalid_definition(
                                "document diagnostics 缺少 textDocumentUri",
                            )
                        })?;
                    client.touch_document(text_document_uri)?;
                    client.document_diagnostic(&LspDiagnosticRequest {
                        id: u64::try_from(index + 30).unwrap_or(30),
                        text_document_uri: text_document_uri.to_owned(),
                    })
                }
                LspDiagnosticsMode::Workspace => {
                    client.workspace_diagnostic(u64::try_from(index + 30).unwrap_or(30))
                }
            });
            match response {
                Ok(response) => hits.push(LspDiagnosticsHit {
                    server_id: plan.server_id.clone(),
                    response,
                }),
                Err(error) => failures.push(format!("{}: {error:?}", plan.server_id)),
            }
        }
        if hits.is_empty() && !failures.is_empty() {
            return Err(AxonError::tool_invalid_definition(format!(
                "LSP diagnostics 全部失败：{}",
                failures.join("; ")
            )));
        }
        Ok(LspDiagnosticsReport {
            launchable_servers: launch_plans
                .into_iter()
                .map(|plan| plan.server_id)
                .collect(),
            hits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_query_rejects_missing_document_uri() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-diagnostics-missing");
        std::fs::create_dir_all(&workspace).unwrap();
        let result = LspDiagnosticsRuntime::query_with_plans(
            &LspDiagnosticsQuery {
                mode: LspDiagnosticsMode::Document,
                workspace_root: workspace.display().to_string(),
                text_document_uri: None,
            },
            Vec::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn workspace_diagnostics_allows_missing_document_uri() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-workspace-diagnostics");
        std::fs::create_dir_all(&workspace).unwrap();
        let report = LspDiagnosticsRuntime::query(&LspDiagnosticsQuery {
            mode: LspDiagnosticsMode::Workspace,
            workspace_root: workspace.display().to_string(),
            text_document_uri: None,
        })
        .unwrap();
        assert!(report.hits.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn document_diagnostics_query_collects_response_from_fake_server() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-document-diagnostics");
        std::fs::create_dir_all(&workspace).unwrap();
        let document = workspace.join("main.rs");
        std::fs::write(&document, "fn main() {}\n").unwrap();
        let document_uri = crate::lsp_document::path_to_file_uri(&document).unwrap();
        let script = workspace.join("fake-diagnostics-lsp.ps1");
        std::fs::write(
            &script,
            r#"
$reader = [Console]::In
while (($line = $reader.ReadLine()) -ne $null) {
  if ($line -match 'Content-Length:\s*(\d+)') {
    $length = [int]$Matches[1]
    $null = $reader.ReadLine()
    $buffer = New-Object char[] $length
    $read = $reader.ReadBlock($buffer, 0, $length)
    $body = -join $buffer[0..($read-1)]
    $request = $body | ConvertFrom-Json
    if ($request.method -eq 'initialize') {
      $response = '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"diagnosticProvider":true}}}'
    } elseif ($request.method -eq 'textDocument/diagnostic') {
      $response = '{"jsonrpc":"2.0","id":30,"result":{"items":[{"message":"demo error"}]}}'
    } else {
      continue
    }
    $payload = "Content-Length: $($response.Length)`r`n`r`n$response"
    [Console]::Out.Write($payload)
    [Console]::Out.Flush()
  }
}
"#,
        )
        .unwrap();
        let plans = vec![LspLaunchPlan {
            server_id: "fake-diagnostics".to_owned(),
            program: "pwsh".to_owned(),
            args: vec![
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-File".to_owned(),
                script.display().to_string(),
            ],
            cwd: workspace.display().to_string(),
        }];
        let report = LspDiagnosticsRuntime::query_with_plans(
            &LspDiagnosticsQuery {
                mode: LspDiagnosticsMode::Document,
                workspace_root: workspace.display().to_string(),
                text_document_uri: Some(document_uri),
            },
            plans,
        )
        .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].server_id, "fake-diagnostics");
        assert_eq!(
            report.hits[0].response["result"]["items"][0]["message"],
            "demo error"
        );
    }
}
