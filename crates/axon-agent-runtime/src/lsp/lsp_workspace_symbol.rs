//! 工作区级 `workspace/symbol` 查询 runtime。

use axon_domain::{AxonError, AxonResult};
use futures_util::future::join_all;
use serde_json::Value;

use crate::LspLaunchPlan;
use crate::lsp_document::file_path_from_uri;
use crate::{
    LspCatalogRuntime, LspClientPoolRuntime, LspWorkspaceCoverage, LspWorkspaceSymbolRequest,
};

/// 工作区级 `workspace/symbol` 查询入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspWorkspaceSymbolQuery {
    /// 查询关键字。
    pub query: String,
    /// 工作区根目录。
    pub workspace_root: String,
    /// 用于选择语言服务器和项目根的目标文档 URI。
    pub text_document_uri: Option<String>,
}

/// 单个 server 返回的 workspace symbol 命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspWorkspaceSymbolHit {
    /// 返回结果对应的 server。
    pub server_id: String,
    /// 原始 JSON-RPC 响应。
    pub response: Value,
}

/// workspace symbol 查询报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspWorkspaceSymbolReport {
    /// 工作区命中覆盖情况。
    pub coverage: LspWorkspaceCoverage,
    /// 当前可直接启动的 server。
    pub launchable_servers: Vec<String>,
    /// 实际查询到的命中结果。
    pub hits: Vec<LspWorkspaceSymbolHit>,
}

/// workspace symbol 查询 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspWorkspaceSymbolRuntime;

impl LspWorkspaceSymbolRuntime {
    /// 只生成当前工作区的查询计划，不启动任何 LSP 进程。
    pub fn plan_only(input: &LspWorkspaceSymbolQuery) -> AxonResult<LspWorkspaceSymbolReport> {
        let workspace_root = std::path::PathBuf::from(&input.workspace_root);
        if !workspace_root.is_dir() {
            return Err(AxonError::tool_invalid_definition(
                "LSP workspace root 不是目录",
            ));
        }
        let coverage = resolve_coverage(&workspace_root, input)?;
        let launch_plans = resolve_launch_plans(&workspace_root, input)?;
        Ok(LspWorkspaceSymbolReport {
            coverage,
            launchable_servers: launch_plans
                .into_iter()
                .map(|plan| plan.server_id)
                .collect(),
            hits: Vec::new(),
        })
    }

    /// 启动当前工作区可用的 LSP 并执行 `workspace/symbol` 查询。
    pub fn query(input: &LspWorkspaceSymbolQuery) -> AxonResult<LspWorkspaceSymbolReport> {
        let workspace_root = std::path::PathBuf::from(&input.workspace_root);
        if !workspace_root.is_dir() {
            return Err(AxonError::tool_invalid_definition(
                "LSP workspace root 不是目录",
            ));
        }
        let coverage = resolve_coverage(&workspace_root, input)?;
        let launch_plans = resolve_launch_plans(&workspace_root, input)?;
        Self::query_with_plans(input, coverage, launch_plans)
    }

    /// 异步启动当前工作区可用的 LSP 并执行 `workspace/symbol` 查询。
    pub async fn query_async(
        input: &LspWorkspaceSymbolQuery,
    ) -> AxonResult<LspWorkspaceSymbolReport> {
        let workspace_root = std::path::PathBuf::from(&input.workspace_root);
        if !workspace_root.is_dir() {
            return Err(AxonError::tool_invalid_definition(
                "LSP workspace root 不是目录",
            ));
        }
        let coverage = resolve_coverage(&workspace_root, input)?;
        let launch_plans = resolve_launch_plans(&workspace_root, input)?;
        Self::query_with_plans_async(input, coverage, launch_plans).await
    }

    /// 使用显式计划执行 `workspace/symbol` 查询，便于测试或上层注入自定义 server。
    pub fn query_with_plans(
        input: &LspWorkspaceSymbolQuery,
        coverage: LspWorkspaceCoverage,
        launch_plans: Vec<LspLaunchPlan>,
    ) -> AxonResult<LspWorkspaceSymbolReport> {
        let mut hits = Vec::new();
        let mut failures = Vec::new();
        for (index, plan) in launch_plans.iter().enumerate() {
            match query_server(
                index,
                plan,
                input.query.clone(),
                input.text_document_uri.as_deref(),
            ) {
                Ok(hit) => hits.push(hit),
                Err(error) => failures.push(format!("{}: {error:?}", plan.server_id)),
            }
        }
        if hits.is_empty() && !failures.is_empty() {
            return Err(AxonError::tool_invalid_definition(format!(
                "LSP workspaceSymbol 全部失败：{}",
                failures.join("; ")
            )));
        }
        Ok(LspWorkspaceSymbolReport {
            coverage,
            launchable_servers: launch_plans
                .into_iter()
                .map(|plan| plan.server_id)
                .collect(),
            hits,
        })
    }

    /// 使用显式计划异步执行 `workspace/symbol` 查询，便于并发查询多个 server。
    pub async fn query_with_plans_async(
        input: &LspWorkspaceSymbolQuery,
        coverage: LspWorkspaceCoverage,
        launch_plans: Vec<LspLaunchPlan>,
    ) -> AxonResult<LspWorkspaceSymbolReport> {
        let tasks = launch_plans
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, plan)| {
                let query = input.query.clone();
                let text_document_uri = input.text_document_uri.clone();
                tokio::task::spawn_blocking(move || {
                    let server_id = plan.server_id.clone();
                    (
                        server_id,
                        query_server(index, &plan, query, text_document_uri.as_deref()),
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut hits = Vec::new();
        let mut failures = Vec::new();
        for task in join_all(tasks).await {
            match task {
                Ok((_, Ok(hit))) => hits.push(hit),
                Ok((server_id, Err(error))) => {
                    failures.push(format!("{server_id}: {error:?}"));
                }
                Err(error) => failures.push(format!("worker: {error}")),
            }
        }
        if hits.is_empty() && !failures.is_empty() {
            return Err(AxonError::tool_invalid_definition(format!(
                "LSP workspaceSymbol 全部失败：{}",
                failures.join("; ")
            )));
        }
        Ok(LspWorkspaceSymbolReport {
            coverage,
            launchable_servers: launch_plans
                .into_iter()
                .map(|plan| plan.server_id)
                .collect(),
            hits,
        })
    }
}

fn query_server(
    index: usize,
    plan: &LspLaunchPlan,
    query: String,
    text_document_uri: Option<&str>,
) -> AxonResult<LspWorkspaceSymbolHit> {
    let response = LspClientPoolRuntime::with_client(plan, |client| {
        if let Some(uri) = text_document_uri {
            client.touch_document(uri)?;
        }
        client.workspace_symbol(&LspWorkspaceSymbolRequest {
            id: u64::try_from(index + 10).unwrap_or(10),
            query,
        })
    })?;
    Ok(LspWorkspaceSymbolHit {
        server_id: plan.server_id.clone(),
        response,
    })
}

fn resolve_coverage(
    workspace_root: &std::path::Path,
    input: &LspWorkspaceSymbolQuery,
) -> AxonResult<LspWorkspaceCoverage> {
    let Some(uri) = input.text_document_uri.as_deref() else {
        return Ok(LspCatalogRuntime::resolve_workspace_coverage(
            workspace_root,
        ));
    };
    let path = file_path_from_uri(uri)?;
    let file_match = LspCatalogRuntime::resolve_workspace_match(workspace_root, &path);
    let matched_servers = file_match
        .as_ref()
        .map_or_else(Vec::new, |item| item.matched_servers.clone());
    let filtered_servers = file_match
        .as_ref()
        .map_or_else(Vec::new, |item| item.filtered_by_root_markers.clone());
    Ok(LspWorkspaceCoverage {
        workspace_root: workspace_root.display().to_string(),
        scanned_files: usize::from(file_match.is_some()),
        truncated: false,
        matched_servers,
        filtered_servers,
        file_matches: file_match.into_iter().collect(),
    })
}

fn resolve_launch_plans(
    workspace_root: &std::path::Path,
    input: &LspWorkspaceSymbolQuery,
) -> AxonResult<Vec<LspLaunchPlan>> {
    input.text_document_uri.as_deref().map_or_else(
        || {
            Ok(LspCatalogRuntime::resolve_workspace_launch_plans(
                workspace_root,
            ))
        },
        |uri| LspCatalogRuntime::resolve_document_launch_plans(workspace_root, uri),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_symbol_plan_reports_coverage_without_querying_servers() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-workspace-symbol");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        std::fs::write(workspace.join("lib.rs"), "pub fn router() {}\n").unwrap();
        LspCatalogRuntime::invalidate_workspace_coverage(&workspace);

        let report = LspWorkspaceSymbolRuntime::plan_only(&LspWorkspaceSymbolQuery {
            query: "router".to_owned(),
            workspace_root: workspace.display().to_string(),
            text_document_uri: None,
        })
        .unwrap();

        assert!(report.hits.is_empty());
        assert!(
            report
                .coverage
                .matched_servers
                .contains(&"rust-analyzer".to_owned())
        );
    }

    #[test]
    fn workspace_symbol_plan_rejects_non_directory_workspace() {
        let file = axon_test_support::test_temp_root().join("axon-lsp-workspace-symbol-file.txt");
        std::fs::write(&file, "not a dir").unwrap();
        let result = LspWorkspaceSymbolRuntime::plan_only(&LspWorkspaceSymbolQuery {
            query: "router".to_owned(),
            workspace_root: file.display().to_string(),
            text_document_uri: None,
        });
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn workspace_symbol_async_query_accepts_explicit_empty_plans() {
        let workspace =
            axon_test_support::test_temp_root().join("axon-lsp-workspace-symbol-async-empty");
        std::fs::create_dir_all(&workspace).unwrap();
        let coverage = LspWorkspaceCoverage {
            workspace_root: workspace.display().to_string(),
            scanned_files: 0,
            truncated: false,
            matched_servers: Vec::new(),
            filtered_servers: Vec::new(),
            file_matches: Vec::new(),
        };

        let report = LspWorkspaceSymbolRuntime::query_with_plans_async(
            &LspWorkspaceSymbolQuery {
                query: "router".to_owned(),
                workspace_root: workspace.display().to_string(),
                text_document_uri: None,
            },
            coverage,
            Vec::new(),
        )
        .await
        .unwrap();

        assert!(report.hits.is_empty());
        assert!(report.launchable_servers.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn workspace_symbol_query_collects_response_from_fake_server() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-workspace-symbol-query");
        std::fs::create_dir_all(&workspace).unwrap();
        let query = LspWorkspaceSymbolQuery {
            query: "router".to_owned(),
            workspace_root: workspace.display().to_string(),
            text_document_uri: None,
        };
        let coverage = LspWorkspaceCoverage {
            workspace_root: workspace.display().to_string(),
            scanned_files: 1,
            truncated: false,
            matched_servers: vec!["fake-lsp".to_owned()],
            filtered_servers: Vec::new(),
            file_matches: Vec::new(),
        };
        let script = workspace.join("fake-lsp.ps1");
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
      $response = '{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"workspaceSymbolProvider":true}}}'
    } elseif ($request.method -eq 'workspace/symbol') {
      $response = '{"jsonrpc":"2.0","id":10,"result":[{"name":"router","kind":12}]}'
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
            server_id: "fake-lsp".to_owned(),
            program: "pwsh".to_owned(),
            args: vec![
                "-NoLogo".to_owned(),
                "-NoProfile".to_owned(),
                "-File".to_owned(),
                script.display().to_string(),
            ],
            cwd: workspace.display().to_string(),
        }];

        let report = LspWorkspaceSymbolRuntime::query_with_plans(&query, coverage, plans).unwrap();

        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].server_id, "fake-lsp");
        assert_eq!(report.hits[0].response["result"][0]["name"], "router");
    }
}
