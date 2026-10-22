//! MCP Streamable HTTP 授权发现与安全 header 解析。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;
use url::Url;

use crate::mcp_transport_http_client::{MCP_METADATA_TIMEOUT_SECS, shared_http_client};
use crate::mcp_transport_oauth_discovery::protected_resource_metadata_endpoints;
use crate::{
    McpHttpHeader, McpHttpServerConfig, McpOAuthLoginStartProjection, McpOAuthLoginStartRequest,
    McpOAuthLogoutProjection, McpOAuthLogoutRequest, McpOAuthPendingLoginSession,
    McpOAuthStatusRequest, McpOAuthTokenImportProjection, McpOAuthTokenImportRequest,
    McpStoredOAuthToken, mcp_transport_oauth,
};

const AUTHORIZATION_HEADER: &str = "Authorization";
const BEARER_PREFIX: &str = "Bearer ";
const WWW_AUTHENTICATE_RESOURCE_METADATA: &str = "resource_metadata";

/// MCP 需要 OAuth 授权时失败文本的稳定标记前缀。
///
/// 401/403 是授权缺失或授权范围不足，必须与普通协议错误区分，否则上层只能报告
/// “状态码异常”，无法把用户引导到 `mcp login`。该前缀是 TUI 与 CLI 共同识别的契约，
/// 透传失败原因的一方只需调用 [`mcp_oauth_required`]，不需要解析服务器地址。
pub const MCP_OAUTH_REQUIRED_MESSAGE_PREFIX: &str = "MCP 需要 OAuth 授权";

/// server 以 401 拒绝当前凭据时的失败原因。
pub const MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON: &str =
    "MCP 需要 OAuth 授权：服务器返回 401，请完成登录后重试";

/// server 以 403 拒绝当前授权范围时的失败原因。
pub const MCP_OAUTH_REQUIRED_INSUFFICIENT_SCOPE_REASON: &str =
    "MCP 需要 OAuth 授权：服务器返回 403 insufficient_scope，请按新 scope 重新授权";

/// server 以其他 403 拒绝请求时的失败原因。
pub const MCP_OAUTH_REQUIRED_FORBIDDEN_REASON: &str =
    "MCP 需要 OAuth 授权：服务器返回 403，请确认当前授权范围";

/// 判断失败文本是否表示 MCP server 需要 OAuth 授权。
#[must_use]
pub fn mcp_oauth_required(message: &str) -> bool {
    message.contains(MCP_OAUTH_REQUIRED_MESSAGE_PREFIX)
}

/// 构造 MCP OAuth 登录引导文本。
#[must_use]
pub fn mcp_oauth_login_guidance(server_id: &str) -> String {
    format!("运行 /mcp login {server_id} 或 axon mcp login {server_id} 完成授权")
}

/// MCP Streamable HTTP 授权状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthStatus {
    /// 不需要认证，或 server 未声明认证。
    Unsupported,
    /// 使用 bearer token。
    BearerToken,
    /// server 暴露 OAuth metadata，可引导后续登录闭环。
    OAuthAvailable,
    /// server 要求认证，但当前没有可用 credential。
    AuthRequired,
}

/// MCP Streamable HTTP credential 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCredentialStatus {
    /// stdio 或未声明认证。
    NotRequired,
    /// bearer token 环境变量存在且非空。
    EnvReady,
    /// bearer token 环境变量未设置。
    EnvMissing,
    /// bearer token 环境变量为空。
    EnvEmpty,
    /// OAuth metadata 可用，可进入登录闭环。
    OAuthAvailable,
    /// OAuth token 已保存。
    OAuthReady,
    /// OAuth 登录已开始，等待导入 token response。
    OAuthPending,
    /// server 要求认证但没有可用登录入口。
    AuthRequired,
}

/// MCP Streamable HTTP 授权发现请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthDiscoveryRequest {
    /// MCP endpoint URL。
    pub url: String,
    /// server 返回的 `WWW-Authenticate` header。
    pub www_authenticate: Option<String>,
    /// 配置中的 bearer token env var。
    pub bearer_token_env_var: Option<String>,
}

/// MCP Streamable HTTP 授权发现投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAuthDiscoveryProjection {
    /// 授权状态。
    pub status: McpAuthStatus,
    /// protected resource metadata URL。
    pub resource_metadata_url: Option<String>,
    /// authorization server issuer。
    pub authorization_server: Option<String>,
    /// server 支持的 OAuth scopes。
    pub scopes_supported: Vec<String>,
    /// `WWW-Authenticate` challenge 声明的 scopes。
    pub challenge_scopes: Vec<String>,
    /// 已保存 OAuth token 的访问令牌是否存在。
    pub oauth_access_token_available: bool,
    /// 是否存在待完成 OAuth 登录会话。
    pub oauth_pending: bool,
    /// credential 脱敏状态。
    pub credential_status: McpCredentialStatus,
    /// 面向 CLI/status 的下一步动作。
    pub next_action: String,
    /// 是否暴露 token 原文。
    pub token_value_exposed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// MCP Streamable HTTP 请求授权配置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpHttpAuthConfig {
    /// Bearer token 环境变量名。
    pub bearer_token_env_var: Option<String>,
    /// 运行态 OAuth access token；只用于请求 header，不进入配置或输出投影。
    pub oauth_access_token: Option<String>,
}

impl McpHttpAuthConfig {
    /// 不启用请求授权。
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// 从 bearer token 环境变量名构造配置。
    #[must_use]
    pub fn from_bearer_token_env_var(value: Option<String>) -> Self {
        Self {
            bearer_token_env_var: value
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            oauth_access_token: None,
        }
    }

    /// 从 Streamable HTTP server 配置构造请求授权配置。
    #[must_use]
    pub fn from_http_server(config: &McpHttpServerConfig) -> Self {
        let mut auth = Self::from_bearer_token_env_var(config.bearer_token_env_var.clone());
        auth.oauth_access_token = config
            .oauth_access_token
            .clone()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        auth
    }
}

/// MCP Streamable HTTP 授权 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpAuthRuntime;

impl McpAuthRuntime {
    /// 根据配置和 `WWW-Authenticate` header 发现授权能力。
    ///
    /// # Errors
    /// URL 非法、metadata 响应不可读取或 JSON 非法时返回错误。
    pub fn discover(request: McpAuthDiscoveryRequest) -> AxonResult<McpAuthDiscoveryProjection> {
        if request.bearer_token_env_var.is_some() {
            return Ok(bearer_projection(request.bearer_token_env_var.as_deref()));
        }
        let challenge_scopes = request
            .www_authenticate
            .as_deref()
            .map(scopes_from_challenge)
            .unwrap_or_default();
        let Some((metadata_url, metadata)) = discover_protected_resource_metadata(&request) else {
            return Ok(unsupported_projection());
        };
        let authorization_server = metadata
            .get("authorization_servers")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(Value::as_str)
            .or_else(|| metadata.get("issuer").and_then(Value::as_str))
            .map(ToOwned::to_owned);
        let scopes_supported = metadata
            .get("scopes_supported")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let oauth_available = authorization_server.is_some();
        Ok(McpAuthDiscoveryProjection {
            status: if oauth_available {
                McpAuthStatus::OAuthAvailable
            } else {
                McpAuthStatus::AuthRequired
            },
            resource_metadata_url: Some(metadata_url),
            authorization_server,
            scopes_supported,
            challenge_scopes,
            oauth_access_token_available: false,
            oauth_pending: false,
            credential_status: if oauth_available {
                McpCredentialStatus::OAuthAvailable
            } else {
                McpCredentialStatus::AuthRequired
            },
            next_action: if oauth_available {
                "运行 MCP OAuth 登录流程或配置 bearerTokenEnvVar".to_owned()
            } else {
                "配置 bearerTokenEnvVar 或检查 MCP server 授权 metadata".to_owned()
            },
            token_value_exposed: false,
            reasons: vec!["mcp_auth:protected_resource_metadata_discovered".to_owned()],
        })
    }

    /// 查询本地 OAuth credential 状态。
    pub fn oauth_status(
        request: McpOAuthStatusRequest,
    ) -> AxonResult<(bool, bool, Option<McpStoredOAuthToken>)> {
        mcp_transport_oauth::oauth_status(request)
    }

    /// 读取请求可用的 OAuth token；过期且可刷新时会回写新 token。
    pub fn oauth_token_for_request(
        request: McpOAuthStatusRequest,
    ) -> AxonResult<Option<McpStoredOAuthToken>> {
        mcp_transport_oauth::oauth_token_for_request(request)
    }

    /// 生成 OAuth 授权 URL 并保存待完成登录会话。
    pub fn start_oauth_login(
        request: McpOAuthLoginStartRequest,
    ) -> AxonResult<McpOAuthLoginStartProjection> {
        mcp_transport_oauth::start_oauth_login(request)
    }

    /// 生成 OAuth 授权 URL 并返回待完成登录会话。
    ///
    /// 调用方拿到 [`McpOAuthPendingLoginSession::authorization_url`] 后可以先用系统浏览器
    /// 打开授权页，再调用 [`Self::complete_oauth_login`] 等待本地回调；只有请求声明
    /// `wait_for_callback` 时才会绑定本地回调端口。
    pub fn begin_oauth_login(
        request: McpOAuthLoginStartRequest,
    ) -> AxonResult<McpOAuthPendingLoginSession> {
        mcp_transport_oauth::begin_oauth_login(request)
    }

    /// 等待本地回调并交换 token，完成 OAuth 登录。
    ///
    /// 会话没有绑定回调端口时直接返回生成的授权 URL 投影，不发起 token 请求。
    pub fn complete_oauth_login(
        session: McpOAuthPendingLoginSession,
    ) -> AxonResult<McpOAuthLoginStartProjection> {
        mcp_transport_oauth::complete_oauth_login(session)
    }

    /// 导入 OAuth token response 并完成登录。
    pub fn import_oauth_token(
        request: McpOAuthTokenImportRequest,
    ) -> AxonResult<McpOAuthTokenImportProjection> {
        mcp_transport_oauth::import_oauth_token(request)
    }

    /// 删除本地 OAuth token 与待完成登录会话。
    pub fn logout_oauth(request: McpOAuthLogoutRequest) -> AxonResult<McpOAuthLogoutProjection> {
        mcp_transport_oauth::logout_oauth(request)
    }

    /// 探测 MCP endpoint 的 `WWW-Authenticate` challenge。
    ///
    /// MCP 2026-07-28 要求客户端优先采用 401/403 响应中的 challenge 作为
    /// `resource_metadata` 与 `scope` 的权威来源。探测使用只读 `tools/list` 请求，
    /// 未授权时服务器只会返回 401，不产生副作用。
    ///
    /// # Errors
    /// URL 非法或请求无法发出时返回错误；服务器未返回 challenge 时返回 `None`。
    pub fn probe_www_authenticate(url: &str) -> AxonResult<Option<String>> {
        let plan = crate::mcp_transport::tools_list_http_request(url, None)?;
        let mut request = shared_http_client()?
            .post(&plan.url)
            .timeout(std::time::Duration::from_secs(MCP_METADATA_TIMEOUT_SECS));
        for header in &plan.headers {
            request = request.header(header.name.as_str(), header.value.as_str());
        }
        let response = request
            .body(plan.body_json)
            .send()
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP auth challenge 探测请求失败"))?;
        Ok(response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned))
    }
}

pub(crate) fn authorization_header_from_env(
    config: &McpHttpAuthConfig,
) -> AxonResult<Option<McpHttpHeader>> {
    if let Some(token) = config.oauth_access_token.as_deref() {
        let token = token.trim();
        if token.is_empty() {
            return Err(AxonError::InvalidMcpDefinition("MCP OAuth token 为空"));
        }
        return Ok(Some(McpHttpHeader {
            name: AUTHORIZATION_HEADER.to_owned(),
            value: format!("{BEARER_PREFIX}{token}"),
        }));
    }
    let Some(env_var) = config.bearer_token_env_var.as_deref() else {
        return Ok(None);
    };
    let token = std::env::var(env_var)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP bearer token 环境变量未设置"))?;
    let token = token.trim();
    if token.is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP bearer token 环境变量为空",
        ));
    }
    Ok(Some(McpHttpHeader {
        name: AUTHORIZATION_HEADER.to_owned(),
        value: format!("{BEARER_PREFIX}{token}"),
    }))
}

fn unsupported_projection() -> McpAuthDiscoveryProjection {
    McpAuthDiscoveryProjection {
        status: McpAuthStatus::Unsupported,
        resource_metadata_url: None,
        authorization_server: None,
        scopes_supported: Vec::new(),
        challenge_scopes: Vec::new(),
        oauth_access_token_available: false,
        oauth_pending: false,
        credential_status: McpCredentialStatus::NotRequired,
        next_action: "无需授权配置".to_owned(),
        token_value_exposed: false,
        reasons: vec!["mcp_auth:oauth_metadata_not_found".to_owned()],
    }
}

fn bearer_projection(env_var: Option<&str>) -> McpAuthDiscoveryProjection {
    let env_var = env_var.unwrap_or_default();
    let (status, credential_status, next_action, reason) = match std::env::var(env_var) {
        Ok(value) if value.trim().is_empty() => (
            McpAuthStatus::AuthRequired,
            McpCredentialStatus::EnvEmpty,
            format!("为环境变量 {env_var} 设置非空 bearer token"),
            "mcp_auth:bearer_token_env_empty",
        ),
        Ok(_) => (
            McpAuthStatus::BearerToken,
            McpCredentialStatus::EnvReady,
            "无需操作".to_owned(),
            "mcp_auth:bearer_token_env_ready",
        ),
        Err(_) => (
            McpAuthStatus::AuthRequired,
            McpCredentialStatus::EnvMissing,
            format!("设置环境变量 {env_var} 后重试"),
            "mcp_auth:bearer_token_env_missing",
        ),
    };
    McpAuthDiscoveryProjection {
        status,
        resource_metadata_url: None,
        authorization_server: None,
        scopes_supported: Vec::new(),
        challenge_scopes: Vec::new(),
        oauth_access_token_available: false,
        oauth_pending: false,
        credential_status,
        next_action,
        token_value_exposed: false,
        reasons: vec![reason.to_owned()],
    }
}

fn resource_metadata_from_challenge(value: &str) -> Option<String> {
    value.split(',').find_map(|part| {
        let part = part.trim();
        let part = part
            .strip_prefix("Bearer ")
            .or_else(|| part.strip_prefix("bearer "))
            .unwrap_or(part);
        let (name, value) = part.trim().split_once('=')?;
        (name.trim() == WWW_AUTHENTICATE_RESOURCE_METADATA)
            .then(|| value.trim().trim_matches('"'))
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn scopes_from_challenge(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            let part = part
                .strip_prefix("Bearer ")
                .or_else(|| part.strip_prefix("bearer "))
                .unwrap_or(part);
            let (name, value) = part.trim().split_once('=')?;
            (name.trim() == "scope")
                .then(|| value.trim().trim_matches('"'))
                .filter(|value| !value.is_empty())
        })
        .flat_map(|value| value.split_whitespace().map(ToOwned::to_owned))
        .collect()
}

/// 按 RFC 9728 与 MCP 2026-07-28 顺序探测 protected resource metadata。
///
/// `WWW-Authenticate` challenge 显式给出的 `resource_metadata` 是权威来源；缺失时先
/// 探测 MCP 端点子路径形式，再回退站点根路径形式。
fn discover_protected_resource_metadata(
    request: &McpAuthDiscoveryRequest,
) -> Option<(String, Value)> {
    for url in resource_metadata_candidates(request) {
        if let Ok(metadata) = fetch_metadata(&url) {
            return Some((url, metadata));
        }
    }
    None
}

fn resource_metadata_candidates(request: &McpAuthDiscoveryRequest) -> Vec<String> {
    if let Some(url) = request
        .www_authenticate
        .as_deref()
        .and_then(resource_metadata_from_challenge)
    {
        return vec![url];
    }
    let Ok(endpoint) = Url::parse(&request.url) else {
        return Vec::new();
    };
    protected_resource_metadata_endpoints(&endpoint)
}

fn fetch_metadata(url: &str) -> AxonResult<Value> {
    let response = shared_http_client()?
        .get(url)
        .timeout(std::time::Duration::from_secs(MCP_METADATA_TIMEOUT_SECS))
        .send()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP auth metadata 请求失败"))?;
    if !response.status().is_success() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP auth metadata 状态码异常",
        ));
    }
    let text = response
        .text()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP auth metadata 响应读取失败"))?;
    let value = serde_json::from_str::<Value>(&text)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP auth metadata JSON 无效"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use super::*;

    #[test]
    fn discover_reads_resource_metadata_from_www_authenticate_challenge() {
        let server = spawn_metadata_server();
        let request = McpAuthDiscoveryRequest {
            url: "http://127.0.0.1:9/mcp".to_owned(),
            www_authenticate: Some(format!(
                r#"Bearer resource_metadata="{}""#,
                server.metadata_url
            )),
            bearer_token_env_var: None,
        };

        let projection = McpAuthRuntime::discover(request).expect("auth metadata 应可发现");

        assert_eq!(projection.status, McpAuthStatus::OAuthAvailable);
        assert_eq!(
            projection.credential_status,
            McpCredentialStatus::OAuthAvailable
        );
        assert_eq!(
            projection.resource_metadata_url.as_deref(),
            Some(server.metadata_url.as_str())
        );
        assert_eq!(
            projection.authorization_server.as_deref(),
            Some("https://auth.example.test")
        );
        assert_eq!(projection.scopes_supported, ["read", "write"]);
        assert_eq!(projection.challenge_scopes, Vec::<String>::new());
        assert!(!projection.token_value_exposed);
        server.join();
    }

    #[test]
    fn bearer_token_auth_status_reports_missing_env_without_exposing_token_value() {
        let env_var = format!("AXON_MCP_TOKEN_MISSING_{}", std::process::id());
        let projection = McpAuthRuntime::discover(McpAuthDiscoveryRequest {
            url: "http://127.0.0.1:9/mcp".to_owned(),
            www_authenticate: None,
            bearer_token_env_var: Some(env_var.clone()),
        })
        .expect("bearer token env 配置应直接得到状态");

        assert_eq!(projection.status, McpAuthStatus::AuthRequired);
        assert_eq!(
            projection.credential_status,
            McpCredentialStatus::EnvMissing
        );
        assert_eq!(projection.resource_metadata_url, None);
        assert!(projection.next_action.contains(&env_var));
        assert!(!projection.token_value_exposed);
    }

    #[test]
    fn auth_challenge_scopes_are_projected_without_raw_challenge() {
        let server = spawn_metadata_server();
        let projection = McpAuthRuntime::discover(McpAuthDiscoveryRequest {
            url: "http://127.0.0.1:9/mcp".to_owned(),
            www_authenticate: Some(format!(
                r#"Bearer scope="read write", resource_metadata="{}""#,
                server.metadata_url
            )),
            bearer_token_env_var: None,
        })
        .expect("auth metadata 应可发现");

        assert_eq!(projection.challenge_scopes, ["read", "write"]);
        assert!(!projection.token_value_exposed);
        server.join();
    }

    #[test]
    fn oauth_required_matches_all_challenge_reasons() {
        for reason in [
            MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON,
            MCP_OAUTH_REQUIRED_INSUFFICIENT_SCOPE_REASON,
            MCP_OAUTH_REQUIRED_FORBIDDEN_REASON,
        ] {
            assert!(mcp_oauth_required(reason));
        }
        assert!(!mcp_oauth_required("MCP HTTP 状态码异常"));
        assert!(!mcp_oauth_required(""));
    }

    #[test]
    fn oauth_login_guidance_carries_server_id_in_both_entry_points() {
        let guidance = mcp_oauth_login_guidance("sm_cloud_platform");
        assert!(guidance.contains("/mcp login sm_cloud_platform"));
        assert!(guidance.contains("axon mcp login sm_cloud_platform"));
    }

    struct MetadataServer {
        metadata_url: String,
        receiver: mpsc::Receiver<()>,
    }

    impl MetadataServer {
        fn join(self) {
            self.receiver.recv().expect("metadata 请求应完成");
        }
    }

    fn spawn_metadata_server() -> MetadataServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("metadata server 应可绑定端口");
        let metadata_url = format!(
            "http://{}/.well-known/oauth-protected-resource",
            listener.local_addr().expect("本地地址应可读取")
        );
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("metadata 请求应可接收");
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let body = r#"{"authorization_servers":["https://auth.example.test"],"scopes_supported":["read","write"]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("metadata 响应应可写入");
            let _ = sender.send(());
        });
        MetadataServer {
            metadata_url,
            receiver,
        }
    }
}
