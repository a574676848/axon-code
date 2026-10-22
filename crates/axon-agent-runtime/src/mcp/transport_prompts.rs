//! MCP Streamable HTTP prompt wire API。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::mcp_transport::{
    optional_string, required_or_fallback, send_streamable_http_request, streamable_http_reasons,
    streamable_http_request,
};
use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::mcp_transport_http::{
    MCP_PROMPTS_GET_ID, MCP_PROMPTS_GET_METHOD, MCP_PROMPTS_LIST_ID, MCP_PROMPTS_LIST_METHOD,
    mcp_meta_value,
};
use crate::{McpHttpRequestPlan, McpTransportRuntime};

const MCP_META_KEY: &str = "_meta";

/// MCP prompt 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptSummary {
    /// Prompt 名称。
    pub name: String,
    /// 展示标题。
    pub title: Option<String>,
    /// 描述。
    pub description: Option<String>,
    /// 参数定义。
    pub arguments: Vec<McpPromptArgument>,
}

/// MCP prompt 参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptArgument {
    /// 参数名。
    pub name: String,
    /// 展示标题。
    pub title: Option<String>,
    /// 描述。
    pub description: Option<String>,
    /// 是否必填。
    pub required: bool,
}

/// MCP prompt message。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptMessage {
    /// MCP role。
    pub role: String,
    /// 内容类型。
    pub content_type: String,
    /// 文本内容。
    pub text: Option<String>,
    /// Resource URI。
    pub resource_uri: Option<String>,
}

/// Streamable HTTP prompts/list 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpPromptsReport {
    /// server 返回的 prompt 摘要。
    pub prompts: Vec<McpPromptSummary>,
    /// 下一页 cursor。
    pub next_cursor: Option<String>,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 已执行请求数量。
    pub requests_executed: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Streamable HTTP prompts/get 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStreamableHttpPromptGetReport {
    /// Prompt 描述。
    pub description: Option<String>,
    /// Prompt messages。
    pub messages: Vec<McpPromptMessage>,
    /// HTTP 请求计划。
    pub request: McpHttpRequestPlan,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

impl McpTransportRuntime {
    /// 通过 Streamable HTTP 执行 prompts/list。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn list_streamable_http_prompts(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        cursor: Option<&str>,
    ) -> AxonResult<McpStreamableHttpPromptsReport> {
        let url = url.into();
        let request = prompts_list_request(&url, cursor)?;
        let value = send_streamable_http_request(&url, extra_headers, auth_config, &request)?;
        Ok(McpStreamableHttpPromptsReport {
            prompts: parse_prompts_list_response(&value)?,
            next_cursor: next_cursor(&value),
            request,
            requests_executed: 1,
            reasons: streamable_http_reasons(),
        })
    }

    /// 通过 Streamable HTTP 执行 prompts/get。
    ///
    /// # Errors
    /// 请求失败、响应非法或 server 返回 error 时返回错误。
    pub fn get_streamable_http_prompt(
        url: impl Into<String>,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        name: &str,
        arguments: Value,
    ) -> AxonResult<McpStreamableHttpPromptGetReport> {
        let url = url.into();
        let request = streamable_http_request(
            &url,
            MCP_PROMPTS_GET_ID,
            MCP_PROMPTS_GET_METHOD,
            serde_json::json!({
                "name": name,
                "arguments": arguments,
                MCP_META_KEY: mcp_meta_value(),
            }),
        )?;
        let value = send_streamable_http_request(&url, extra_headers, auth_config, &request)?;
        Ok(McpStreamableHttpPromptGetReport {
            description: value
                .get("result")
                .and_then(|result| optional_string(result, "description")),
            messages: parse_prompt_get_response(&value)?,
            request,
            reasons: streamable_http_reasons(),
        })
    }
}

fn prompts_list_request(url: &str, cursor: Option<&str>) -> AxonResult<McpHttpRequestPlan> {
    let params = match cursor {
        Some(cursor) => serde_json::json!({
            "cursor": cursor,
            MCP_META_KEY: mcp_meta_value(),
        }),
        None => serde_json::json!({ MCP_META_KEY: mcp_meta_value() }),
    };
    streamable_http_request(url, MCP_PROMPTS_LIST_ID, MCP_PROMPTS_LIST_METHOD, params)
}

fn next_cursor(value: &Value) -> Option<String> {
    value
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn parse_prompts_list_response(value: &Value) -> AxonResult<Vec<McpPromptSummary>> {
    let prompts = value
        .get("result")
        .and_then(|result| result.get("prompts"))
        .and_then(Value::as_array)
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP prompts/list 响应缺少 prompts",
        ))?;
    Ok(prompts.iter().filter_map(prompt_summary).collect())
}

pub(crate) fn parse_prompt_get_response(value: &Value) -> AxonResult<Vec<McpPromptMessage>> {
    let messages = value
        .get("result")
        .and_then(|result| result.get("messages"))
        .and_then(Value::as_array)
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP prompts/get 响应缺少 messages",
        ))?;
    Ok(messages.iter().filter_map(prompt_message).collect())
}

fn prompt_summary(prompt: &Value) -> Option<McpPromptSummary> {
    Some(McpPromptSummary {
        name: prompt.get("name").and_then(Value::as_str)?.to_owned(),
        title: optional_string(prompt, "title"),
        description: optional_string(prompt, "description"),
        arguments: prompt
            .get("arguments")
            .and_then(Value::as_array)
            .map(|arguments| {
                arguments
                    .iter()
                    .filter_map(prompt_argument)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    })
}

fn prompt_argument(argument: &Value) -> Option<McpPromptArgument> {
    Some(McpPromptArgument {
        name: argument.get("name").and_then(Value::as_str)?.to_owned(),
        title: optional_string(argument, "title"),
        description: optional_string(argument, "description"),
        required: argument
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn prompt_message(message: &Value) -> Option<McpPromptMessage> {
    let content = message.get("content")?;
    let content_type = required_or_fallback(content, "type", "mimeType")?;
    Some(McpPromptMessage {
        role: message.get("role").and_then(Value::as_str)?.to_owned(),
        text: optional_string(content, "text"),
        resource_uri: content
            .get("resource")
            .and_then(|resource| resource.get("uri"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        content_type,
    })
}
