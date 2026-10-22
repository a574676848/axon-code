//! MCP server 元数据与协议能力投影。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::mcp_transport_http::{StreamableHttpSession, parse_http_response_json};
use crate::{McpServerCommand, McpServerConfig, McpStdioExchangeRuntime};

/// MCP server 信息投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerInfoProjection {
    /// server 名称。
    pub name: Option<String>,
    /// server title。
    pub title: Option<String>,
    /// server 版本。
    pub version: Option<String>,
    /// server website URL。
    pub website_url: Option<String>,
    /// server instructions 是否存在。
    pub instructions_declared: bool,
    /// instructions 摘要。
    pub instructions_preview: Option<String>,
    /// server capabilities。
    pub capabilities: McpServerCapabilitiesProjection,
    /// 是否暴露原始 initialize payload。
    pub raw_payload_exposed: bool,
}

/// MCP server capabilities 投影。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerCapabilitiesProjection {
    /// 是否声明 tools。
    pub tools: bool,
    /// tools/listChanged。
    pub tools_list_changed: Option<bool>,
    /// 是否声明 resources。
    pub resources: bool,
    /// resources/listChanged。
    pub resources_list_changed: Option<bool>,
    /// resources/subscribe。
    pub resources_subscribe: Option<bool>,
    /// 是否声明 prompts。
    pub prompts: bool,
    /// prompts/listChanged。
    pub prompts_list_changed: Option<bool>,
    /// 是否声明 completions。
    pub completions: bool,
}

/// MCP server metadata 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerMetadataRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
}

/// MCP server metadata runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpServerMetadataRuntime;

impl McpServerMetadataRuntime {
    /// 读取 MCP initialize 结果并投影 serverInfo、instructions 与 capabilities。
    ///
    /// # Errors
    /// MCP server 启动、握手或 initialize 响应解析失败时返回错误。
    pub fn read_metadata(request: McpServerMetadataRequest) -> AxonResult<McpServerInfoProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => read_stdio_metadata(&command),
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let response = StreamableHttpSession::initialize_result_text(
                    &http.url,
                    &http.headers,
                    &auth_config,
                )?;
                let value = parse_http_response_json(&response)?;
                projection_from_initialize_response(&value)
            }
        }
    }
}

fn read_stdio_metadata(command: &McpServerCommand) -> AxonResult<McpServerInfoProjection> {
    let responses = McpStdioExchangeRuntime::exchange(command, Vec::new())?;
    let initialize = responses
        .first()
        .ok_or(AxonError::InvalidMcpDefinition("MCP initialize 响应缺失"))?;
    projection_from_initialize_response(initialize)
}

fn projection_from_initialize_response(value: &Value) -> AxonResult<McpServerInfoProjection> {
    let result = value.get("result").ok_or(AxonError::InvalidMcpDefinition(
        "MCP initialize 响应缺少 result",
    ))?;
    let server_info = result.get("serverInfo").unwrap_or(&Value::Null);
    let instructions = result
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    Ok(McpServerInfoProjection {
        name: optional_string(server_info, "name"),
        title: optional_string(server_info, "title"),
        version: optional_string(server_info, "version"),
        website_url: optional_string(server_info, "websiteUrl"),
        instructions_declared: instructions.is_some(),
        instructions_preview: instructions.map(instruction_preview),
        capabilities: capabilities_projection(result.get("capabilities")),
        raw_payload_exposed: false,
    })
}

fn capabilities_projection(value: Option<&Value>) -> McpServerCapabilitiesProjection {
    let capability = value.unwrap_or(&Value::Null);
    McpServerCapabilitiesProjection {
        tools: capability.get("tools").is_some(),
        tools_list_changed: capability_bool(capability, "tools", "listChanged"),
        resources: capability.get("resources").is_some(),
        resources_list_changed: capability_bool(capability, "resources", "listChanged"),
        resources_subscribe: capability_bool(capability, "resources", "subscribe"),
        prompts: capability.get("prompts").is_some(),
        prompts_list_changed: capability_bool(capability, "prompts", "listChanged"),
        completions: capability.get("completions").is_some(),
    }
}

fn capability_bool(value: &Value, section: &str, key: &str) -> Option<bool> {
    value
        .get(section)
        .and_then(|section| section.get(key))
        .and_then(Value::as_bool)
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn instruction_preview(value: &str) -> String {
    const INSTRUCTION_PREVIEW_MAX_CHARS: usize = 160;
    let trimmed = value.trim();
    trimmed
        .chars()
        .take(INSTRUCTION_PREVIEW_MAX_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_projection_extracts_server_info_capabilities_and_instructions() {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "listChanged": true, "subscribe": false },
                    "prompts": { "listChanged": false },
                    "completions": {}
                },
                "serverInfo": {
                    "name": "team-tools",
                    "title": "Team Tools",
                    "version": "1.2.3",
                    "websiteUrl": "https://example.test"
                },
                "instructions": "Use stable tool names."
            }
        });

        let projection = projection_from_initialize_response(&value).unwrap();

        assert_eq!(projection.name.as_deref(), Some("team-tools"));
        assert_eq!(projection.title.as_deref(), Some("Team Tools"));
        assert_eq!(projection.version.as_deref(), Some("1.2.3"));
        assert_eq!(
            projection.website_url.as_deref(),
            Some("https://example.test")
        );
        assert!(projection.instructions_declared);
        assert_eq!(
            projection.instructions_preview.as_deref(),
            Some("Use stable tool names.")
        );
        assert!(projection.capabilities.tools);
        assert_eq!(projection.capabilities.tools_list_changed, Some(false));
        assert!(projection.capabilities.resources);
        assert_eq!(projection.capabilities.resources_list_changed, Some(true));
        assert_eq!(projection.capabilities.resources_subscribe, Some(false));
        assert!(projection.capabilities.prompts);
        assert!(projection.capabilities.completions);
        assert!(!projection.raw_payload_exposed);
    }
}
