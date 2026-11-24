use std::collections::BTreeMap;

use axon_agent_runtime::{McpHttpServerConfig, McpServerCommand, McpServerConfig};

use crate::mcp_commands::McpAddCommand;

pub(super) fn server_config_from_add(command: &McpAddCommand) -> Result<McpServerConfig, String> {
    match command.url.as_ref() {
        Some(url) => streamable_http_config(command, url),
        None => stdio_config(command),
    }
}

fn stdio_config(command: &McpAddCommand) -> Result<McpServerConfig, String> {
    let Some(program) = command.command.first() else {
        return Err(
            "MCP stdio 启动命令不能为空；或使用 --url 添加 Streamable HTTP server".to_owned(),
        );
    };
    if program.trim().is_empty() {
        return Err("MCP 启动命令不能为空".to_owned());
    }
    Ok(McpServerConfig::Stdio(McpServerCommand {
        program: program.clone(),
        args: command.command.iter().skip(1).cloned().collect(),
        working_dir: command
            .working_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        env: BTreeMap::new(),
        tool_timeout_ms: None,
    }))
}

fn streamable_http_config(command: &McpAddCommand, url: &str) -> Result<McpServerConfig, String> {
    if url.trim().is_empty() {
        return Err("MCP Streamable HTTP URL 不能为空".to_owned());
    }
    if !command.command.is_empty() {
        return Err("MCP add 不能同时传入 --url 与 stdio 启动命令".to_owned());
    }
    if command.working_dir.is_some() {
        return Err("MCP Streamable HTTP server 不支持 --working-dir".to_owned());
    }
    Ok(McpServerConfig::StreamableHttp(McpHttpServerConfig {
        url: url.to_owned(),
        headers: Vec::new(),
        bearer_token_env_var: command.bearer_token_env_var.clone(),
        oauth_access_token: None,
        tool_timeout_ms: None,
    }))
}
