//! MCP completion/complete wire API。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::mcp_transport::{
    send_streamable_http_request, streamable_http_reasons, streamable_http_request,
};
use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::mcp_transport_http::{
    MCP_COMPLETION_COMPLETE_ID, MCP_COMPLETION_COMPLETE_METHOD, mcp_meta_value,
};
use crate::{McpHttpRequestPlan, McpTransportRuntime};

const MCP_META_KEY: &str = "_meta";

/// MCP completion 值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCompletionValue {
    /// 建议值。
    pub value: String,
}

/// MCP completion 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCompletionProjection {
    /// 建议值。
    pub values: Vec<McpCompletionValue>,
    /// server 声明的总数。
    pub total: Option<usize>,
    /// server 是否声明还有更多。
    pub has_more: Option<bool>,
    /// 是否暴露原始 payload。
    pub raw_payload_exposed: bool,
}

/// Streamable HTTP completion/complete 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpCompletionReport {
    /// completion 安全投影。
    pub completion: McpCompletionProjection,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

impl McpTransportRuntime {
    /// 通过 Streamable HTTP 执行 completion/complete。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn complete_streamable_http(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        params: Value,
    ) -> AxonResult<McpStreamableHttpCompletionReport> {
        let url = url.into();
        let request = streamable_http_request(
            &url,
            MCP_COMPLETION_COMPLETE_ID,
            MCP_COMPLETION_COMPLETE_METHOD,
            with_meta(params),
        )?;
        let value = send_streamable_http_request(&url, extra_headers, auth_config, &request)?;
        Ok(McpStreamableHttpCompletionReport {
            completion: parse_completion_response(&value)?,
            request,
            reasons: streamable_http_reasons(),
        })
    }
}

pub(crate) fn parse_completion_response(value: &Value) -> AxonResult<McpCompletionProjection> {
    let completion = value
        .get("result")
        .and_then(|result| result.get("completion"))
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP completion/complete 响应缺少 completion",
        ))?;
    let values = completion.get("values").and_then(Value::as_array).ok_or(
        AxonError::InvalidMcpDefinition("MCP completion/complete 响应缺少 values"),
    )?;
    Ok(McpCompletionProjection {
        values: values
            .iter()
            .filter_map(|value| value.as_str())
            .map(|value| McpCompletionValue {
                value: value.to_owned(),
            })
            .collect(),
        total: completion
            .get("total")
            .and_then(Value::as_u64)
            .and_then(|total| usize::try_from(total).ok()),
        has_more: completion.get("hasMore").and_then(Value::as_bool),
        raw_payload_exposed: false,
    })
}

fn with_meta(params: Value) -> Value {
    let mut params = params.as_object().cloned().unwrap_or_default();
    params.insert(MCP_META_KEY.to_owned(), mcp_meta_value());
    Value::Object(params)
}
