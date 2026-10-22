//! MCP OAuth 授权流程 HTTP 层：metadata 入口、授权 URL、本地 callback 与 token 端点交互。
//!
//! 该模块承载 MCP 2026-07-28 授权章节的强制校验：
//! 授权服务器 metadata 必须来自真实的发现端点（禁止合成 `/authorize`、`/token` 回退），
//! 授权响应在授权服务器声明支持时校验 RFC 9207 `iss` 参数。

use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

use axon_domain::{AxonError, AxonResult};
use serde::Deserialize;
use url::Url;

use crate::mcp_transport_http_client::shared_http_client;
use crate::mcp_transport_oauth_discovery::{
    DiscoveredAuthorizationServerMetadata, discover_authorization_server_metadata,
    validate_authorization_response_issuer,
};

/// 未启动本地 callback 监听时使用的静态 redirect URI。
pub(crate) const OAUTH_REDIRECT_URI: &str = "http://127.0.0.1/oauth/callback";
const OAUTH_CALLBACK_HOST: &str = "127.0.0.1";
/// 首选 callback 端口。
///
/// 端口稳定时 redirect URI 稳定，动态注册的 client 才能被复用；仅在端口被占用时
/// 回退到系统分配的临时端口。
const OAUTH_CALLBACK_PREFERRED_PORT: u16 = 47821;
const OAUTH_CALLBACK_EPHEMERAL_PORT: u16 = 0;
const OAUTH_CALLBACK_PATH: &str = "/oauth/callback";
const OAUTH_CODE_CHALLENGE_METHOD: &str = "S256";
const OAUTH_RESPONSE_TYPE_CODE: &str = "code";
const OAUTH_GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";
const OAUTH_GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";
const OAUTH_CALLBACK_BUFFER_BYTES: usize = 4096;
const OAUTH_CALLBACK_WAIT_TIMEOUT: Duration = Duration::from_secs(300);
const OAUTH_CALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const OAUTH_TOKEN_TIMEOUT_SECS: u64 = 30;
const OAUTH_CALLBACK_OK_BODY: &str = "Axon MCP login completed";
const OAUTH_CALLBACK_ERROR_BODY: &str = "Axon MCP login failed";
const OAUTH_CALLBACK_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// OAuth token endpoint 响应。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct OAuthTokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

/// OAuth 2.1 错误响应。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct OAuthErrorResponse {
    pub(crate) error: String,
    #[serde(default)]
    pub(crate) error_description: Option<String>,
}

/// authorization code 交换入参。
pub(crate) struct AuthorizationCodeExchange<'a> {
    pub token_endpoint: &'a str,
    pub client_id: &'a str,
    pub client_secret: Option<&'a str>,
    pub redirect_uri: &'a str,
    pub resource: &'a str,
    pub code_verifier: &'a str,
    pub code: &'a str,
}

/// refresh token 交换入参。
pub(crate) struct RefreshTokenExchange<'a> {
    pub token_endpoint: &'a str,
    pub client_id: Option<&'a str>,
    pub client_secret: Option<&'a str>,
    pub refresh_token: &'a str,
    pub resource: Option<&'a str>,
}

/// OAuth callback 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OAuthCallbackOutcome {
    /// 授权码。
    pub code: String,
    /// RFC 9207 授权响应 issuer；未返回时为 `None`。
    pub issuer: Option<String>,
}

/// 发现授权服务器 metadata，供后续授权流程复用。
///
/// MCP 2026-07-28 要求客户端在 OAuth Authorization Server Metadata 与 OpenID Connect
/// Discovery 之间双发现，且接受任何通过 issuer 一致性校验的文档。全部候选不可用时
/// 直接失败，不再合成授权端点。
pub(crate) fn provider_metadata(
    authorization_server: &str,
) -> AxonResult<DiscoveredAuthorizationServerMetadata> {
    discover_authorization_server_metadata(authorization_server)?.ok_or(
        AxonError::InvalidMcpDefinition("MCP OAuth 授权服务器 metadata 发现失败"),
    )
}

/// 构造授权请求 URL。
pub(crate) fn authorization_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
    scopes: &[String],
    resource: &str,
) -> AxonResult<String> {
    let mut url = Url::parse(authorization_endpoint)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth authorization server URL 无效"))?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", OAUTH_RESPONSE_TYPE_CODE)
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", code_challenge)
            .append_pair("code_challenge_method", OAUTH_CODE_CHALLENGE_METHOD)
            .append_pair("resource", resource);
        if !scopes.is_empty() {
            query.append_pair("scope", &scopes.join(" "));
        }
    }
    Ok(url.to_string())
}

/// 绑定本地 callback 监听端口。
pub(crate) fn callback_listener() -> AxonResult<TcpListener> {
    TcpListener::bind((OAUTH_CALLBACK_HOST, OAUTH_CALLBACK_PREFERRED_PORT))
        .or_else(|_| TcpListener::bind((OAUTH_CALLBACK_HOST, OAUTH_CALLBACK_EPHEMERAL_PORT)))
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth callback 端口绑定失败"))
}

/// 读取本地 callback 的实际 redirect URI。
pub(crate) fn callback_redirect_uri(listener: &TcpListener) -> AxonResult<String> {
    let address = listener
        .local_addr()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth callback 地址读取失败"))?;
    Ok(format!("http://{address}{OAUTH_CALLBACK_PATH}"))
}

/// 等待本地 callback 并校验 state 与 RFC 9207 `iss`。
pub(crate) fn wait_for_callback_code(
    listener: TcpListener,
    expected_state: &str,
    expected_issuer: &str,
    require_issuer: bool,
) -> AxonResult<OAuthCallbackOutcome> {
    let mut stream = accept_callback_stream(listener)?;
    let mut buffer = [0_u8; OAUTH_CALLBACK_BUFFER_BYTES];
    let bytes = stream
        .read(&mut buffer)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth callback 读取失败"))?;
    let request = String::from_utf8_lossy(&buffer[..bytes]);
    let first_line = request.lines().next().unwrap_or_default();
    let path = first_line.split_whitespace().nth(1).unwrap_or_default();
    let callback_url = Url::parse(&format!("http://localhost{path}"))
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth callback URL 无效"))?;
    let parameter = |name: &str| {
        callback_url
            .query_pairs()
            .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
            .filter(|value| !value.trim().is_empty())
    };
    if let Some(error) = parameter("error") {
        let description = parameter("error_description").unwrap_or_default();
        write_callback_response(&mut stream, "400 Bad Request", OAUTH_CALLBACK_ERROR_BODY);
        return Err(AxonError::invalid_input(if description.is_empty() {
            format!("MCP OAuth callback 返回错误：{error}")
        } else {
            format!("MCP OAuth callback 返回错误：{error}（{description}）")
        }));
    }
    let code = parameter("code").ok_or(AxonError::InvalidMcpDefinition(
        "MCP OAuth callback 缺少 code",
    ))?;
    let state = parameter("state").ok_or(AxonError::InvalidMcpDefinition(
        "MCP OAuth callback 缺少 state",
    ))?;
    if state != expected_state {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth callback state 不匹配",
        ));
    }
    let issuer = parameter("iss");
    validate_authorization_response_issuer(
        Some(expected_issuer),
        require_issuer,
        issuer.as_deref(),
    )?;
    write_callback_response(&mut stream, "200 OK", OAUTH_CALLBACK_OK_BODY);
    Ok(OAuthCallbackOutcome { code, issuer })
}

fn write_callback_response(stream: &mut impl Write, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {OAUTH_CALLBACK_CONTENT_TYPE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

/// 在有界预算内等待浏览器回调连接。
///
/// 交互式 TUI 把登录放在后台线程执行，无限期阻塞会留下无法回收的等待线程，
/// 因此这里使用非阻塞 accept 轮询并在超时后返回可读错误。
fn accept_callback_stream(listener: TcpListener) -> AxonResult<TcpStream> {
    listener
        .set_nonblocking(true)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth callback 监听配置失败"))?;
    let deadline = Instant::now() + OAUTH_CALLBACK_WAIT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).map_err(|_| {
                    AxonError::InvalidMcpDefinition("MCP OAuth callback 连接配置失败")
                })?;
                return Ok(stream);
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(AxonError::InvalidMcpDefinition(
                        "MCP OAuth 授权等待超时，请重新发起登录",
                    ));
                }
                thread::sleep(OAUTH_CALLBACK_POLL_INTERVAL);
            }
            Err(_) => {
                return Err(AxonError::InvalidMcpDefinition(
                    "MCP OAuth callback 等待失败",
                ));
            }
        }
    }
}

/// 使用 authorization code 交换 token。
pub(crate) fn exchange_authorization_code(
    exchange: AuthorizationCodeExchange<'_>,
) -> AxonResult<OAuthTokenResponse> {
    let mut params = vec![
        ("grant_type", OAUTH_GRANT_TYPE_AUTHORIZATION_CODE.to_owned()),
        ("code", exchange.code.to_owned()),
        ("redirect_uri", exchange.redirect_uri.to_owned()),
        ("client_id", exchange.client_id.to_owned()),
        ("code_verifier", exchange.code_verifier.to_owned()),
        ("resource", exchange.resource.to_owned()),
    ];
    if let Some(client_secret) = exchange.client_secret {
        params.push(("client_secret", client_secret.to_owned()));
    }
    post_token_endpoint(exchange.token_endpoint, &params)
}

/// 使用 refresh token 刷新 access token。
pub(crate) fn exchange_refresh_token(
    exchange: RefreshTokenExchange<'_>,
) -> AxonResult<OAuthTokenResponse> {
    let mut params = vec![
        ("grant_type", OAUTH_GRANT_TYPE_REFRESH_TOKEN.to_owned()),
        ("refresh_token", exchange.refresh_token.to_owned()),
    ];
    if let Some(client_id) = exchange.client_id {
        params.push(("client_id", client_id.to_owned()));
    }
    if let Some(client_secret) = exchange.client_secret {
        params.push(("client_secret", client_secret.to_owned()));
    }
    if let Some(resource) = exchange.resource {
        params.push(("resource", resource.to_owned()));
    }
    post_token_endpoint(exchange.token_endpoint, &params)
}

fn post_token_endpoint(
    token_endpoint: &str,
    params: &[(&str, String)],
) -> AxonResult<OAuthTokenResponse> {
    let response = shared_http_client()?
        .post(token_endpoint)
        .timeout(std::time::Duration::from_secs(OAUTH_TOKEN_TIMEOUT_SECS))
        .form(params)
        .send()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth token 请求失败"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth token 响应读取失败"))?;
    serde_json::from_str::<OAuthTokenResponse>(&text)
        .map_err(|_| token_endpoint_error(status, &text))
}

/// 把 token 端点错误响应投影为可读错误，保留 OAuth 2.1 `error` 与描述。
fn token_endpoint_error(status: reqwest::StatusCode, body: &str) -> AxonError {
    let Ok(error) = serde_json::from_str::<OAuthErrorResponse>(body) else {
        return AxonError::InvalidMcpDefinition("MCP OAuth token 响应无效");
    };
    match error
        .error_description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(description) => AxonError::invalid_input(format!(
            "MCP OAuth token 端点返回错误：{}（{description}），HTTP {status}",
            error.error
        )),
        None => AxonError::invalid_input(format!(
            "MCP OAuth token 端点返回错误：{}，HTTP {status}",
            error.error
        )),
    }
}

/// PKCE code challenge（S256）。
pub(crate) fn oauth_code_challenge(verifier: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    URL_SAFE_NO_PAD.encode(axon_crypto::Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preferred_redirect_uri() -> String {
        format!("http://{OAUTH_CALLBACK_HOST}:{OAUTH_CALLBACK_PREFERRED_PORT}{OAUTH_CALLBACK_PATH}")
    }

    #[test]
    fn callback_redirect_uri_prefers_stable_port_when_available() {
        let Ok(probe) = TcpListener::bind((OAUTH_CALLBACK_HOST, OAUTH_CALLBACK_PREFERRED_PORT))
        else {
            // 端口被其他进程占用时无法断言首选端口，跳过该用例。
            return;
        };
        drop(probe);

        let listener = callback_listener().expect("应可绑定 callback 端口");
        assert_eq!(
            callback_redirect_uri(&listener).expect("应可读取 redirect URI"),
            preferred_redirect_uri()
        );
    }

    #[test]
    fn callback_listener_falls_back_to_ephemeral_port_when_preferred_is_taken() {
        let Ok(squatter) = TcpListener::bind((OAUTH_CALLBACK_HOST, OAUTH_CALLBACK_PREFERRED_PORT))
        else {
            // 首选端口本就不可用，无法构造占用场景，跳过该用例。
            return;
        };

        let listener = callback_listener().expect("应回退到系统分配的临时端口");
        let redirect = callback_redirect_uri(&listener).expect("应可读取 redirect URI");
        assert_ne!(redirect, preferred_redirect_uri());
        assert!(redirect.starts_with(&format!("http://{OAUTH_CALLBACK_HOST}:")));
        assert!(redirect.ends_with(OAUTH_CALLBACK_PATH));
        drop(squatter);
    }
}
