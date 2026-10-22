//! MCP transport 生命周期与错误隔离规划。

use std::time::Duration;

use axon_domain::{AxonError, AxonResult};
use serde_json::{Map, Value};

use crate::mcp_notifications::{notification_event_from_value, notification_events_from_text};
use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::mcp_transport_http::{
    HTTP_POST_METHOD, MCP_INITIALIZE_ID, MCP_INITIALIZE_METHOD, MCP_TOOLS_CALL_ID,
    MCP_TOOLS_LIST_ID, MCP_TOOLS_LIST_METHOD, StreamableHttpSession, initialize_request_payload,
    json_rpc_request, mcp_meta_json, mcp_meta_value, mcp_tool_timeout, request_meta_json,
    rpc_request_payload, streamable_http_headers,
};

/// MCP 协议版本。
pub const MCP_TRANSPORT_PROTOCOL_VERSION: &str = "2025-11-25";
/// MCP JSON-RPC 版本。
pub const MCP_TRANSPORT_JSONRPC_VERSION: &str = "2.0";
/// MCP Streamable HTTP POST Accept。
pub const MCP_TRANSPORT_ACCEPT: &str = "application/json, text/event-stream";
/// MCP JSON Content-Type。
pub const MCP_TRANSPORT_JSON_CONTENT_TYPE: &str = "application/json";
/// MCP protocol version header。
pub const MCP_TRANSPORT_PROTOCOL_HEADER: &str = "MCP-Protocol-Version";
/// MCP method metadata header。
pub const MCP_TRANSPORT_METHOD_HEADER: &str = "Mcp-Method";
/// MCP name metadata header。
pub const MCP_TRANSPORT_NAME_HEADER: &str = "Mcp-Name";
/// stdio transport 已隔离到本机进程。
pub const MCP_TRANSPORT_REASON_STDIO_LOCAL_ONLY: &str = "mcp_transport:stdio_local_only";
/// Streamable HTTP 请求已规划。
pub const MCP_TRANSPORT_REASON_STREAMABLE_HTTP_PLANNED: &str =
    "mcp_transport:streamable_http_planned";
/// transport 错误已隔离。
pub const MCP_TRANSPORT_REASON_ERROR_ISOLATED: &str = "mcp_transport:error_isolated";
/// 远控触发保持关闭。
pub const MCP_TRANSPORT_REASON_REMOTE_TRIGGER_CLOSED: &str = "mcp_transport:remote_trigger_closed";
/// initialize 能力协商已规划。
pub const MCP_TRANSPORT_REASON_INITIALIZE_PLANNED: &str = "mcp_transport:initialize_planned";
/// Streamable HTTP 会话 header 已按官方协议接入。
pub const MCP_TRANSPORT_REASON_SESSION_HEADER_MANAGED: &str =
    "mcp_transport:session_header_managed";

const MCP_META_KEY: &str = "_meta";

/// MCP transport 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportKind {
    /// 本机 stdio 进程。
    Stdio,
    /// Streamable HTTP。
    StreamableHttp,
}

/// MCP lifecycle 阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpLifecyclePhase {
    /// 已发现配置。
    Discovered,
    /// initialize 已规划。
    Initializing,
    /// 会话运行中。
    Running,
    /// transport 错误已隔离。
    ErrorIsolated,
}

/// MCP transport 错误类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportErrorKind {
    /// 鉴权失败。
    Unauthorized,
    /// server 不支持请求内容类型或 stream。
    UnsupportedContentType,
    /// 非 JSON 或非 SSE 响应。
    InvalidResponse,
    /// server 侧错误。
    ServerError,
}

/// MCP HTTP header。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpHttpHeader {
    /// Header 名称。
    pub name: String,
    /// Header 值。
    pub value: String,
}

/// MCP JSON-RPC 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpJsonRpcRequestPlan {
    /// JSON-RPC ID。
    pub id: u64,
    /// 方法名。
    pub method: String,
    /// JSON-RPC payload。
    pub payload_json: String,
}

/// MCP HTTP 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpHttpRequestPlan {
    /// HTTP 方法。
    pub method: String,
    /// URL。
    pub url: String,
    /// Header。
    pub headers: Vec<McpHttpHeader>,
    /// JSON-RPC 请求体。
    pub body_json: String,
    /// 是否为流式响应。
    pub stream_response: bool,
}

/// MCP transport 生命周期计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpTransportLifecyclePlan {
    /// transport 类型。
    pub transport_kind: McpTransportKind,
    /// 当前阶段。
    pub phase: McpLifecyclePhase,
    /// initialize 请求。
    pub initialize_request: McpJsonRpcRequestPlan,
    /// tools/list 请求。
    pub tools_list_request: McpJsonRpcRequestPlan,
    /// HTTP 请求。
    pub http_request: Option<McpHttpRequestPlan>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// terminal 主循环是否继续。
    pub terminal_main_loop_continues: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// MCP transport 错误隔离报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpTransportErrorIsolationReport {
    /// transport 类型。
    pub transport_kind: McpTransportKind,
    /// 错误类别。
    pub error_kind: McpTransportErrorKind,
    /// HTTP 状态码。
    pub http_status: Option<u16>,
    /// 当前阶段。
    pub phase: McpLifecyclePhase,
    /// 是否建议本机重启或重新协商。
    pub restart_recommended: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// terminal 主循环是否继续。
    pub terminal_main_loop_continues: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP tools/list 报告。
#[derive(Debug, Clone, PartialEq)]
pub struct McpStreamableHttpToolsReport {
    /// server 返回的 tool schema 摘要。
    pub tool_schemas: Vec<crate::McpToolSchema>,
    /// 下一页 cursor。
    pub next_cursor: Option<String>,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
    /// MCP server 通知摘要。
    pub notification_summary: crate::McpNotificationSummary,
}

/// Streamable HTTP tools/list 分页报告。
#[derive(Debug, Clone, PartialEq)]
pub struct McpStreamableHttpPagedToolsReport {
    /// server 返回的所有 tool schema 摘要。
    pub tool_schemas: Vec<crate::McpToolSchema>,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
    /// MCP server 通知摘要。
    pub notification_summary: crate::McpNotificationSummary,
}

/// Streamable HTTP tools/call 报告。
#[derive(Debug, Clone, PartialEq)]
pub struct McpStreamableHttpCallReport {
    /// server 返回的 result 字段。
    pub result_text: String,
    /// server 返回的 result 安全投影。
    pub result_projection: McpToolCallResultProjection,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 可审计原因。
    pub reasons: Vec<String>,
    /// MCP server 通知摘要。
    pub notification_summary: crate::McpNotificationSummary,
}

/// MCP tools/call result 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolCallResultProjection {
    /// server 是否声明调用失败。
    pub is_error: bool,
    /// result.content 的内容类型摘要。
    pub content_types: Vec<String>,
    /// 是否返回 structuredContent。
    pub structured_content_declared: bool,
    /// structuredContent 顶层字段摘要。
    pub structured_content_keys: Vec<String>,
    /// 是否暴露原始 result payload。
    pub raw_payload_exposed: bool,
}

/// MCP transport runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpTransportRuntime;

impl McpTransportRuntime {
    /// 规划 stdio transport initialize / tools/list 生命周期。
    #[must_use]
    pub fn plan_stdio_lifecycle(expected_tools: &[String]) -> McpTransportLifecyclePlan {
        McpTransportLifecyclePlan {
            transport_kind: McpTransportKind::Stdio,
            phase: McpLifecyclePhase::Initializing,
            initialize_request: initialize_request_plan(),
            tools_list_request: tools_list_request_plan(expected_tools),
            http_request: None,
            remote_trigger_allowed: false,
            terminal_main_loop_continues: true,
            reasons: vec![
                MCP_TRANSPORT_REASON_INITIALIZE_PLANNED.to_owned(),
                MCP_TRANSPORT_REASON_STDIO_LOCAL_ONLY.to_owned(),
                MCP_TRANSPORT_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
            ],
        }
    }

    /// 规划现代 Streamable HTTP POST 请求。
    #[must_use]
    pub fn plan_streamable_http_request(
        url: impl Into<String>,
        method: impl Into<String>,
        name: impl Into<String>,
    ) -> McpTransportLifecyclePlan {
        let method = method.into();
        let name = name.into();
        let headers = streamable_http_headers(&method, &name);

        McpTransportLifecyclePlan {
            transport_kind: McpTransportKind::StreamableHttp,
            phase: McpLifecyclePhase::Running,
            initialize_request: initialize_request_plan(),
            tools_list_request: tools_list_request_plan(&[name]),
            http_request: Some(McpHttpRequestPlan {
                method: HTTP_POST_METHOD.to_owned(),
                url: url.into(),
                headers,
                body_json: rpc_request_payload(MCP_TOOLS_LIST_ID, &method, &request_meta_json()),
                stream_response: true,
            }),
            remote_trigger_allowed: false,
            terminal_main_loop_continues: true,
            reasons: vec![
                MCP_TRANSPORT_REASON_STREAMABLE_HTTP_PLANNED.to_owned(),
                MCP_TRANSPORT_REASON_SESSION_HEADER_MANAGED.to_owned(),
                MCP_TRANSPORT_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
            ],
        }
    }

    /// 通过 Streamable HTTP 执行 tools/list。
    ///
    /// # Errors
    /// 请求失败、状态码异常或响应不是 MCP JSON-RPC tools/list 结果时返回错误。
    pub fn list_streamable_http_tools(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        cursor: Option<&str>,
    ) -> AxonResult<McpStreamableHttpToolsReport> {
        let url = url.into();
        let request = tools_list_http_request(&url, cursor)?;
        let mut session = StreamableHttpSession::initialize(&url, extra_headers, auth_config)?;
        let response_text = session.post_request(
            &request,
            extra_headers,
            auth_config,
            Some(mcp_tool_timeout()),
        )?;
        let notification_summary = crate::McpNotificationSummary::from_events(
            &notification_events_from_text(&response_text),
        );
        let value = parse_streamable_http_response_json(&response_text)?;
        if value.get("error").is_some() {
            return Err(AxonError::tool_invalid_definition(
                "MCP tools/list 返回 error",
            ));
        }
        Ok(McpStreamableHttpToolsReport {
            tool_schemas: parse_tools_list_response(&value)?,
            next_cursor: next_cursor(&value),
            request,
            requests_executed: 1,
            reasons: streamable_http_reasons(),
            notification_summary,
        })
    }

    /// 通过 Streamable HTTP 聚合 tools/list 全部分页。
    ///
    /// # Errors
    /// 请求失败、响应非法、server 返回 error 或 cursor 重复时返回错误。
    pub fn list_streamable_http_tools_with_pagination(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
    ) -> AxonResult<McpStreamableHttpPagedToolsReport> {
        let url = url.into();
        let mut session = StreamableHttpSession::initialize(&url, extra_headers, auth_config)?;
        let mut cursor = None::<String>;
        let mut seen_cursors = std::collections::BTreeSet::new();
        let mut tool_schemas = Vec::new();
        let mut notifications = Vec::new();
        let mut requests_executed = 0;
        loop {
            let request = tools_list_http_request(&url, cursor.as_deref())?;
            let response_text = session.post_request(
                &request,
                extra_headers,
                auth_config,
                Some(mcp_tool_timeout()),
            )?;
            notifications.extend(notification_events_from_text(&response_text));
            let value = parse_streamable_http_response_json(&response_text)?;
            if value.get("error").is_some() {
                return Err(AxonError::tool_invalid_definition(
                    "MCP tools/list 返回 error",
                ));
            }
            tool_schemas.extend(parse_tools_list_response(&value)?);
            requests_executed += 1;
            let Some(next_cursor) = next_cursor(&value) else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(AxonError::InvalidMcpDefinition(
                    "MCP tools/list 返回重复 cursor",
                ));
            }
            cursor = Some(next_cursor);
        }
        Ok(McpStreamableHttpPagedToolsReport {
            tool_schemas,
            requests_executed,
            reasons: streamable_http_reasons(),
            notification_summary: crate::McpNotificationSummary::from_events(&notifications),
        })
    }

    /// 通过 Streamable HTTP 执行 tools/call。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn call_streamable_http_tool(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        tool_name: &str,
        arguments_json: &str,
        timeout: Option<Duration>,
    ) -> AxonResult<McpStreamableHttpCallReport> {
        let arguments = if arguments_json.trim().is_empty() {
            Value::Object(Map::default())
        } else {
            serde_json::from_str::<Value>(arguments_json)
                .map_err(|_| AxonError::tool_invalid_definition("MCP tool arguments 必须是 JSON"))?
        };
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
            "_meta": mcp_meta_value(),
        });
        let url = url.into();
        let request =
            streamable_http_request(&url, MCP_TOOLS_CALL_ID, crate::MCP_TOOL_CALL_METHOD, params)?;
        let mut session = StreamableHttpSession::initialize(&url, extra_headers, auth_config)?;
        let response_text = session.post_request(&request, extra_headers, auth_config, timeout)?;
        let notification_summary = crate::McpNotificationSummary::from_events(
            &notification_events_from_text(&response_text),
        );
        let value = parse_streamable_http_response_json(&response_text)?;
        if value.get("error").is_some() {
            return Err(AxonError::tool_invalid_definition(
                "MCP tools/call 返回 error",
            ));
        }
        let result_text = value
            .get("result")
            .map(ToString::to_string)
            .unwrap_or_default();
        let result_projection = tool_call_result_projection(value.get("result"));
        Ok(McpStreamableHttpCallReport {
            result_text,
            result_projection,
            request,
            reasons: streamable_http_reasons(),
            notification_summary,
        })
    }

    /// 隔离 transport 错误，确保终端主循环继续。
    #[must_use]
    pub fn isolate_error(
        transport_kind: McpTransportKind,
        http_status: Option<u16>,
        response_content_type: Option<&str>,
    ) -> McpTransportErrorIsolationReport {
        let error_kind = classify_transport_error(http_status, response_content_type);
        McpTransportErrorIsolationReport {
            transport_kind,
            error_kind,
            http_status,
            phase: McpLifecyclePhase::ErrorIsolated,
            restart_recommended: matches!(error_kind, McpTransportErrorKind::ServerError),
            remote_trigger_allowed: false,
            terminal_main_loop_continues: true,
            reasons: vec![
                MCP_TRANSPORT_REASON_ERROR_ISOLATED.to_owned(),
                MCP_TRANSPORT_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
            ],
        }
    }
}

pub(crate) fn tool_call_result_projection(result: Option<&Value>) -> McpToolCallResultProjection {
    let result = result.unwrap_or(&Value::Null);
    let content_types = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|content| content.get("type").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let structured_content = result.get("structuredContent");
    let mut structured_content_keys = structured_content
        .and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    structured_content_keys.sort();
    McpToolCallResultProjection {
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        content_types,
        structured_content_declared: structured_content.is_some(),
        structured_content_keys,
        raw_payload_exposed: false,
    }
}

fn initialize_request_plan() -> McpJsonRpcRequestPlan {
    McpJsonRpcRequestPlan {
        id: MCP_INITIALIZE_ID,
        method: MCP_INITIALIZE_METHOD.to_owned(),
        payload_json: initialize_request_payload(),
    }
}

fn tools_list_request_plan(expected_tools: &[String]) -> McpJsonRpcRequestPlan {
    let tools = expected_tools
        .iter()
        .map(|tool| format!(r#"{{"name":"{}"}}"#, escape_json(tool)))
        .collect::<Vec<_>>()
        .join(",");
    McpJsonRpcRequestPlan {
        id: MCP_TOOLS_LIST_ID,
        method: MCP_TOOLS_LIST_METHOD.to_owned(),
        payload_json: rpc_request_payload(
            MCP_TOOLS_LIST_ID,
            MCP_TOOLS_LIST_METHOD,
            &format!(
                r#"{{"expectedTools":[{tools}],"{MCP_META_KEY}":{}}}"#,
                mcp_meta_json()
            ),
        ),
    }
}

pub(crate) fn tools_list_http_request(
    url: &str,
    cursor: Option<&str>,
) -> AxonResult<McpHttpRequestPlan> {
    let params = match cursor {
        Some(cursor) => serde_json::json!({
            "cursor": cursor,
            MCP_META_KEY: mcp_meta_value(),
        }),
        None => serde_json::json!({ MCP_META_KEY: mcp_meta_value() }),
    };
    streamable_http_request(url, MCP_TOOLS_LIST_ID, MCP_TOOLS_LIST_METHOD, params)
}

fn classify_transport_error(
    http_status: Option<u16>,
    response_content_type: Option<&str>,
) -> McpTransportErrorKind {
    match http_status {
        Some(401 | 403) => McpTransportErrorKind::Unauthorized,
        Some(404) => McpTransportErrorKind::InvalidResponse,
        Some(415) => McpTransportErrorKind::UnsupportedContentType,
        Some(status) if status >= 500 => McpTransportErrorKind::ServerError,
        _ if !content_type_supported(response_content_type) => {
            McpTransportErrorKind::InvalidResponse
        }
        _ => McpTransportErrorKind::InvalidResponse,
    }
}

fn content_type_supported(response_content_type: Option<&str>) -> bool {
    response_content_type.is_some_and(|content_type| {
        content_type.starts_with("text/event-stream")
            || content_type.starts_with(MCP_TRANSPORT_JSON_CONTENT_TYPE)
    })
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn streamable_http_request(
    url: impl Into<String>,
    id: u64,
    method: &str,
    params: Value,
) -> AxonResult<McpHttpRequestPlan> {
    json_rpc_request(&url.into(), id, method, params)
}

pub(crate) fn send_streamable_http_request(
    url: &str,
    extra_headers: &[crate::McpServerHeader],
    auth_config: &McpHttpAuthConfig,
    request: &McpHttpRequestPlan,
) -> AxonResult<Value> {
    let mut session = StreamableHttpSession::initialize(url, extra_headers, auth_config)?;
    let response_text = session.post_request(
        request,
        extra_headers,
        auth_config,
        Some(mcp_tool_timeout()),
    )?;
    let value = parse_streamable_http_response_json(&response_text)?;
    if value.get("error").is_some() {
        return Err(AxonError::tool_invalid_definition(
            "MCP resources 返回 error",
        ));
    }
    Ok(value)
}

pub(crate) fn streamable_http_reasons() -> Vec<String> {
    vec![
        MCP_TRANSPORT_REASON_INITIALIZE_PLANNED.to_owned(),
        MCP_TRANSPORT_REASON_STREAMABLE_HTTP_PLANNED.to_owned(),
        MCP_TRANSPORT_REASON_SESSION_HEADER_MANAGED.to_owned(),
        MCP_TRANSPORT_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
    ]
}

fn parse_tools_list_response(value: &Value) -> AxonResult<Vec<crate::McpToolSchema>> {
    let tools = value
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP tools/list 响应缺少 tools",
        ))?;
    Ok(tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Some(crate::McpToolSchema {
                name: name.to_owned(),
                mcp_name: name.to_owned(),
                description: description.to_owned(),
                input_schema: tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(crate::mcp_tools::empty_mcp_input_schema),
                output_schema: tool.get("outputSchema").cloned(),
                annotations: tool_annotations(tool),
            })
        })
        .collect())
}

fn next_cursor(value: &Value) -> Option<String> {
    value
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn tool_annotations(value: &Value) -> crate::McpToolAnnotations {
    let annotations = value.get("annotations");
    crate::McpToolAnnotations {
        read_only_hint: annotation_bool(annotations, "readOnlyHint"),
        destructive_hint: annotation_bool(annotations, "destructiveHint"),
        open_world_hint: annotation_bool(annotations, "openWorldHint"),
    }
}

fn annotation_bool(annotations: Option<&Value>, key: &str) -> Option<bool> {
    annotations
        .and_then(|value| value.get(key))
        .and_then(Value::as_bool)
}

pub(crate) fn required_or_fallback(value: &Value, field: &str, fallback: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .or_else(|| value.get(fallback).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

pub(crate) fn optional_string(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn parse_streamable_http_response_json(text: &str) -> AxonResult<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text)
        && notification_event_from_value(&value).is_none()
    {
        return Ok(value);
    }
    for line in text.lines() {
        let Some(payload) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload)
            && value.get("id").is_some()
            && notification_event_from_value(&value).is_none()
        {
            return Ok(value);
        }
    }
    Err(AxonError::InvalidMcpDefinition(
        "MCP Streamable HTTP 响应缺少业务结果",
    ))
}
