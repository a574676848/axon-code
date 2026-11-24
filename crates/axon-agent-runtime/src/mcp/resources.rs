//! 外部 MCP resource 产品化 runtime。

use axon_domain::{AxonError, AxonResult};
use serde_json::{Value, json};

use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::mcp_transport_http::{
    MCP_RESOURCES_SUBSCRIBE_ID, MCP_RESOURCES_SUBSCRIBE_METHOD, MCP_RESOURCES_UNSUBSCRIBE_ID,
    MCP_RESOURCES_UNSUBSCRIBE_METHOD,
};
use crate::{
    McpNotificationSummary, McpResourceContent, McpResourceSummary, McpResourceTemplateSummary,
    McpServerCommand, McpServerConfig, McpStdioExchangeRuntime, McpTransportRuntime,
};

/// MCP resource 列表请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceListRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
    /// resources/list 不透明分页 cursor。
    pub resource_cursor: Option<String>,
    /// resources/templates/list 不透明分页 cursor。
    pub template_cursor: Option<String>,
}

/// MCP resource 读取请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceReadRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
    /// Resource URI。
    pub uri: String,
}

/// MCP resource 订阅请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceSubscriptionRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
    /// Resource URI。
    pub uri: String,
}

/// MCP resource 列表投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceListProjection {
    /// Resource 摘要。
    pub resources: Vec<McpResourceSummary>,
    /// Resource template 摘要。
    pub templates: Vec<McpResourceTemplateSummary>,
    /// resources/list 下一页 cursor。
    pub resources_next_cursor: Option<String>,
    /// resources/templates/list 下一页 cursor。
    pub templates_next_cursor: Option<String>,
    /// resources/list 本次请求数量。
    pub resources_requests_executed: usize,
    /// resources/templates/list 本次请求数量。
    pub templates_requests_executed: usize,
    /// transport 类型。
    pub transport: McpResourceTransport,
}

/// MCP resource 读取投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceReadProjection {
    /// Resource 内容。
    pub contents: Vec<McpResourceContent>,
    /// transport 类型。
    pub transport: McpResourceTransport,
}

/// MCP resource 订阅投影。
#[derive(Debug, Clone, PartialEq)]
pub struct McpResourceSubscriptionProjection {
    /// Resource URI。
    pub uri: String,
    /// 操作名称。
    pub action: &'static str,
    /// 是否暴露原始 payload。
    pub raw_payload_exposed: bool,
    /// 是否允许远程触发。
    pub remote_trigger_allowed: bool,
    /// 安全通知摘要。
    pub notification_summary: McpNotificationSummary,
    /// transport 类型。
    pub transport: McpResourceTransport,
}

/// MCP resource transport 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpResourceTransport {
    /// stdio。
    Stdio,
    /// Streamable HTTP。
    StreamableHttp,
}

/// MCP resource runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpResourceRuntime;

impl McpResourceRuntime {
    /// 列出 MCP resources 与 templates。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn list_resources(
        request: McpResourceListRequest,
    ) -> AxonResult<McpResourceListProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => {
                list_stdio_resources(&command, request.resource_cursor, request.template_cursor)
            }
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let resources = McpTransportRuntime::list_streamable_http_resources(
                    http.url.clone(),
                    &http.headers,
                    &auth_config,
                    request.resource_cursor.as_deref(),
                )?;
                let templates = McpTransportRuntime::list_streamable_http_resource_templates(
                    http.url,
                    &http.headers,
                    &auth_config,
                    request.template_cursor.as_deref(),
                )?;
                Ok(McpResourceListProjection {
                    resources: resources.resources,
                    templates: templates.templates,
                    resources_next_cursor: resources.next_cursor,
                    templates_next_cursor: templates.next_cursor,
                    resources_requests_executed: resources.requests_executed,
                    templates_requests_executed: templates.requests_executed,
                    transport: McpResourceTransport::StreamableHttp,
                })
            }
        }
    }

    /// 读取 MCP resource。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn read_resource(request: McpResourceReadRequest) -> AxonResult<McpResourceReadProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => read_stdio_resource(&command, &request.uri),
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let report = McpTransportRuntime::read_streamable_http_resource(
                    http.url,
                    &http.headers,
                    &auth_config,
                    &request.uri,
                )?;
                Ok(McpResourceReadProjection {
                    contents: report.contents,
                    transport: McpResourceTransport::StreamableHttp,
                })
            }
        }
    }

    /// 订阅 MCP resource 更新通知。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn subscribe_resource(
        request: McpResourceSubscriptionRequest,
    ) -> AxonResult<McpResourceSubscriptionProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => resource_subscription_stdio(
                &command,
                &request.uri,
                MCP_RESOURCES_SUBSCRIBE_METHOD,
                MCP_RESOURCES_SUBSCRIBE_ID,
            ),
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let report = McpTransportRuntime::subscribe_streamable_http_resource(
                    http.url,
                    &http.headers,
                    &auth_config,
                    &request.uri,
                )?;
                Ok(McpResourceSubscriptionProjection {
                    uri: report.uri,
                    action: report.action,
                    raw_payload_exposed: false,
                    remote_trigger_allowed: false,
                    notification_summary: report.notification_summary,
                    transport: McpResourceTransport::StreamableHttp,
                })
            }
        }
    }

    /// 取消订阅 MCP resource 更新通知。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn unsubscribe_resource(
        request: McpResourceSubscriptionRequest,
    ) -> AxonResult<McpResourceSubscriptionProjection> {
        match request.config {
            McpServerConfig::Stdio(command) => resource_subscription_stdio(
                &command,
                &request.uri,
                MCP_RESOURCES_UNSUBSCRIBE_METHOD,
                MCP_RESOURCES_UNSUBSCRIBE_ID,
            ),
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let report = McpTransportRuntime::unsubscribe_streamable_http_resource(
                    http.url,
                    &http.headers,
                    &auth_config,
                    &request.uri,
                )?;
                Ok(McpResourceSubscriptionProjection {
                    uri: report.uri,
                    action: report.action,
                    raw_payload_exposed: false,
                    remote_trigger_allowed: false,
                    notification_summary: report.notification_summary,
                    transport: McpResourceTransport::StreamableHttp,
                })
            }
        }
    }
}

fn list_stdio_resources(
    command: &McpServerCommand,
    resource_cursor: Option<String>,
    template_cursor: Option<String>,
) -> AxonResult<McpResourceListProjection> {
    let responses = McpStdioExchangeRuntime::exchange(
        command,
        vec![
            stdio_request(
                4,
                "resources/list",
                cursor_params(resource_cursor.as_deref()),
            )?,
            stdio_request(
                5,
                "resources/templates/list",
                cursor_params(template_cursor.as_deref()),
            )?,
        ],
    )?;
    let resources = parse_resources_response(&responses, 4)?;
    let templates = parse_templates_response(&responses, 5)?;
    let resources_next_cursor = parse_next_cursor(&responses, 4)?;
    let templates_next_cursor = parse_next_cursor(&responses, 5)?;
    Ok(McpResourceListProjection {
        resources,
        templates,
        resources_next_cursor,
        templates_next_cursor,
        resources_requests_executed: 1,
        templates_requests_executed: 1,
        transport: McpResourceTransport::Stdio,
    })
}

fn cursor_params(cursor: Option<&str>) -> Value {
    match cursor {
        Some(cursor) => json!({ "cursor": cursor }),
        None => json!({}),
    }
}

fn read_stdio_resource(
    command: &McpServerCommand,
    uri: &str,
) -> AxonResult<McpResourceReadProjection> {
    let responses = McpStdioExchangeRuntime::exchange(
        command,
        vec![stdio_request(6, "resources/read", json!({ "uri": uri }))?],
    )?;
    Ok(McpResourceReadProjection {
        contents: parse_read_response(&responses, 6)?,
        transport: McpResourceTransport::Stdio,
    })
}

fn resource_subscription_stdio(
    command: &McpServerCommand,
    uri: &str,
    method: &'static str,
    id: u64,
) -> AxonResult<McpResourceSubscriptionProjection> {
    let report = McpStdioExchangeRuntime::exchange_with_notifications(
        command,
        vec![stdio_request(id, method, json!({ "uri": uri }))?],
    )?;
    let response = response_by_id(&report.responses, id)?;
    if response.get("error").is_some() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP resource 订阅请求返回 error",
        ));
    }
    Ok(McpResourceSubscriptionProjection {
        uri: uri.to_owned(),
        action: if method.ends_with("unsubscribe") {
            "unsubscribe"
        } else {
            "subscribe"
        },
        raw_payload_exposed: false,
        remote_trigger_allowed: false,
        notification_summary: McpNotificationSummary::from_events(&report.notifications),
        transport: McpResourceTransport::Stdio,
    })
}

fn stdio_request(id: u64, method: &str, params: Value) -> AxonResult<String> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|_| AxonError::InvalidMcpDefinition("MCP resource 请求序列化失败"))
}

fn response_by_id(responses: &[Value], id: u64) -> AxonResult<&Value> {
    responses
        .iter()
        .find(|value| value.get("id").and_then(Value::as_u64) == Some(id))
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP resource 响应缺少目标 id",
        ))
}

fn parse_resources_response(responses: &[Value], id: u64) -> AxonResult<Vec<McpResourceSummary>> {
    let resources = response_by_id(responses, id)?
        .get("result")
        .and_then(|result| result.get("resources"))
        .and_then(Value::as_array)
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP resources/list 响应缺少 resources",
        ))?;
    Ok(resources
        .iter()
        .filter_map(|resource| {
            Some(McpResourceSummary {
                uri: resource.get("uri").and_then(Value::as_str)?.to_owned(),
                name: required_or_fallback(resource, "name", "uri")?,
                description: optional_string(resource, "description"),
                mime_type: optional_string(resource, "mimeType"),
            })
        })
        .collect())
}

fn parse_templates_response(
    responses: &[Value],
    id: u64,
) -> AxonResult<Vec<McpResourceTemplateSummary>> {
    let templates = response_by_id(responses, id)?
        .get("result")
        .and_then(|result| result.get("resourceTemplates"))
        .and_then(Value::as_array)
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP resources/templates/list 响应缺少 resourceTemplates",
        ))?;
    Ok(templates
        .iter()
        .filter_map(|template| {
            Some(McpResourceTemplateSummary {
                uri_template: template
                    .get("uriTemplate")
                    .and_then(Value::as_str)?
                    .to_owned(),
                name: required_or_fallback(template, "name", "uriTemplate")?,
                description: optional_string(template, "description"),
                mime_type: optional_string(template, "mimeType"),
            })
        })
        .collect())
}

fn parse_read_response(responses: &[Value], id: u64) -> AxonResult<Vec<McpResourceContent>> {
    let contents = response_by_id(responses, id)?
        .get("result")
        .and_then(|result| result.get("contents"))
        .and_then(Value::as_array)
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP resources/read 响应缺少 contents",
        ))?;
    Ok(contents
        .iter()
        .filter_map(|content| {
            Some(McpResourceContent {
                uri: content.get("uri").and_then(Value::as_str)?.to_owned(),
                mime_type: optional_string(content, "mimeType"),
                text: optional_string(content, "text"),
                blob: optional_string(content, "blob"),
            })
        })
        .collect())
}

fn parse_next_cursor(responses: &[Value], id: u64) -> AxonResult<Option<String>> {
    Ok(response_by_id(responses, id)?
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned))
}

fn required_or_fallback(value: &Value, field: &str, fallback: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .or_else(|| value.get(fallback).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn optional_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}
