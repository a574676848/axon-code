use std::collections::BTreeMap;

use axon_agent_runtime::{McpServerCommand, McpServerConfig};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum McpConfigObject {
    Stdio(McpStdioConfigObject),
    StreamableHttp(McpStreamableHttpConfigObject),
}

#[derive(Debug, Serialize)]
struct McpStdioConfigObject {
    #[serde(rename = "type")]
    transport_type: &'static str,
    command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "cwd",
        alias = "workingDir",
        alias = "working_dir",
        skip_serializing_if = "Option::is_none"
    )]
    working_dir: Option<String>,
}

#[derive(Debug, Serialize)]
struct McpStreamableHttpConfigObject {
    #[serde(rename = "type")]
    transport_type: &'static str,
    url: String,
    #[serde(
        default,
        rename = "bearerTokenEnvVar",
        alias = "bearer_token_env_var",
        skip_serializing_if = "Option::is_none"
    )]
    bearer_token_env_var: Option<String>,
}

pub(crate) fn mcp_config_json(config: &McpServerConfig) -> Result<String, String> {
    let document = match config {
        McpServerConfig::Stdio(command) => McpConfigObject::Stdio(stdio_config(command)),
        McpServerConfig::StreamableHttp(http) => {
            McpConfigObject::StreamableHttp(McpStreamableHttpConfigObject {
                transport_type: "streamable_http",
                url: http.url.clone(),
                bearer_token_env_var: http.bearer_token_env_var.clone(),
            })
        }
    };
    serde_json::to_string_pretty(&document).map_err(|_| "MCP 配置序列化失败".to_owned())
}

pub(crate) fn mcp_config_value(config: &McpServerConfig) -> Result<Value, String> {
    let raw = mcp_config_json(config)?;
    serde_json::from_str(&raw).map_err(|_| "MCP 配置序列化失败".to_owned())
}

fn stdio_config(command: &McpServerCommand) -> McpStdioConfigObject {
    McpStdioConfigObject {
        transport_type: "stdio",
        command: command.program.clone(),
        args: command.args.clone(),
        env: command.env.clone(),
        working_dir: command.working_dir.clone(),
    }
}
