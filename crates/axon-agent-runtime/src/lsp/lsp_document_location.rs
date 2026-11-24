//! 文档位置类 LSP 查询 runtime。

use crate::{LspCatalogRuntime, LspClientPoolRuntime, LspDocumentPositionRequest, LspLaunchPlan};
use axon_domain::{AxonError, AxonResult};
use axon_output_filter::{LspDocumentSymbolHit, LspDocumentSymbolProvider, LspDocumentSymbolQuery};
use serde_json::Value;

/// 文档位置查询方法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspDocumentLocationMethod {
    /// `textDocument/definition`
    Definition,
    /// `textDocument/references`
    References,
    /// `textDocument/hover`
    Hover,
    /// `textDocument/implementation`
    Implementation,
    /// `textDocument/prepareCallHierarchy`
    PrepareCallHierarchy,
    /// `callHierarchy/incomingCalls`
    IncomingCalls,
    /// `callHierarchy/outgoingCalls`
    OutgoingCalls,
    /// `textDocument/documentSymbol`
    DocumentSymbol,
}

/// 文档位置查询入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocumentLocationQuery {
    /// 查询方法。
    pub method: LspDocumentLocationMethod,
    /// 工作区根目录。
    pub workspace_root: String,
    /// 目标文档 URI。
    pub text_document_uri: String,
    /// 0-based 行号。
    pub line: u32,
    /// 0-based 列号。
    pub character: u32,
}

/// 单个 server 返回的文档位置命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocumentLocationHit {
    /// 命中对应的 server。
    pub server_id: String,
    /// 原始 JSON-RPC 响应。
    pub response: Value,
}

/// 文档位置查询报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocumentLocationReport {
    /// 当前工作区可直接启动的 server。
    pub launchable_servers: Vec<String>,
    /// 查询命中集合。
    pub hits: Vec<LspDocumentLocationHit>,
}

/// 文档位置查询 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspDocumentLocationRuntime;

impl LspDocumentLocationRuntime {
    /// 自动解析工作区可启动计划并执行文档位置查询。
    pub fn query(input: &LspDocumentLocationQuery) -> AxonResult<LspDocumentLocationReport> {
        let workspace_root = std::path::PathBuf::from(&input.workspace_root);
        if !workspace_root.is_dir() {
            return Err(AxonError::tool_invalid_definition(
                "LSP workspace root 不是目录",
            ));
        }
        let launch_plans = LspCatalogRuntime::resolve_document_launch_plans(
            &workspace_root,
            &input.text_document_uri,
        )?;
        Self::query_with_plans(input, launch_plans)
    }

    /// 使用显式计划执行文档位置查询，便于测试或上层注入自定义 server。
    pub fn query_with_plans(
        input: &LspDocumentLocationQuery,
        launch_plans: Vec<LspLaunchPlan>,
    ) -> AxonResult<LspDocumentLocationReport> {
        let mut hits = Vec::new();
        let mut failures = Vec::new();
        for (index, plan) in launch_plans.iter().enumerate() {
            let request = LspDocumentPositionRequest {
                id: u64::try_from(index + 20).unwrap_or(20),
                text_document_uri: input.text_document_uri.clone(),
                line: input.line,
                character: input.character,
            };
            let response = LspClientPoolRuntime::with_client(plan, |client| {
                client.touch_document(&input.text_document_uri)?;
                match input.method {
                    LspDocumentLocationMethod::Definition => client.definition(&request),
                    LspDocumentLocationMethod::References => client.references(&request),
                    LspDocumentLocationMethod::Hover => client.hover(&request),
                    LspDocumentLocationMethod::Implementation => client.implementation(&request),
                    LspDocumentLocationMethod::PrepareCallHierarchy => {
                        client.prepare_call_hierarchy(&request)
                    }
                    LspDocumentLocationMethod::IncomingCalls => {
                        let prepared = client.prepare_call_hierarchy(&request)?;
                        let item = prepared
                            .get("result")
                            .and_then(|result| result.get(0))
                            .cloned()
                            .ok_or(AxonError::tool_invalid_definition(
                                "LSP call hierarchy 未返回 item",
                            ))?;
                        client.incoming_calls(u64::try_from(index + 40).unwrap_or(40), &item)
                    }
                    LspDocumentLocationMethod::OutgoingCalls => {
                        let prepared = client.prepare_call_hierarchy(&request)?;
                        let item = prepared
                            .get("result")
                            .and_then(|result| result.get(0))
                            .cloned()
                            .ok_or(AxonError::tool_invalid_definition(
                                "LSP call hierarchy 未返回 item",
                            ))?;
                        client.outgoing_calls(u64::try_from(index + 50).unwrap_or(50), &item)
                    }
                    LspDocumentLocationMethod::DocumentSymbol => client.document_symbol(
                        u64::try_from(index + 60).unwrap_or(60),
                        &input.text_document_uri,
                    ),
                }
            });
            match response {
                Ok(response) => hits.push(LspDocumentLocationHit {
                    server_id: plan.server_id.clone(),
                    response,
                }),
                Err(error) => failures.push(format!("{}: {error:?}", plan.server_id)),
            }
        }
        if hits.is_empty() && !failures.is_empty() {
            return Err(AxonError::tool_invalid_definition(format!(
                "LSP 文档查询全部失败：{}",
                failures.join("; ")
            )));
        }
        Ok(LspDocumentLocationReport {
            launchable_servers: launch_plans
                .into_iter()
                .map(|plan| plan.server_id)
                .collect(),
            hits,
        })
    }
}

impl LspDocumentSymbolProvider for LspDocumentLocationRuntime {
    fn query_document_symbols(
        &self,
        query: &LspDocumentSymbolQuery,
    ) -> AxonResult<Vec<LspDocumentSymbolHit>> {
        let report = Self::query(&LspDocumentLocationQuery {
            method: LspDocumentLocationMethod::DocumentSymbol,
            workspace_root: query.workspace_root.clone(),
            text_document_uri: query.text_document_uri.clone(),
            line: 0,
            character: 0,
        })?;
        Ok(report
            .hits
            .into_iter()
            .map(|hit| LspDocumentSymbolHit {
                server_id: hit.server_id,
                response: hit.response,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_query_rejects_non_directory_workspace() {
        let file = axon_test_support::test_temp_root().join("axon-lsp-location-file.txt");
        std::fs::write(&file, "not a dir").unwrap();
        let result = LspDocumentLocationRuntime::query(&LspDocumentLocationQuery {
            method: LspDocumentLocationMethod::Definition,
            workspace_root: file.display().to_string(),
            text_document_uri: "file:///repo/main.rs".to_owned(),
            line: 0,
            character: 0,
        });
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn definition_query_collects_response_from_fake_server() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-definition-query");
        std::fs::create_dir_all(&workspace).unwrap();
        let document = workspace.join("main.rs");
        std::fs::write(&document, "fn main() {}\n").unwrap();
        let document_uri = crate::lsp_document::path_to_file_uri(&document).unwrap();
        let script = workspace.join("fake-definition-lsp.ps1");
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
      $response = '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"definitionProvider":true}}}'
    } elseif ($request.method -eq 'textDocument/definition') {
      $response = '{"jsonrpc":"2.0","id":20,"result":[{"uri":"file:///repo/lib.rs","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":6}}}]}'
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
            server_id: "fake-definition".to_owned(),
            program: "pwsh".to_owned(),
            args: vec![
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-File".to_owned(),
                script.display().to_string(),
            ],
            cwd: workspace.display().to_string(),
        }];
        let report = LspDocumentLocationRuntime::query_with_plans(
            &LspDocumentLocationQuery {
                method: LspDocumentLocationMethod::Definition,
                workspace_root: workspace.display().to_string(),
                text_document_uri: document_uri,
                line: 0,
                character: 0,
            },
            plans,
        )
        .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].server_id, "fake-definition");
        assert_eq!(
            report.hits[0].response["result"][0]["uri"],
            "file:///repo/lib.rs"
        );
    }

    #[cfg(windows)]
    #[test]
    fn references_query_collects_response_from_fake_server() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-references-query");
        std::fs::create_dir_all(&workspace).unwrap();
        let document = workspace.join("main.rs");
        std::fs::write(&document, "fn main() {}\n").unwrap();
        let document_uri = crate::lsp_document::path_to_file_uri(&document).unwrap();
        let script = workspace.join("fake-references-lsp.ps1");
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
      $response = '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"referencesProvider":true}}}'
    } elseif ($request.method -eq 'textDocument/references') {
      $response = '{"jsonrpc":"2.0","id":20,"result":[{"uri":"file:///repo/main.rs"},{"uri":"file:///repo/lib.rs"}]}'
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
            server_id: "fake-references".to_owned(),
            program: "pwsh".to_owned(),
            args: vec![
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-File".to_owned(),
                script.display().to_string(),
            ],
            cwd: workspace.display().to_string(),
        }];
        let report = LspDocumentLocationRuntime::query_with_plans(
            &LspDocumentLocationQuery {
                method: LspDocumentLocationMethod::References,
                workspace_root: workspace.display().to_string(),
                text_document_uri: document_uri,
                line: 0,
                character: 0,
            },
            plans,
        )
        .unwrap();
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].server_id, "fake-references");
        assert_eq!(
            report.hits[0].response["result"][1]["uri"],
            "file:///repo/lib.rs"
        );
    }
}
