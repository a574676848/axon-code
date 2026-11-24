//! 模型侧 MCP resource 工具执行面。

use std::path::Path;

use axon_agent_runtime::{McpResourceListRequest, McpResourceReadRequest, McpResourceRuntime};

use super::{
    discovery::{RuntimeMcpServer, discover_runtime_servers, runtime_home},
    mcp_runtime_oauth::config_with_runtime_oauth,
};

const RESOURCES_LIST_OPERATION: &str = "resources/list";
const RESOURCE_TEMPLATES_LIST_OPERATION: &str = "resources/templates/list";

/// MCP resource 工具执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMcpResourceToolOutput {
    /// 模型可读摘要。
    pub summary: String,
    /// 脱敏结构化 metadata。
    pub metadata: serde_json::Value,
}

/// 执行模型侧 MCP resources/list 工具。
pub fn execute_runtime_mcp_list_resources(
    config_path: &Path,
    cwd: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    let home = runtime_home();
    execute_runtime_mcp_list_resources_with_home(config_path, cwd, home.as_deref(), server, cursor)
}

/// 执行模型侧 MCP resources/templates/list 工具。
pub fn execute_runtime_mcp_list_resource_templates(
    config_path: &Path,
    cwd: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    let home = runtime_home();
    execute_runtime_mcp_list_resource_templates_with_home(
        config_path,
        cwd,
        home.as_deref(),
        server,
        cursor,
    )
}

/// 执行模型侧 MCP resources/read 工具。
pub fn execute_runtime_mcp_read_resource(
    config_path: &Path,
    cwd: Option<&Path>,
    server: &str,
    uri: &str,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    let home = runtime_home();
    execute_runtime_mcp_read_resource_with_home(config_path, cwd, home.as_deref(), server, uri)
}

/// 使用指定 home 执行模型侧 MCP resources/list，供 CLI runtime 测试隔离本机环境。
pub fn execute_runtime_mcp_list_resources_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    execute_runtime_mcp_list_resources_with_home(config_path, cwd, home, server, cursor)
}

/// 使用指定 home 执行模型侧 MCP resources/templates/list，供 CLI runtime 测试隔离本机环境。
pub fn execute_runtime_mcp_list_resource_templates_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    execute_runtime_mcp_list_resource_templates_with_home(config_path, cwd, home, server, cursor)
}

/// 使用指定 home 执行模型侧 MCP resources/read，供 CLI runtime 测试隔离本机环境。
pub fn execute_runtime_mcp_read_resource_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: &str,
    uri: &str,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    execute_runtime_mcp_read_resource_with_home(config_path, cwd, home, server, uri)
}

fn execute_runtime_mcp_list_resources_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    if cursor.is_some() && server.is_none() {
        return Err("cursor 只能在指定 MCP server 时使用".to_owned());
    }
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let installs = discover_mcp_installs(&root, cwd, home, server)?;
    let server_count = installs.len();
    let mut entries = Vec::new();
    let mut page_states = Vec::new();
    let mut failures = Vec::new();
    for install in installs {
        let server_name = install.server_name.clone();
        let config = match install.mcp_config() {
            Ok(config) => config_with_runtime_oauth(&root, install.server_id(), config),
            Err(error) if server.is_none() => {
                failures.push(mcp_list_failure(
                    &server_name,
                    RESOURCES_LIST_OPERATION,
                    &error,
                ));
                continue;
            }
            Err(error) => {
                return Err(mcp_list_error(
                    &server_name,
                    RESOURCES_LIST_OPERATION,
                    error,
                ));
            }
        };
        let request = McpResourceListRequest {
            config,
            resource_cursor: cursor.map(ToOwned::to_owned),
            template_cursor: None,
        };
        let projection = match McpResourceRuntime::list_resources(request) {
            Ok(projection) => projection,
            Err(error) if server.is_none() => {
                let message = format!("{error:?}");
                failures.push(mcp_list_failure(
                    &server_name,
                    RESOURCES_LIST_OPERATION,
                    &message,
                ));
                continue;
            }
            Err(error) => {
                return Err(mcp_list_error(
                    &server_name,
                    RESOURCES_LIST_OPERATION,
                    format!("{error:?}"),
                ));
            }
        };
        let next_cursor = projection.resources_next_cursor.clone();
        page_states.push(serde_json::json!({
            "server": server_name,
            "nextCursor": next_cursor,
            "requestsExecuted": projection.resources_requests_executed,
        }));
        for resource in projection.resources {
            entries.push(serde_json::json!({
                "server": server_name,
                "uri": resource.uri,
                "name": resource.name,
                "description": resource.description,
                "mimeType": resource.mime_type,
            }));
        }
    }
    let returned = entries.len();
    let failed = failures.len();
    Ok(RuntimeMcpResourceToolOutput {
        summary: serde_json::json!({"resources": entries}).to_string(),
        metadata: serde_json::json!({
            "mcpResourceServersQueried": server_count,
            "mcpResourceServersFailed": failed,
            "mcpResourceFailures": failures,
            "mcpResourcesReturned": returned,
            "mcpResourcePagination": page_states,
            "cursor": cursor,
            "rawPayloadExposed": false,
        }),
    })
}

fn execute_runtime_mcp_list_resource_templates_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    if cursor.is_some() && server.is_none() {
        return Err("cursor 只能在指定 MCP server 时使用".to_owned());
    }
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let installs = discover_mcp_installs(&root, cwd, home, server)?;
    let server_count = installs.len();
    let mut entries = Vec::new();
    let mut page_states = Vec::new();
    let mut failures = Vec::new();
    for install in installs {
        let server_name = install.server_name.clone();
        let config = match install.mcp_config() {
            Ok(config) => config_with_runtime_oauth(&root, install.server_id(), config),
            Err(error) if server.is_none() => {
                failures.push(mcp_list_failure(
                    &server_name,
                    RESOURCE_TEMPLATES_LIST_OPERATION,
                    &error,
                ));
                continue;
            }
            Err(error) => {
                return Err(mcp_list_error(
                    &server_name,
                    RESOURCE_TEMPLATES_LIST_OPERATION,
                    error,
                ));
            }
        };
        let request = McpResourceListRequest {
            config,
            resource_cursor: None,
            template_cursor: cursor.map(ToOwned::to_owned),
        };
        let projection = match McpResourceRuntime::list_resources(request) {
            Ok(projection) => projection,
            Err(error) if server.is_none() => {
                let message = format!("{error:?}");
                failures.push(mcp_list_failure(
                    &server_name,
                    RESOURCE_TEMPLATES_LIST_OPERATION,
                    &message,
                ));
                continue;
            }
            Err(error) => {
                return Err(mcp_list_error(
                    &server_name,
                    RESOURCE_TEMPLATES_LIST_OPERATION,
                    format!("{error:?}"),
                ));
            }
        };
        let next_cursor = projection.templates_next_cursor.clone();
        page_states.push(serde_json::json!({
            "server": server_name,
            "nextCursor": next_cursor,
            "requestsExecuted": projection.templates_requests_executed,
        }));
        for template in projection.templates {
            entries.push(serde_json::json!({
                "server": server_name,
                "uriTemplate": template.uri_template,
                "name": template.name,
                "description": template.description,
                "mimeType": template.mime_type,
            }));
        }
    }
    let returned = entries.len();
    let failed = failures.len();
    Ok(RuntimeMcpResourceToolOutput {
        summary: serde_json::json!({"resourceTemplates": entries}).to_string(),
        metadata: serde_json::json!({
            "mcpResourceServersQueried": server_count,
            "mcpResourceServersFailed": failed,
            "mcpResourceFailures": failures,
            "mcpResourceTemplatesReturned": returned,
            "mcpResourceTemplatePagination": page_states,
            "cursor": cursor,
            "rawPayloadExposed": false,
        }),
    })
}

fn execute_runtime_mcp_read_resource_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: &str,
    uri: &str,
) -> Result<RuntimeMcpResourceToolOutput, String> {
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let install = discover_mcp_installs(&root, cwd, home, Some(server))?
        .into_iter()
        .next()
        .ok_or_else(|| format!("未找到 MCP server：{server}"))?;
    let config = config_with_runtime_oauth(&root, install.server_id(), install.mcp_config()?);
    let projection = McpResourceRuntime::read_resource(McpResourceReadRequest {
        config,
        uri: uri.to_owned(),
    })
    .map_err(|error| format!("MCP resources/read 失败：{error:?}"))?;
    let contents = projection
        .contents
        .into_iter()
        .map(|content| {
            serde_json::json!({
                "uri": content.uri,
                "mimeType": content.mime_type,
                "text": content.text,
                "blob": content.blob,
            })
        })
        .collect::<Vec<_>>();
    Ok(RuntimeMcpResourceToolOutput {
        summary: serde_json::json!({"server": server, "contents": contents}).to_string(),
        metadata: serde_json::json!({
            "mcpResourceServer": server,
            "mcpResourceUri": uri,
            "mcpResourceContentCount": contents.len(),
            "rawPayloadExposed": false,
        }),
    })
}

fn discover_mcp_installs(
    root: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
) -> Result<Vec<RuntimeMcpServer>, String> {
    let installs = discover_runtime_servers(root, cwd, home)?
        .into_iter()
        .filter(|install| server.is_none_or(|server| install.matches_name(server)))
        .collect::<Vec<_>>();
    if server.is_some() && installs.is_empty() {
        return Err(format!("未找到 MCP server：{}", server.unwrap_or_default()));
    }
    Ok(installs)
}

fn mcp_list_failure(server_name: &str, operation: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "server": server_name,
        "operation": operation,
        "message": message,
    })
}

fn mcp_list_error(server_name: &str, operation: &str, message: impl AsRef<str>) -> String {
    format!(
        "MCP {operation} 失败：server={server_name}；{}",
        message.as_ref()
    )
}
