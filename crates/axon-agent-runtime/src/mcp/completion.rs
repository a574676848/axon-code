//! 外部 MCP completion 产品化 runtime。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult};
use serde_json::{Map, Value, json};

use crate::mcp_transport_auth::McpHttpAuthConfig;
use crate::{
    McpCompletionProjection, McpServerCommand, McpServerConfig, McpStdioExchangeRuntime,
    McpTransportRuntime,
};

/// MCP completion 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCompletionRequest {
    /// MCP server 配置。
    pub config: McpServerConfig,
    /// Completion ref。
    pub reference: McpCompletionReference,
    /// 参数名。
    pub argument_name: String,
    /// 已输入的参数值前缀。
    pub value: String,
    /// 已知上下文参数。
    pub context_arguments: BTreeMap<String, String>,
}

/// MCP completion ref。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCompletionReference {
    /// Prompt 参数补全。
    Prompt {
        /// Prompt 名称。
        name: String,
    },
    /// Resource URI/template 补全。
    Resource {
        /// Resource URI 或 URI template。
        uri: String,
    },
}

/// MCP completion 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCompletionRuntimeProjection {
    /// completion 安全投影。
    pub completion: McpCompletionProjection,
    /// transport 类型。
    pub transport: McpCompletionTransport,
}

/// MCP completion transport 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCompletionTransport {
    /// stdio。
    Stdio,
    /// Streamable HTTP。
    StreamableHttp,
}

/// MCP completion runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpCompletionRuntime;

impl McpCompletionRuntime {
    /// 执行 MCP completion/complete。
    ///
    /// # Errors
    /// MCP server 启动、握手、请求或响应解析失败时返回错误。
    pub fn complete(request: McpCompletionRequest) -> AxonResult<McpCompletionRuntimeProjection> {
        let params = completion_params(
            &request.reference,
            &request.argument_name,
            &request.value,
            request.context_arguments,
        );
        match request.config {
            McpServerConfig::Stdio(command) => complete_stdio(&command, params),
            McpServerConfig::StreamableHttp(http) => {
                let auth_config = McpHttpAuthConfig::from_http_server(&http);
                let report = McpTransportRuntime::complete_streamable_http(
                    http.url,
                    &http.headers,
                    &auth_config,
                    params,
                )?;
                Ok(McpCompletionRuntimeProjection {
                    completion: report.completion,
                    transport: McpCompletionTransport::StreamableHttp,
                })
            }
        }
    }
}

fn complete_stdio(
    command: &McpServerCommand,
    params: Value,
) -> AxonResult<McpCompletionRuntimeProjection> {
    let responses = McpStdioExchangeRuntime::exchange(
        command,
        vec![stdio_request(
            crate::mcp_transport_http::MCP_COMPLETION_COMPLETE_ID,
            crate::mcp_transport_http::MCP_COMPLETION_COMPLETE_METHOD,
            params,
        )?],
    )?;
    let response = responses
        .iter()
        .find(|value| {
            value.get("id").and_then(Value::as_u64)
                == Some(crate::mcp_transport_http::MCP_COMPLETION_COMPLETE_ID)
        })
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP completion/complete 响应缺少目标 id",
        ))?;
    Ok(McpCompletionRuntimeProjection {
        completion: crate::mcp_transport_completion::parse_completion_response(response)?,
        transport: McpCompletionTransport::Stdio,
    })
}

fn completion_params(
    reference: &McpCompletionReference,
    argument_name: &str,
    value: &str,
    context_arguments: BTreeMap<String, String>,
) -> Value {
    let reference = match reference {
        McpCompletionReference::Prompt { name } => {
            json!({ "type": "ref/prompt", "name": name })
        }
        McpCompletionReference::Resource { uri } => {
            json!({ "type": "ref/resource", "uri": uri })
        }
    };
    json!({
        "ref": reference,
        "argument": {
            "name": argument_name,
            "value": value,
        },
        "context": {
            "arguments": arguments_value(context_arguments),
        }
    })
}

fn stdio_request(id: u64, method: &str, params: Value) -> AxonResult<String> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|_| AxonError::InvalidMcpDefinition("MCP completion 请求序列化失败"))
}

fn arguments_value(arguments: BTreeMap<String, String>) -> Value {
    Value::Object(
        arguments
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect::<Map<_, _>>(),
    )
}
