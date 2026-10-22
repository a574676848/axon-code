//! 模型侧 MCP prompt 工具执行面。

use std::{collections::BTreeMap, path::Path};

use axon_agent_runtime::{McpPromptGetRequest, McpPromptListRequest, McpPromptRuntime};

use super::{
    discovery::{RuntimeMcpServer, discover_runtime_servers, runtime_home},
    mcp_runtime_oauth::config_with_runtime_oauth,
};

/// MCP prompt 工具执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMcpPromptToolOutput {
    /// 模型可读摘要。
    pub summary: String,
    /// 脱敏结构化 metadata。
    pub metadata: serde_json::Value,
}

/// 执行模型侧 MCP prompts/list 工具。
pub fn execute_runtime_mcp_list_prompts(
    config_path: &Path,
    cwd: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpPromptToolOutput, String> {
    let home = runtime_home();
    execute_runtime_mcp_list_prompts_with_home(config_path, cwd, home.as_deref(), server, cursor)
}

/// 执行模型侧 MCP prompts/get 工具。
pub fn execute_runtime_mcp_get_prompt(
    config_path: &Path,
    cwd: Option<&Path>,
    server: &str,
    name: &str,
    arguments: BTreeMap<String, String>,
) -> Result<RuntimeMcpPromptToolOutput, String> {
    let home = runtime_home();
    execute_runtime_mcp_get_prompt_with_home(
        config_path,
        cwd,
        home.as_deref(),
        server,
        name,
        arguments,
    )
}

/// 使用指定 home 执行模型侧 MCP prompts/list，供 CLI runtime 测试隔离本机环境。
pub fn execute_runtime_mcp_list_prompts_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpPromptToolOutput, String> {
    execute_runtime_mcp_list_prompts_with_home(config_path, cwd, home, server, cursor)
}

/// 使用指定 home 执行模型侧 MCP prompts/get，供 CLI runtime 测试隔离本机环境。
pub fn execute_runtime_mcp_get_prompt_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: &str,
    name: &str,
    arguments: BTreeMap<String, String>,
) -> Result<RuntimeMcpPromptToolOutput, String> {
    execute_runtime_mcp_get_prompt_with_home(config_path, cwd, home, server, name, arguments)
}

fn execute_runtime_mcp_list_prompts_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<RuntimeMcpPromptToolOutput, String> {
    if cursor.is_some() && server.is_none() {
        return Err("cursor 只能在指定 MCP server 时使用".to_owned());
    }
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let installs = discover_mcp_installs(&root, cwd, home, server)?;
    let server_count = installs.len();
    let mut entries = Vec::new();
    let mut page_states = Vec::new();
    for install in installs {
        let server_name = install.server_name.clone();
        let config = config_with_runtime_oauth(&root, install.server_id(), install.mcp_config()?);
        let projection = McpPromptRuntime::list_prompts(McpPromptListRequest {
            config,
            cursor: cursor.map(ToOwned::to_owned),
        })
        .map_err(|error| format!("MCP prompts/list 失败：{error:?}"))?;
        let next_cursor = projection.next_cursor.clone();
        page_states.push(serde_json::json!({
            "server": server_name,
            "nextCursor": next_cursor,
            "requestsExecuted": projection.requests_executed,
        }));
        for prompt in projection.prompts {
            let arguments = prompt
                .arguments
                .into_iter()
                .map(|argument| {
                    serde_json::json!({
                        "name": argument.name,
                        "title": argument.title,
                        "description": argument.description,
                        "required": argument.required,
                    })
                })
                .collect::<Vec<_>>();
            entries.push(serde_json::json!({
                "server": server_name,
                "name": prompt.name,
                "title": prompt.title,
                "description": prompt.description,
                "arguments": arguments,
            }));
        }
    }
    let returned = entries.len();
    Ok(RuntimeMcpPromptToolOutput {
        summary: serde_json::json!({"prompts": entries}).to_string(),
        metadata: serde_json::json!({
            "mcpPromptServersQueried": server_count,
            "mcpPromptsReturned": returned,
            "mcpPromptPagination": page_states,
            "cursor": cursor,
            "rawPayloadExposed": false,
            "remoteTriggerAllowed": false,
        }),
    })
}

fn execute_runtime_mcp_get_prompt_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    server: &str,
    name: &str,
    arguments: BTreeMap<String, String>,
) -> Result<RuntimeMcpPromptToolOutput, String> {
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let install = discover_mcp_installs(&root, cwd, home, Some(server))?
        .into_iter()
        .next()
        .ok_or_else(|| format!("未找到 MCP server：{server}"))?;
    let config = config_with_runtime_oauth(&root, install.server_id(), install.mcp_config()?);
    let projection = McpPromptRuntime::get_prompt(McpPromptGetRequest {
        config,
        name: name.to_owned(),
        arguments,
    })
    .map_err(|error| format!("MCP prompts/get 失败：{error:?}"))?;
    let messages = projection
        .messages
        .into_iter()
        .map(|message| {
            serde_json::json!({
                "role": message.role,
                "contentType": message.content_type,
                "text": message.text,
                "resourceUri": message.resource_uri,
            })
        })
        .collect::<Vec<_>>();
    Ok(RuntimeMcpPromptToolOutput {
        summary: serde_json::json!({
            "server": server,
            "name": name,
            "description": projection.description,
            "messages": messages,
        })
        .to_string(),
        metadata: serde_json::json!({
            "mcpPromptServer": server,
            "mcpPromptName": name,
            "mcpPromptMessageCount": messages.len(),
            "rawPayloadExposed": false,
            "remoteTriggerAllowed": false,
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
