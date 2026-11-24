//! 外部 MCP prompt 产品化 runtime。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult};
use serde_json::{Map, Value, json};

use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::{
    McpPromptMessage, McpPromptSummary, McpServerCommand, McpServerConfig, McpStdioExchangeRuntime,
    McpTransportRuntime,
};

/// MCP prompt 列表请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptListRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
    /// prompts/list 不透明分页 cursor。
    pub cursor: Option<String>,
}

/// MCP prompt 获取请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptGetRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
    /// Prompt 名称。
    pub name: String,
    /// Prompt 参数。
    pub arguments: BTreeMap<String, String>,
}

/// MCP prompt 列表投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptListProjection {
    /// Prompt 摘要。
    pub prompts: Vec<McpPromptSummary>,
    /// 下一页 cursor。
    pub next_cursor: Option<String>,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// transport 类型。
    pub transport: McpPromptTransport,
}

/// MCP prompt 获取投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptGetProjection {
    /// Prompt 描述。
    pub description: Option<String>,
    /// Prompt messages。
    pub messages: Vec<McpPromptMessage>,
    /// transport 类型。
    pub transport: McpPromptTransport,
}

/// MCP prompt transport 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPromptTransport {
    /// stdio。
    Stdio,
    /// Streamable HTTP。
    StreamableHttp,
}

/// MCP prompt runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpPromptRuntime;

impl McpPromptRuntime {
    /// 列出 MCP prompts。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn list_prompts(request: McpPromptListRequest) -> AxonResult<McpPromptListProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => list_stdio_prompts(&command, request.cursor),
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let report = McpTransportRuntime::list_streamable_http_prompts(
                    http.url,
                    &http.headers,
                    &auth_config,
                    request.cursor.as_deref(),
                )?;
                Ok(McpPromptListProjection {
                    prompts: report.prompts,
                    next_cursor: report.next_cursor,
                    requests_executed: report.requests_executed,
                    transport: McpPromptTransport::StreamableHttp,
                })
            }
        }
    }

    /// 获取 MCP prompt。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn get_prompt(request: McpPromptGetRequest) -> AxonResult<McpPromptGetProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => {
                get_stdio_prompt(&command, &request.name, request.arguments)
            }
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let report = McpTransportRuntime::get_streamable_http_prompt(
                    http.url,
                    &http.headers,
                    &auth_config,
                    &request.name,
                    arguments_value(request.arguments),
                )?;
                Ok(McpPromptGetProjection {
                    description: report.description,
                    messages: report.messages,
                    transport: McpPromptTransport::StreamableHttp,
                })
            }
        }
    }
}

fn list_stdio_prompts(
    command: &McpServerCommand,
    cursor: Option<String>,
) -> AxonResult<McpPromptListProjection> {
    let responses = McpStdioExchangeRuntime::exchange(
        command,
        vec![stdio_request(
            7,
            "prompts/list",
            cursor_params(cursor.as_deref()),
        )?],
    )?;
    Ok(McpPromptListProjection {
        prompts: parse_prompts_response(&responses, 7)?,
        next_cursor: parse_next_cursor(&responses, 7)?,
        requests_executed: 1,
        transport: McpPromptTransport::Stdio,
    })
}

fn get_stdio_prompt(
    command: &McpServerCommand,
    name: &str,
    arguments: BTreeMap<String, String>,
) -> AxonResult<McpPromptGetProjection> {
    let responses = McpStdioExchangeRuntime::exchange(
        command,
        vec![stdio_request(
            8,
            "prompts/get",
            json!({ "name": name, "arguments": arguments_value(arguments) }),
        )?],
    )?;
    let response = response_by_id(&responses, 8)?;
    Ok(McpPromptGetProjection {
        description: response
            .get("result")
            .and_then(|result| optional_string(result, "description")),
        messages: parse_get_response(&responses, 8)?,
        transport: McpPromptTransport::Stdio,
    })
}

fn stdio_request(id: u64, method: &str, params: Value) -> AxonResult<String> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|_| AxonError::InvalidMcpDefinition("MCP prompt 请求序列化失败"))
}

fn cursor_params(cursor: Option<&str>) -> Value {
    match cursor {
        Some(cursor) => json!({ "cursor": cursor }),
        None => json!({}),
    }
}

fn response_by_id(responses: &[Value], id: u64) -> AxonResult<&Value> {
    responses
        .iter()
        .find(|value| value.get("id").and_then(Value::as_u64) == Some(id))
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP prompt 响应缺少目标 id",
        ))
}

fn parse_prompts_response(responses: &[Value], id: u64) -> AxonResult<Vec<McpPromptSummary>> {
    let response = response_by_id(responses, id)?;
    crate::mcp_transport_prompts::parse_prompts_list_response(response)
}

fn parse_next_cursor(responses: &[Value], id: u64) -> AxonResult<Option<String>> {
    let response = response_by_id(responses, id)?;
    Ok(response
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned))
}

fn parse_get_response(responses: &[Value], id: u64) -> AxonResult<Vec<McpPromptMessage>> {
    let response = response_by_id(responses, id)?;
    crate::mcp_transport_prompts::parse_prompt_get_response(response)
}

fn arguments_value(arguments: BTreeMap<String, String>) -> Value {
    Value::Object(
        arguments
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<Map<_, _>>(),
    )
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}
