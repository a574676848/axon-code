//! MCP Streamable HTTP OAuth 凭据闭环。

use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
};

use axon_crypto::Sha256Hex;
use axon_domain::{AxonError, AxonResult};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::McpCredentialStatus;
use crate::mcp_transport_oauth_discovery::canonical_resource_uri;
use crate::mcp_transport_oauth_flow::{
    AuthorizationCodeExchange, OAUTH_REDIRECT_URI, OAuthTokenResponse, RefreshTokenExchange,
    authorization_url, callback_listener, callback_redirect_uri, exchange_authorization_code,
    exchange_refresh_token, oauth_code_challenge, provider_metadata, wait_for_callback_code,
};
use crate::mcp_transport_oauth_registration::resolve_client_registration;
pub use crate::mcp_transport_oauth_store::McpStoredOAuthToken;
use crate::mcp_transport_oauth_store::{
    now_ms, oauth_ref, pending_path, read_json, read_oauth_token, remove_file_if_exists,
    token_path, write_json,
};

const OAUTH_REFRESH_SKEW_MS: u64 = 60_000;

/// MCP OAuth 状态查询请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthStatusRequest {
    /// Axon runtime 根目录。
    pub root: PathBuf,
    /// MCP server 名称。
    pub server_name: String,
    /// MCP Streamable HTTP endpoint URL。
    pub url: String,
}

/// MCP OAuth 登录开始请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthLoginStartRequest {
    /// Axon runtime 根目录。
    pub root: PathBuf,
    /// MCP server 名称。
    pub server_name: String,
    /// MCP Streamable HTTP endpoint URL。
    pub url: String,
    /// OAuth authorization server 或 authorization endpoint。
    pub authorization_server: String,
    /// 请求 scopes。
    pub scopes: Vec<String>,
    /// 可选 OAuth client_id；为空时使用 Axon 默认 public client。
    pub client_id: Option<String>,
    /// 可选 OAuth resource indicator；为空时使用 MCP endpoint URL。
    pub resource: Option<String>,
    /// 是否启动本地 callback 并交换 token。
    pub wait_for_callback: bool,
}

/// MCP OAuth 登录开始投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthLoginStartProjection {
    /// 需要用户打开的授权 URL。
    pub authorization_url: String,
    /// 待完成登录会话引用。
    pub pending_ref: String,
    /// PKCE redirect URI。
    pub redirect_uri: String,
    /// 实际请求 scopes。
    pub scopes: Vec<String>,
    /// 客户端注册来源（pre_registered、client_id_metadata_document 或 dynamic_client_registration）。
    pub client_registration_source: String,
    /// 是否已保存 code verifier。
    pub code_verifier_stored: bool,
    /// 是否暴露 token 原文。
    pub token_value_exposed: bool,
    /// 是否已完成 callback token exchange。
    pub callback_completed: bool,
    /// token endpoint 是否已用于交换授权码。
    pub token_endpoint_used: bool,
    /// 是否已保存 access token。
    pub access_token_stored: bool,
    /// 是否已保存 refresh token。
    pub refresh_token_stored: bool,
    /// access token 过期时间。
    pub expires_at_ms: Option<u64>,
}

/// MCP OAuth token 导入请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthTokenImportRequest {
    /// Axon runtime 根目录。
    pub root: PathBuf,
    /// MCP server 名称。
    pub server_name: String,
    /// MCP Streamable HTTP endpoint URL。
    pub url: String,
    /// 待完成登录会话引用。
    pub pending_ref: String,
    /// OAuth token endpoint 返回的 JSON。
    pub token_response_json: String,
}

/// 待完成的 OAuth 登录会话。
///
/// 会话在生成授权 URL 与客户端注册后返回，调用方可以先用系统浏览器打开授权页，
/// 再消费会话等待本地回调；只有请求声明 `wait_for_callback` 时才持有回调监听端口。
#[derive(Debug)]
pub struct McpOAuthPendingLoginSession {
    root: PathBuf,
    pending: PendingOAuthLogin,
    projection: McpOAuthLoginStartProjection,
    listener: Option<TcpListener>,
    token_endpoint: Option<String>,
}

impl McpOAuthPendingLoginSession {
    /// 需要用户打开的授权 URL。
    #[must_use]
    pub fn authorization_url(&self) -> &str {
        &self.projection.authorization_url
    }

    /// 是否已绑定本地回调端口并等待浏览器授权。
    #[must_use]
    pub const fn waits_for_callback(&self) -> bool {
        self.listener.is_some()
    }
}

/// MCP OAuth token 导入投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthTokenImportProjection {
    /// 导入后的 credential 状态。
    pub credential_status: McpCredentialStatus,
    /// 是否已保存 access token。
    pub access_token_stored: bool,
    /// 是否已保存 refresh token。
    pub refresh_token_stored: bool,
    /// access token 过期时间。
    pub expires_at_ms: Option<u64>,
    /// 是否暴露 token 原文。
    pub token_value_exposed: bool,
}

/// MCP OAuth logout 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthLogoutRequest {
    /// Axon runtime 根目录。
    pub root: PathBuf,
    /// MCP server 名称。
    pub server_name: String,
    /// MCP Streamable HTTP endpoint URL。
    pub url: String,
}

/// MCP OAuth logout 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthLogoutProjection {
    /// 是否删除了已保存 token。
    pub removed_token: bool,
    /// 是否删除了待完成登录会话。
    pub removed_pending: bool,
    /// 是否暴露 token 原文。
    pub token_value_exposed: bool,
}

/// 查询本地 OAuth credential 状态。
pub(crate) fn oauth_status(
    request: McpOAuthStatusRequest,
) -> AxonResult<(bool, bool, Option<McpStoredOAuthToken>)> {
    let token = read_oauth_token(&request.root, &request.server_name, &request.url)?;
    let pending = pending_path(&request.root, &request.server_name, &request.url).exists();
    Ok((token.is_some(), pending, token))
}

/// 读取请求可用 token，过期时优先使用 refresh token 更新本地凭据。
pub(crate) fn oauth_token_for_request(
    request: McpOAuthStatusRequest,
) -> AxonResult<Option<McpStoredOAuthToken>> {
    let Some(token) = read_oauth_token(&request.root, &request.server_name, &request.url)? else {
        return Ok(None);
    };
    if !token_needs_refresh(&token) {
        return Ok(Some(token));
    }
    refresh_oauth_token(&request.root, token).map(Some)
}

/// 生成 OAuth 授权 URL 并保存待完成登录会话。
pub(crate) fn start_oauth_login(
    request: McpOAuthLoginStartRequest,
) -> AxonResult<McpOAuthLoginStartProjection> {
    let wait_for_callback = request.wait_for_callback;
    let session = begin_oauth_login(request)?;
    if wait_for_callback {
        complete_oauth_login(session)
    } else {
        Ok(session.projection)
    }
}

/// 生成 OAuth 授权 URL 并返回待完成登录会话。
pub(crate) fn begin_oauth_login(
    request: McpOAuthLoginStartRequest,
) -> AxonResult<McpOAuthPendingLoginSession> {
    let pending_ref = oauth_ref(&request.server_name, &request.url);
    let state = oauth_state(&request.server_name, &request.url);
    let code_verifier = oauth_code_verifier(&request.server_name, &request.url);
    let code_challenge = oauth_code_challenge(&code_verifier);
    let metadata = provider_metadata(&request.authorization_server)?;
    let listener = if request.wait_for_callback {
        Some(callback_listener()?)
    } else {
        None
    };
    let redirect_uri = match listener.as_ref() {
        Some(listener) => callback_redirect_uri(listener)?,
        None => OAUTH_REDIRECT_URI.to_owned(),
    };
    let registration = resolve_client_registration(
        &request.root,
        &request.authorization_server,
        request.client_id.as_deref(),
        &metadata,
        &redirect_uri,
    )?;
    let registration_source = registration.source.as_str().to_owned();
    let scopes = if request.scopes.is_empty() {
        metadata.scopes_supported.clone()
    } else {
        request.scopes.clone()
    };
    let resource = canonical_resource_uri(request.resource.as_deref().unwrap_or(&request.url))?;
    let authorization_url = authorization_url(
        &metadata.authorization_endpoint,
        &registration.client_id,
        &redirect_uri,
        &state,
        &code_challenge,
        &scopes,
        &resource,
    )?;
    let pending = PendingOAuthLogin {
        server_name: request.server_name.clone(),
        url: request.url.clone(),
        authorization_server: request.authorization_server.clone(),
        issuer: metadata.issuer.clone(),
        token_endpoint: metadata.token_endpoint.clone(),
        client_id: registration.client_id,
        client_secret: registration.client_secret,
        registration_source: registration_source.clone(),
        redirect_uri,
        resource,
        scopes: scopes.clone(),
        state,
        code_verifier,
        require_authorization_response_iss: metadata.authorization_response_iss_parameter_supported,
        created_at_ms: now_ms(),
    };
    write_json(
        &pending_path(&request.root, &request.server_name, &request.url),
        &pending,
    )?;
    let projection = McpOAuthLoginStartProjection {
        authorization_url,
        pending_ref,
        redirect_uri: pending.redirect_uri.clone(),
        scopes,
        client_registration_source: registration_source,
        code_verifier_stored: true,
        token_value_exposed: false,
        callback_completed: false,
        token_endpoint_used: false,
        access_token_stored: false,
        refresh_token_stored: false,
        expires_at_ms: None,
    };
    Ok(McpOAuthPendingLoginSession {
        root: request.root,
        token_endpoint: metadata.token_endpoint,
        pending,
        projection,
        listener,
    })
}

/// 等待本地回调、交换授权码并保存 token。
///
/// 会话未绑定回调端口时直接返回授权 URL 投影，不发起 token 请求。
pub(crate) fn complete_oauth_login(
    session: McpOAuthPendingLoginSession,
) -> AxonResult<McpOAuthLoginStartProjection> {
    let McpOAuthPendingLoginSession {
        root,
        pending,
        mut projection,
        listener,
        token_endpoint,
    } = session;
    let Some(listener) = listener else {
        return Ok(projection);
    };
    let callback = wait_for_callback_code(
        listener,
        &pending.state,
        &pending.issuer,
        pending.require_authorization_response_iss,
    )?;
    let Some(token_endpoint) = token_endpoint else {
        return Ok(projection);
    };
    let token_response = exchange_authorization_code(AuthorizationCodeExchange {
        token_endpoint: &token_endpoint,
        client_id: &pending.client_id,
        client_secret: pending.client_secret.as_deref(),
        redirect_uri: &pending.redirect_uri,
        resource: &pending.resource,
        code_verifier: &pending.code_verifier,
        code: &callback.code,
    })?;
    let import = import_token_response(&root, pending, token_response)?;
    projection.callback_completed = true;
    projection.token_endpoint_used = true;
    projection.access_token_stored = import.access_token_stored;
    projection.refresh_token_stored = import.refresh_token_stored;
    projection.expires_at_ms = import.expires_at_ms;
    Ok(projection)
}

/// 导入 OAuth token response 并完成登录。
pub(crate) fn import_oauth_token(
    request: McpOAuthTokenImportRequest,
) -> AxonResult<McpOAuthTokenImportProjection> {
    let expected_ref = oauth_ref(&request.server_name, &request.url);
    if request.pending_ref != expected_ref {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth pending ref 不匹配",
        ));
    }
    let response: OAuthTokenResponse = serde_json::from_str(&request.token_response_json)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth token response JSON 无效"))?;
    let pending_file = pending_path(&request.root, &request.server_name, &request.url);
    let pending: PendingOAuthLogin = read_json(&pending_file)?;
    import_token_response(&request.root, pending, response)
}

fn import_token_response(
    root: &Path,
    pending: PendingOAuthLogin,
    response: OAuthTokenResponse,
) -> AxonResult<McpOAuthTokenImportProjection> {
    let access_token = response.access_token.trim();
    if access_token.is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth access_token 为空",
        ));
    }
    let expires_at_ms = response
        .expires_in
        .and_then(|seconds| now_ms().checked_add(seconds.saturating_mul(1000)));
    let token = McpStoredOAuthToken {
        server_name: pending.server_name,
        url: pending.url,
        issuer: Some(pending.issuer),
        access_token: access_token.to_owned(),
        refresh_token: response
            .refresh_token
            .filter(|value| !value.trim().is_empty()),
        token_type: response.token_type.filter(|value| !value.trim().is_empty()),
        scope: response.scope.filter(|value| !value.trim().is_empty()),
        expires_at_ms,
        token_endpoint: pending.token_endpoint,
        client_id: Some(pending.client_id),
        client_secret: pending.client_secret,
        resource: Some(pending.resource),
        registration_source: Some(pending.registration_source),
    };
    write_json(&token_path(root, &token.server_name, &token.url), &token)?;
    let _ = fs::remove_file(pending_path(root, &token.server_name, &token.url));
    Ok(McpOAuthTokenImportProjection {
        credential_status: McpCredentialStatus::OAuthReady,
        access_token_stored: true,
        refresh_token_stored: token.refresh_token.is_some(),
        expires_at_ms,
        token_value_exposed: false,
    })
}

fn token_needs_refresh(token: &McpStoredOAuthToken) -> bool {
    token
        .expires_at_ms
        .is_some_and(|expires_at| expires_at <= now_ms().saturating_add(OAUTH_REFRESH_SKEW_MS))
}

fn refresh_oauth_token(root: &Path, token: McpStoredOAuthToken) -> AxonResult<McpStoredOAuthToken> {
    let refresh_token = token
        .refresh_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(AxonError::InvalidMcpDefinition("MCP OAuth token 已过期"))?;
    let token_endpoint = token
        .token_endpoint
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or(AxonError::InvalidMcpDefinition(
            "MCP OAuth token 已过期且缺少 token endpoint",
        ))?;
    let response = exchange_refresh_token(RefreshTokenExchange {
        token_endpoint,
        client_id: token
            .client_id
            .as_deref()
            .filter(|value| !value.trim().is_empty()),
        client_secret: token
            .client_secret
            .as_deref()
            .filter(|value| !value.trim().is_empty()),
        refresh_token,
        resource: token
            .resource
            .as_deref()
            .filter(|value| !value.trim().is_empty()),
    })?;
    let refreshed = refreshed_token(token, response)?;
    write_json(
        &token_path(root, &refreshed.server_name, &refreshed.url),
        &refreshed,
    )?;
    Ok(refreshed)
}

fn refreshed_token(
    mut token: McpStoredOAuthToken,
    response: OAuthTokenResponse,
) -> AxonResult<McpStoredOAuthToken> {
    let access_token = response.access_token.trim();
    if access_token.is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth refresh access_token 为空",
        ));
    }
    access_token.clone_into(&mut token.access_token);
    if let Some(refresh_token) = response
        .refresh_token
        .filter(|value| !value.trim().is_empty())
    {
        token.refresh_token = Some(refresh_token);
    }
    if let Some(token_type) = response.token_type.filter(|value| !value.trim().is_empty()) {
        token.token_type = Some(token_type);
    }
    if let Some(scope) = response.scope.filter(|value| !value.trim().is_empty()) {
        token.scope = Some(scope);
    }
    token.expires_at_ms = response
        .expires_in
        .and_then(|seconds| now_ms().checked_add(seconds.saturating_mul(1000)));
    Ok(token)
}

/// 删除本地 OAuth token 与待完成登录会话。
pub(crate) fn logout_oauth(request: McpOAuthLogoutRequest) -> AxonResult<McpOAuthLogoutProjection> {
    let token_file = token_path(&request.root, &request.server_name, &request.url);
    let pending_file = pending_path(&request.root, &request.server_name, &request.url);
    let removed_token = remove_file_if_exists(&token_file)?;
    let removed_pending = remove_file_if_exists(&pending_file)?;
    Ok(McpOAuthLogoutProjection {
        removed_token,
        removed_pending,
        token_value_exposed: false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PendingOAuthLogin {
    server_name: String,
    url: String,
    authorization_server: String,
    /// 通过 issuer 校验的授权服务器标识符。
    #[serde(default)]
    issuer: String,
    token_endpoint: Option<String>,
    client_id: String,
    /// OAuth client_secret；public client 为空。
    #[serde(default)]
    client_secret: Option<String>,
    /// 客户端注册来源。
    #[serde(default)]
    registration_source: String,
    redirect_uri: String,
    resource: String,
    scopes: Vec<String>,
    state: String,
    code_verifier: String,
    /// 授权服务器是否要求授权响应携带 RFC 9207 `iss`。
    #[serde(default)]
    require_authorization_response_iss: bool,
    created_at_ms: u64,
}

fn oauth_state(server_name: &str, url: &str) -> String {
    oauth_material("state", server_name, url)
}

fn oauth_code_verifier(server_name: &str, url: &str) -> String {
    oauth_material("verifier", server_name, url)
}

fn oauth_material(kind: &str, server_name: &str, url: &str) -> String {
    let now = now_ms();
    let mut hasher = Sha256Hex::new();
    hasher.update(kind.as_bytes());
    hasher.update(server_name.as_bytes());
    hasher.update(url.as_bytes());
    hasher.update(now.to_le_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}
