//! MCP Streamable HTTP resource wire API。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::mcp_transport::{
    optional_string, required_or_fallback, send_streamable_http_request, streamable_http_reasons,
    streamable_http_request,
};
use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::mcp_transport_http::{
    MCP_RESOURCE_TEMPLATES_LIST_ID, MCP_RESOURCE_TEMPLATES_LIST_METHOD, MCP_RESOURCES_LIST_ID,
    MCP_RESOURCES_LIST_METHOD, MCP_RESOURCES_READ_ID, MCP_RESOURCES_READ_METHOD,
    MCP_RESOURCES_SUBSCRIBE_ID, MCP_RESOURCES_SUBSCRIBE_METHOD, MCP_RESOURCES_UNSUBSCRIBE_ID,
    MCP_RESOURCES_UNSUBSCRIBE_METHOD, StreamableHttpSession, mcp_meta_value, mcp_tool_timeout,
    parse_http_response_json,
};
use crate::{McpHttpRequestPlan, McpTransportRuntime};

const MCP_META_KEY: &str = "_meta";

/// MCP resource 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceSummary {
    /// Resource URI。
    pub uri: String,
    /// 展示名称。
    pub name: String,
    /// 描述。
    pub description: Option<String>,
    /// MIME 类型。
    pub mime_type: Option<String>,
}

/// MCP resource template 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceTemplateSummary {
    /// URI template。
    pub uri_template: String,
    /// 展示名称。
    pub name: String,
    /// 描述。
    pub description: Option<String>,
    /// MIME 类型。
    pub mime_type: Option<String>,
}

/// MCP resource 读取内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceContent {
    /// Resource URI。
    pub uri: String,
    /// MIME 类型。
    pub mime_type: Option<String>,
    /// 文本内容。
    pub text: Option<String>,
    /// Blob 内容。
    pub blob: Option<String>,
}

/// Streamable HTTP resources/list 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpResourcesReport {
    /// server 返回的 resource 摘要。
    pub resources: Vec<McpResourceSummary>,
    /// 下一页 cursor。
    pub next_cursor: Option<String>,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP resources/list 分页报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpPagedResourcesReport {
    /// server 返回的所有 resource 摘要。
    pub resources: Vec<McpResourceSummary>,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP resources/templates/list 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpResourceTemplatesReport {
    /// server 返回的 resource template 摘要。
    pub templates: Vec<McpResourceTemplateSummary>,
    /// 下一页 cursor。
    pub next_cursor: Option<String>,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP resources/templates/list 分页报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpPagedResourceTemplatesReport {
    /// server 返回的所有 resource template 摘要。
    pub templates: Vec<McpResourceTemplateSummary>,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP resources/read 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpResourceReadReport {
    /// server 返回的 resource 内容。
    pub contents: Vec<McpResourceContent>,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP resources/subscribe 或 unsubscribe 报告。
#[derive(Debug, Clone, PartialEq)]
pub struct McpStreamableHttpResourceSubscriptionReport {
    /// 资源 URI。
    pub uri: String,
    /// 操作名称。
    pub action: &'static str,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 安全通知摘要。
    pub notification_summary: crate::McpNotificationSummary,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

impl McpTransportRuntime {
    /// 通过 Streamable HTTP 执行 resources/list。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn list_streamable_http_resources(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        cursor: Option<&str>,
    ) -> AxonResult<McpStreamableHttpResourcesReport> {
        let url = url.into();
        let request = paged_request(
            &url,
            MCP_RESOURCES_LIST_ID,
            MCP_RESOURCES_LIST_METHOD,
            cursor,
        )?;
        let value = send_streamable_http_request(&url, extra_headers, auth_config, &request)?;
        Ok(McpStreamableHttpResourcesReport {
            resources: parse_resources_list_response(&value)?,
            next_cursor: next_cursor(&value),
            request,
            requests_executed: 1,
            reasons: streamable_http_reasons(),
        })
    }

    /// 通过 Streamable HTTP 执行带 cursor 的 resources/list。
    ///
    /// # Errors
    /// 请求失败、响应非法、server 返回 error 或 cursor 重复时返回错误。
    pub fn list_streamable_http_resources_with_pagination(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
    ) -> AxonResult<McpStreamableHttpPagedResourcesReport> {
        let url = url.into();
        let mut session = StreamableHttpSession::initialize(&url, extra_headers, auth_config)?;
        let mut cursor = None::<String>;
        let mut seen_cursors = std::collections::BTreeSet::new();
        let mut resources = Vec::new();
        let mut requests_executed = 0;
        loop {
            let request = paged_request(
                &url,
                MCP_RESOURCES_LIST_ID,
                MCP_RESOURCES_LIST_METHOD,
                cursor.as_deref(),
            )?;
            let response_text = session.post_request(
                &request,
                extra_headers,
                auth_config,
                Some(mcp_tool_timeout()),
            )?;
            let value = parse_http_response_json(&response_text)?;
            if value.get("error").is_some() {
                return Err(AxonError::tool_invalid_definition(
                    "MCP resources/list 返回 error",
                ));
            }
            resources.extend(parse_resources_list_response(&value)?);
            requests_executed += 1;
            let Some(next_cursor) = next_cursor(&value) else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(AxonError::InvalidMcpDefinition(
                    "MCP resources/list 返回重复 cursor",
                ));
            }
            cursor = Some(next_cursor);
        }
        Ok(McpStreamableHttpPagedResourcesReport {
            resources,
            requests_executed,
            reasons: streamable_http_reasons(),
        })
    }

    /// 通过 Streamable HTTP 执行 resources/templates/list。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn list_streamable_http_resource_templates(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        cursor: Option<&str>,
    ) -> AxonResult<McpStreamableHttpResourceTemplatesReport> {
        let url = url.into();
        let request = paged_request(
            &url,
            MCP_RESOURCE_TEMPLATES_LIST_ID,
            MCP_RESOURCE_TEMPLATES_LIST_METHOD,
            cursor,
        )?;
        let value = send_streamable_http_request(&url, extra_headers, auth_config, &request)?;
        Ok(McpStreamableHttpResourceTemplatesReport {
            templates: parse_resource_templates_list_response(&value)?,
            next_cursor: next_cursor(&value),
            request,
            requests_executed: 1,
            reasons: streamable_http_reasons(),
        })
    }

    /// 通过 Streamable HTTP 聚合 resources/templates/list 全部分页。
    ///
    /// # Errors
    /// 请求失败、响应非法、server 返回 error 或 cursor 重复时返回错误。
    pub fn list_streamable_http_resource_templates_with_pagination(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
    ) -> AxonResult<McpStreamableHttpPagedResourceTemplatesReport> {
        let url = url.into();
        let mut session = StreamableHttpSession::initialize(&url, extra_headers, auth_config)?;
        let mut cursor = None::<String>;
        let mut seen_cursors = std::collections::BTreeSet::new();
        let mut templates = Vec::new();
        let mut requests_executed = 0;
        loop {
            let request = paged_request(
                &url,
                MCP_RESOURCE_TEMPLATES_LIST_ID,
                MCP_RESOURCE_TEMPLATES_LIST_METHOD,
                cursor.as_deref(),
            )?;
            let response_text = session.post_request(
                &request,
                extra_headers,
                auth_config,
                Some(mcp_tool_timeout()),
            )?;
            let value = parse_http_response_json(&response_text)?;
            if value.get("error").is_some() {
                return Err(AxonError::tool_invalid_definition(
                    "MCP resources/templates/list 返回 error",
                ));
            }
            templates.extend(parse_resource_templates_list_response(&value)?);
            requests_executed += 1;
            let Some(next_cursor) = next_cursor(&value) else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(AxonError::InvalidMcpDefinition(
                    "MCP resources/templates/list 返回重复 cursor",
                ));
            }
            cursor = Some(next_cursor);
        }
        Ok(McpStreamableHttpPagedResourceTemplatesReport {
            templates,
            requests_executed,
            reasons: streamable_http_reasons(),
        })
    }

    /// 通过 Streamable HTTP 执行 resources/read。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn read_streamable_http_resource(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        uri: &str,
    ) -> AxonResult<McpStreamableHttpResourceReadReport> {
        let url = url.into();
        let request = streamable_http_request(
            &url,
            MCP_RESOURCES_READ_ID,
            MCP_RESOURCES_READ_METHOD,
            serde_json::json!({
                "uri": uri,
                MCP_META_KEY: mcp_meta_value(),
            }),
        )?;
        let value = send_streamable_http_request(&url, extra_headers, auth_config, &request)?;
        Ok(McpStreamableHttpResourceReadReport {
            contents: parse_resources_read_response(&value)?,
            request,
            reasons: streamable_http_reasons(),
        })
    }

    /// 通过 Streamable HTTP 执行 resources/subscribe。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn subscribe_streamable_http_resource(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        uri: &str,
    ) -> AxonResult<McpStreamableHttpResourceSubscriptionReport> {
        resource_subscription_request(
            url,
            extra_headers,
            auth_config,
            uri,
            MCP_RESOURCES_SUBSCRIBE_ID,
            MCP_RESOURCES_SUBSCRIBE_METHOD,
            "subscribe",
        )
    }

    /// 通过 Streamable HTTP 执行 resources/unsubscribe。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn unsubscribe_streamable_http_resource(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        uri: &str,
    ) -> AxonResult<McpStreamableHttpResourceSubscriptionReport> {
        resource_subscription_request(
            url,
            extra_headers,
            auth_config,
            uri,
            MCP_RESOURCES_UNSUBSCRIBE_ID,
            MCP_RESOURCES_UNSUBSCRIBE_METHOD,
            "unsubscribe",
        )
    }
}

fn resource_subscription_request(
    url: impl Into<String>,
    extra_headers: &[crate::McpServerHeader],
    auth_config: &McpHttpAuthConfig,
    uri: &str,
    id: u64,
    method: &'static str,
    action: &'static str,
) -> AxonResult<McpStreamableHttpResourceSubscriptionReport> {
    let url = url.into();
    let request = streamable_http_request(
        &url,
        id,
        method,
        serde_json::json!({
            "uri": uri,
            MCP_META_KEY: mcp_meta_value(),
        }),
    )?;
    let mut session = StreamableHttpSession::initialize(&url, extra_headers, auth_config)?;
    let response_text = session.post_request(
        &request,
        extra_headers,
        auth_config,
        Some(mcp_tool_timeout()),
    )?;
    let value = parse_http_response_json(&response_text)?;
    if value.get("error").is_some() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP resource 订阅请求返回 error",
        ));
    }
    Ok(McpStreamableHttpResourceSubscriptionReport {
        uri: uri.to_owned(),
        action,
        request,
        notification_summary: crate::McpNotificationSummary::from_events(
            &crate::mcp_notifications::notification_events_from_text(&response_text),
        ),
        reasons: streamable_http_reasons(),
    })
}

fn paged_request(
    url: &str,
    id: u64,
    method: &str,
    cursor: Option<&str>,
) -> AxonResult<McpHttpRequestPlan> {
    let params = match cursor {
        Some(cursor) => serde_json::json!({
            "cursor": cursor,
            MCP_META_KEY: mcp_meta_value(),
        }),
        None => serde_json::json!({ MCP_META_KEY: mcp_meta_value() }),
    };
    streamable_http_request(url, id, method, params)
}

fn next_cursor(value: &Value) -> Option<String> {
    value
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn parse_resources_list_response(value: &Value) -> AxonResult<Vec<McpResourceSummary>> {
    let resources = value
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

pub(crate) fn parse_resource_templates_list_response(
    value: &Value,
) -> AxonResult<Vec<McpResourceTemplateSummary>> {
    let templates = value
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

pub(crate) fn parse_resources_read_response(value: &Value) -> AxonResult<Vec<McpResourceContent>> {
    let contents = value
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
