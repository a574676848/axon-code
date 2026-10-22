//! MCP Streamable HTTP 会话、JSON-RPC payload 与响应解析。

use std::time::Duration;

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::mcp_notifications::{McpNotificationEvent, notification_events_from_text};
use crate::mcp_transport_auth::{
    MCP_OAUTH_REQUIRED_FORBIDDEN_REASON, MCP_OAUTH_REQUIRED_INSUFFICIENT_SCOPE_REASON,
    MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON, McpHttpAuthConfig, authorization_header_from_env,
};
use crate::mcp_transport_http_client::shared_http_client;
use crate::{
    MCP_TRANSPORT_ACCEPT, MCP_TRANSPORT_JSON_CONTENT_TYPE, MCP_TRANSPORT_JSONRPC_VERSION,
    MCP_TRANSPORT_METHOD_HEADER, MCP_TRANSPORT_NAME_HEADER, MCP_TRANSPORT_PROTOCOL_HEADER,
    MCP_TRANSPORT_PROTOCOL_VERSION, McpClientCapabilitiesRuntime, McpHttpHeader,
    McpHttpRequestPlan,
};

pub(crate) const MCP_CLIENT_NAME: &str = "axon-mcp";
pub(crate) const MCP_CLIENT_VERSION: &str = "0.1.0";
pub(crate) const MCP_INITIALIZE_ID: u64 = 1;
pub(crate) const MCP_TOOLS_LIST_ID: u64 = 2;
pub(crate) const MCP_TOOLS_CALL_ID: u64 = 3;
pub(crate) const MCP_RESOURCES_LIST_ID: u64 = 4;
pub(crate) const MCP_RESOURCE_TEMPLATES_LIST_ID: u64 = 5;
pub(crate) const MCP_RESOURCES_READ_ID: u64 = 6;
pub(crate) const MCP_PROMPTS_LIST_ID: u64 = 7;
pub(crate) const MCP_PROMPTS_GET_ID: u64 = 8;
pub(crate) const MCP_COMPLETION_COMPLETE_ID: u64 = 9;
pub(crate) const MCP_RESOURCES_SUBSCRIBE_ID: u64 = 10;
pub(crate) const MCP_RESOURCES_UNSUBSCRIBE_ID: u64 = 11;
pub(crate) const MCP_INITIALIZE_METHOD: &str = "initialize";
pub(crate) const MCP_INITIALIZED_NOTIFICATION_METHOD: &str = "notifications/initialized";
pub(crate) const MCP_TOOLS_LIST_METHOD: &str = "tools/list";
pub(crate) const MCP_RESOURCES_LIST_METHOD: &str = "resources/list";
pub(crate) const MCP_RESOURCE_TEMPLATES_LIST_METHOD: &str = "resources/templates/list";
pub(crate) const MCP_RESOURCES_READ_METHOD: &str = "resources/read";
pub(crate) const MCP_RESOURCES_SUBSCRIBE_METHOD: &str = "resources/subscribe";
pub(crate) const MCP_RESOURCES_UNSUBSCRIBE_METHOD: &str = "resources/unsubscribe";
pub(crate) const MCP_PROMPTS_LIST_METHOD: &str = "prompts/list";
pub(crate) const MCP_PROMPTS_GET_METHOD: &str = "prompts/get";
pub(crate) const MCP_COMPLETION_COMPLETE_METHOD: &str = "completion/complete";
pub(crate) const HTTP_POST_METHOD: &str = "POST";
pub(crate) const MCP_META_KEY: &str = "_meta";
pub(crate) const MCP_META_PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
pub(crate) const MCP_META_CLIENT_INFO_KEY: &str = "io.modelcontextprotocol/clientInfo";

const HEADER_ACCEPT: &str = "Accept";
const HEADER_CONTENT_TYPE: &str = "Content-Type";
const HEADER_MCP_SESSION_ID: &str = "Mcp-Session-Id";
const MCP_HTTP_STATUS_UNAUTHORIZED: u16 = 401;
const MCP_HTTP_STATUS_FORBIDDEN: u16 = 403;
const MCP_HTTP_INSUFFICIENT_SCOPE_ERROR: &str = "insufficient_scope";
const MCP_HTTP_AUTH_ERROR_PARAM: &str = "error";
const MCP_HTTP_STARTUP_TIMEOUT_SECS: u64 = 30;
const MCP_HTTP_TOOL_TIMEOUT_SECS: u64 = 300;

#[derive(Debug, Default)]
pub(crate) struct StreamableHttpSession {
    session_id: Option<String>,
}

impl StreamableHttpSession {
    pub(crate) fn initialize_result_text(
        url: &str,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
    ) -> AxonResult<String> {
        let mut session = Self::default();
        let initialize = initialize_request(url);
        let response = session.post_request_with_metadata(
            &initialize,
            extra_headers,
            auth_config,
            Some(mcp_startup_timeout()),
        )?;
        let value = parse_http_response_json(&response.text)?;
        if value.get("error").is_some() || value.get("result").is_none() {
            return Err(AxonError::InvalidMcpDefinition("MCP initialize 响应无效"));
        }
        Ok(response.text)
    }

    pub(crate) fn initialize(
        url: &str,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
    ) -> AxonResult<Self> {
        let mut session = Self::default();
        let initialize = initialize_request(url);
        let response = session.post_request_with_metadata(
            &initialize,
            extra_headers,
            auth_config,
            Some(mcp_startup_timeout()),
        )?;
        let value = parse_http_response_json(&response.text)?;
        if value.get("error").is_some() || value.get("result").is_none() {
            return Err(AxonError::InvalidMcpDefinition("MCP initialize 响应无效"));
        }
        session.session_id = response.session_id;
        let initialized = McpHttpRequestPlan {
            method: HTTP_POST_METHOD.to_owned(),
            url: url.to_owned(),
            headers: streamable_http_headers(
                MCP_INITIALIZED_NOTIFICATION_METHOD,
                "notifications-initialized",
            ),
            body_json: rpc_notification_payload(
                MCP_INITIALIZED_NOTIFICATION_METHOD,
                &request_meta_json(),
            ),
            stream_response: false,
        };
        let _ = session.post_request_with_metadata(
            &initialized,
            extra_headers,
            auth_config,
            Some(mcp_startup_timeout()),
        )?;
        Ok(session)
    }

    pub(crate) fn post_request(
        &mut self,
        request: &McpHttpRequestPlan,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        timeout: Option<Duration>,
    ) -> AxonResult<String> {
        let response =
            self.post_request_with_metadata(request, extra_headers, auth_config, timeout)?;
        self.respond_to_denied_server_requests(
            &response.text,
            request,
            extra_headers,
            auth_config,
            timeout,
        )?;
        Ok(response.text)
    }

    fn post_request_with_metadata(
        &mut self,
        request: &McpHttpRequestPlan,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        timeout: Option<Duration>,
    ) -> AxonResult<StreamableHttpResponse> {
        // 复用进程级连接池与 TLS 会话；调用参数只覆盖本次超时。
        let client = shared_http_client()?;
        let mut builder = client.post(&request.url);
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        for header in &request.headers {
            builder = builder.header(&header.name, &header.value);
        }
        if let Some(session_id) = &self.session_id {
            builder = builder.header(HEADER_MCP_SESSION_ID, session_id);
        }
        for header in extra_headers {
            builder = builder.header(&header.name, &header.value);
        }
        if let Some(header) = authorization_header_from_env(auth_config)? {
            builder = builder.header(header.name, header.value);
        }
        let response = builder
            .body(request.body_json.clone())
            .send()
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP HTTP 请求失败"))?;
        if !response.status().is_success() {
            return Err(mcp_http_status_error(response.status(), response.headers()));
        }
        let session_id = response
            .headers()
            .get(HEADER_MCP_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);
        if session_id.is_some() {
            self.session_id.clone_from(&session_id);
        }
        let text = response
            .text()
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP HTTP 响应读取失败"))?;
        Ok(StreamableHttpResponse { text, session_id })
    }

    fn respond_to_denied_server_requests(
        &mut self,
        response_text: &str,
        source_request: &McpHttpRequestPlan,
        extra_headers: &[crate::McpServerHeader],
        auth_config: &McpHttpAuthConfig,
        timeout: Option<Duration>,
    ) -> AxonResult<()> {
        for event in notification_events_from_text(response_text) {
            let Some(body_json) = event.denial_response_json.as_deref() else {
                continue;
            };
            let request = denial_response_request(source_request, &event, body_json);
            let _ =
                self.post_request_with_metadata(&request, extra_headers, auth_config, timeout)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct StreamableHttpResponse {
    text: String,
    session_id: Option<String>,
}

/// 把非 2xx 的 MCP HTTP 状态码投影为可识别错误。
///
/// MCP 2026-07-28 要求客户端在 401 与 403 `insufficient_scope` 时进入 OAuth 授权闭环，
/// 因此这里必须保留“需要 OAuth 授权”语义，而不是把它降级为笼统的状态码异常。
fn mcp_http_status_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> AxonError {
    match status.as_u16() {
        MCP_HTTP_STATUS_UNAUTHORIZED => {
            AxonError::InvalidMcpDefinition(MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON)
        }
        MCP_HTTP_STATUS_FORBIDDEN => match www_authenticate_error(headers) {
            Some(error) if error == MCP_HTTP_INSUFFICIENT_SCOPE_ERROR => {
                AxonError::InvalidMcpDefinition(MCP_OAUTH_REQUIRED_INSUFFICIENT_SCOPE_REASON)
            }
            _ => AxonError::InvalidMcpDefinition(MCP_OAUTH_REQUIRED_FORBIDDEN_REASON),
        },
        _ => AxonError::InvalidMcpDefinition("MCP HTTP 状态码异常"),
    }
}

fn www_authenticate_error(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let challenge = headers
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    challenge.split([';', ',']).find_map(|part| {
        let (name, value) = part.split_once('=')?;
        // RFC 6750 允许首个参数带认证方案前缀（如 `Bearer error="insufficient_scope"`），
        // 因此取空白分隔后的最后一段作为参数名再比较。
        let name = name.rsplit(char::is_whitespace).next().unwrap_or(name);
        (name.trim().eq_ignore_ascii_case(MCP_HTTP_AUTH_ERROR_PARAM))
            .then(|| value.trim().trim_matches('"').trim().to_ascii_lowercase())
    })
}

fn initialize_request(url: &str) -> McpHttpRequestPlan {
    McpHttpRequestPlan {
        method: HTTP_POST_METHOD.to_owned(),
        url: url.to_owned(),
        headers: streamable_http_headers(MCP_INITIALIZE_METHOD, "initialize"),
        body_json: initialize_request_payload(),
        stream_response: true,
    }
}

fn denial_response_request(
    source_request: &McpHttpRequestPlan,
    event: &McpNotificationEvent,
    body_json: &str,
) -> McpHttpRequestPlan {
    McpHttpRequestPlan {
        method: HTTP_POST_METHOD.to_owned(),
        url: source_request.url.clone(),
        headers: streamable_http_headers(
            event
                .denied_method
                .as_deref()
                .unwrap_or("client/request_denied"),
            "client-request-denied",
        ),
        body_json: body_json.to_owned(),
        stream_response: false,
    }
}

pub(crate) fn streamable_http_headers(method: &str, name: &str) -> Vec<McpHttpHeader> {
    vec![
        header(HEADER_ACCEPT, MCP_TRANSPORT_ACCEPT),
        header(HEADER_CONTENT_TYPE, MCP_TRANSPORT_JSON_CONTENT_TYPE),
        header(
            MCP_TRANSPORT_PROTOCOL_HEADER,
            MCP_TRANSPORT_PROTOCOL_VERSION,
        ),
        header(MCP_TRANSPORT_METHOD_HEADER, method),
        header(MCP_TRANSPORT_NAME_HEADER, name),
    ]
}

pub(crate) fn header(name: impl Into<String>, value: impl Into<String>) -> McpHttpHeader {
    McpHttpHeader {
        name: name.into(),
        value: value.into(),
    }
}

pub(crate) fn mcp_tool_timeout() -> Duration {
    Duration::from_secs(MCP_HTTP_TOOL_TIMEOUT_SECS)
}

pub(crate) fn request_meta_json() -> String {
    format!(r#"{{"{MCP_META_KEY}":{}}}"#, mcp_meta_json())
}

pub(crate) fn mcp_meta_value() -> Value {
    serde_json::json!({
        MCP_META_PROTOCOL_VERSION_KEY: MCP_TRANSPORT_PROTOCOL_VERSION,
        MCP_META_CLIENT_INFO_KEY: {
            "name": MCP_CLIENT_NAME,
            "version": MCP_CLIENT_VERSION,
        }
    })
}

pub(crate) fn mcp_meta_json() -> String {
    format!(
        r#"{{"{MCP_META_PROTOCOL_VERSION_KEY}":"{MCP_TRANSPORT_PROTOCOL_VERSION}","{MCP_META_CLIENT_INFO_KEY}":{}}}"#,
        client_info_json()
    )
}

pub(crate) fn client_info_json() -> String {
    format!(r#"{{"name":"{MCP_CLIENT_NAME}","version":"{MCP_CLIENT_VERSION}"}}"#)
}

pub(crate) fn rpc_request_payload(id: u64, method: &str, params_json: &str) -> String {
    format!(
        r#"{{"jsonrpc":"{MCP_TRANSPORT_JSONRPC_VERSION}","id":{id},"method":"{method}","params":{params_json}}}"#
    )
}

pub(crate) fn rpc_notification_payload(method: &str, params_json: &str) -> String {
    format!(
        r#"{{"jsonrpc":"{MCP_TRANSPORT_JSONRPC_VERSION}","method":"{method}","params":{params_json}}}"#
    )
}

pub(crate) fn json_rpc_request(
    url: &str,
    id: u64,
    method: &str,
    params: Value,
) -> AxonResult<McpHttpRequestPlan> {
    let body_json = serde_json::to_string(&serde_json::json!({
        "jsonrpc": MCP_TRANSPORT_JSONRPC_VERSION,
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|_| AxonError::InvalidMcpDefinition("MCP HTTP 请求序列化失败"))?;
    Ok(McpHttpRequestPlan {
        method: HTTP_POST_METHOD.to_owned(),
        url: url.to_owned(),
        headers: streamable_http_headers(method, method),
        body_json,
        stream_response: true,
    })
}

pub(crate) fn initialize_request_payload() -> String {
    let params = serde_json::json!({
        "protocolVersion": MCP_TRANSPORT_PROTOCOL_VERSION,
        "capabilities": McpClientCapabilitiesRuntime::initialize_capabilities_value(),
        "clientInfo": {
            "name": MCP_CLIENT_NAME,
            "version": MCP_CLIENT_VERSION,
        },
        MCP_META_KEY: mcp_meta_value(),
    });
    rpc_request_payload(
        MCP_INITIALIZE_ID,
        MCP_INITIALIZE_METHOD,
        &params.to_string(),
    )
}

pub(crate) fn parse_http_response_json(text: &str) -> AxonResult<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Ok(value);
    }
    for line in text.lines() {
        let Some(payload) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let payload = payload.trim();
        if payload == "[DONE]" || payload.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(payload)
            && value.get("id").is_some()
        {
            return Ok(value);
        }
    }
    Err(AxonError::InvalidMcpDefinition(
        "MCP HTTP 响应不是 JSON 或 SSE JSON",
    ))
}

fn mcp_startup_timeout() -> Duration {
    Duration::from_secs(MCP_HTTP_STARTUP_TIMEOUT_SECS)
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue, WWW_AUTHENTICATE};

    use super::*;
    use crate::mcp_transport_auth::mcp_oauth_required;

    fn oauth_reason(error: AxonError) -> String {
        match error {
            AxonError::InvalidMcpDefinition(reason) => reason.to_owned(),
            other => panic!("应投影为 InvalidMcpDefinition，实际为 {other:?}"),
        }
    }

    fn challenge_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_str(value).expect("challenge header 应可构造"),
        );
        headers
    }

    #[test]
    fn unauthorized_status_maps_to_oauth_required_reason() {
        let reason = oauth_reason(mcp_http_status_error(
            reqwest::StatusCode::UNAUTHORIZED,
            &HeaderMap::new(),
        ));
        assert_eq!(reason, MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON);
        assert!(mcp_oauth_required(&reason));
    }

    #[test]
    fn forbidden_insufficient_scope_maps_to_scope_reason() {
        let headers = challenge_headers(r#"Bearer error="insufficient_scope", scope="read write""#);
        let reason = oauth_reason(mcp_http_status_error(
            reqwest::StatusCode::FORBIDDEN,
            &headers,
        ));
        assert_eq!(reason, MCP_OAUTH_REQUIRED_INSUFFICIENT_SCOPE_REASON);
        assert!(mcp_oauth_required(&reason));
    }

    #[test]
    fn forbidden_without_scope_challenge_maps_to_forbidden_reason() {
        let headers = challenge_headers(r#"Bearer error="invalid_token""#);
        let reason = oauth_reason(mcp_http_status_error(
            reqwest::StatusCode::FORBIDDEN,
            &headers,
        ));
        assert_eq!(reason, MCP_OAUTH_REQUIRED_FORBIDDEN_REASON);

        let bare = oauth_reason(mcp_http_status_error(
            reqwest::StatusCode::FORBIDDEN,
            &HeaderMap::new(),
        ));
        assert_eq!(bare, MCP_OAUTH_REQUIRED_FORBIDDEN_REASON);
    }

    #[test]
    fn unrelated_status_keeps_generic_status_error() {
        for status in [
            reqwest::StatusCode::BAD_REQUEST,
            reqwest::StatusCode::NOT_FOUND,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let reason = oauth_reason(mcp_http_status_error(status, &HeaderMap::new()));
            assert_eq!(reason, "MCP HTTP 状态码异常");
            assert!(!mcp_oauth_required(&reason));
        }
    }

    #[test]
    fn www_authenticate_error_matches_case_insensitively() {
        let headers = challenge_headers("Bearer realm=\"mcp\", Error=\"Insufficient_Scope\"");
        assert_eq!(
            www_authenticate_error(&headers).as_deref(),
            Some("insufficient_scope")
        );
        assert_eq!(www_authenticate_error(&HeaderMap::new()), None);
    }
}
