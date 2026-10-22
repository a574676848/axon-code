//! MCP OAuth 客户端注册（对齐 MCP 2026-07-28 Client Registration 章节）。
//!
//! 规范给出的优先级：
//! 1. 预注册 client 信息（用户已持有注册结果时）；
//! 2. Client ID Metadata Documents（授权服务器声明 `client_id_metadata_document_supported`）；
//! 3. Dynamic Client Registration（授权服务器暴露 `registration_endpoint`，RFC 7591 已
//!    标记 deprecated，仅作为向后兼容回退）；
//! 4. 均不可用时提示用户显式提供 client 信息。
//!
//! Axon CLI 自身无法托管 HTTPS client metadata 文档，因此 CIMD 仅在用户显式提供
//! HTTPS URL 形式的 client_id 时启用。

use axon_domain::{AxonError, AxonResult};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use serde::{Deserialize, Serialize};

use crate::mcp_transport_http_client::shared_http_client;
use crate::mcp_transport_oauth_discovery::DiscoveredAuthorizationServerMetadata;
use crate::mcp_transport_oauth_flow::OAuthErrorResponse;
use crate::mcp_transport_oauth_store::{
    McpStoredOAuthClient, now_ms, read_oauth_client, write_oauth_client,
};

const OAUTH_REGISTRATION_TIMEOUT_SECS: u64 = 15;
const OAUTH_REGISTRATION_CONTENT_TYPE: &str = "application/json";
const OAUTH_APPLICATION_TYPE_NATIVE: &str = "native";
const OAUTH_CLIENT_NAME: &str = "Axon CLI";
const OAUTH_GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";
const OAUTH_GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";
const OAUTH_RESPONSE_TYPE_CODE: &str = "code";
const OAUTH_TOKEN_ENDPOINT_AUTH_METHOD_NONE: &str = "none";
const OAUTH_CLIENT_ID_HTTPS_SCHEME: &str = "https";

/// 客户端注册来源，用于审计与投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpClientRegistrationSource {
    /// 用户显式提供的已注册 client。
    PreRegistered,
    /// 用户提供的 Client ID Metadata Document URL。
    ClientIdMetadataDocument,
    /// 通过动态客户端注册获得。
    DynamicClientRegistration,
}

impl McpClientRegistrationSource {
    /// 投影为稳定的审计字符串。
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::PreRegistered => "pre_registered",
            Self::ClientIdMetadataDocument => "client_id_metadata_document",
            Self::DynamicClientRegistration => "dynamic_client_registration",
        }
    }
}

/// 已解析的 OAuth 客户端注册结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct McpClientRegistration {
    /// OAuth client_id。
    pub client_id: String,
    /// OAuth client_secret；public client 为空。
    pub client_secret: Option<String>,
    /// 注册来源。
    pub source: McpClientRegistrationSource,
}

/// 按规范优先级解析客户端注册。
///
/// 预注册与 CIMD 不产生网络请求；仅在两者都不可用时才会调用动态注册端点。
/// 动态注册结果会按「授权服务器 + redirect URI」缓存复用，避免每次登录都重新注册。
pub(crate) fn resolve_client_registration(
    root: &std::path::Path,
    authorization_server: &str,
    pre_registered_client_id: Option<&str>,
    metadata: &DiscoveredAuthorizationServerMetadata,
    redirect_uri: &str,
) -> AxonResult<McpClientRegistration> {
    if let Some(client_id) = pre_registered_client_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let source = if is_client_id_metadata_document(client_id)
            && metadata.client_id_metadata_document_supported
        {
            McpClientRegistrationSource::ClientIdMetadataDocument
        } else {
            McpClientRegistrationSource::PreRegistered
        };
        return Ok(McpClientRegistration {
            client_id: client_id.to_owned(),
            client_secret: None,
            source,
        });
    }
    if let Some(cached) = cached_client_registration(root, authorization_server, redirect_uri) {
        return Ok(cached);
    }
    let Some(registration_endpoint) = metadata
        .registration_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth 授权服务器未提供动态客户端注册端点，请通过 client_id 提供预注册客户端",
        ));
    };
    let registration = dynamic_client_registration(registration_endpoint, redirect_uri)?;
    cache_client_registration(root, authorization_server, redirect_uri, &registration);
    Ok(registration)
}

/// 读取缓存的动态注册结果；缺失或内容不可用时视为未注册。
fn cached_client_registration(
    root: &std::path::Path,
    authorization_server: &str,
    redirect_uri: &str,
) -> Option<McpClientRegistration> {
    let cached = read_oauth_client(root, authorization_server, redirect_uri)
        .ok()
        .flatten()?;
    let client_id = cached.client_id.trim();
    if client_id.is_empty()
        || cached.authorization_server != authorization_server
        || cached.redirect_uri != redirect_uri
    {
        return None;
    }
    Some(McpClientRegistration {
        client_id: client_id.to_owned(),
        client_secret: cached
            .client_secret
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()),
        source: McpClientRegistrationSource::DynamicClientRegistration,
    })
}

/// 持久化动态注册结果；写盘失败只影响下次复用，不阻断本次登录。
fn cache_client_registration(
    root: &std::path::Path,
    authorization_server: &str,
    redirect_uri: &str,
    registration: &McpClientRegistration,
) {
    let stored = McpStoredOAuthClient {
        authorization_server: authorization_server.to_owned(),
        redirect_uri: redirect_uri.to_owned(),
        client_id: registration.client_id.clone(),
        client_secret: registration.client_secret.clone(),
        registration_source: registration.source.as_str().to_owned(),
        registered_at_ms: now_ms(),
    };
    let _ = write_oauth_client(root, &stored);
}

fn is_client_id_metadata_document(client_id: &str) -> bool {
    client_id.starts_with(&format!("{OAUTH_CLIENT_ID_HTTPS_SCHEME}://"))
}

#[derive(Debug, Serialize)]
struct DynamicClientRegistrationRequest<'a> {
    client_name: &'a str,
    application_type: &'a str,
    redirect_uris: [&'a str; 1],
    grant_types: [&'a str; 2],
    response_types: [&'a str; 1],
    token_endpoint_auth_method: &'a str,
}

#[derive(Debug, Deserialize)]
struct DynamicClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

fn dynamic_client_registration(
    registration_endpoint: &str,
    redirect_uri: &str,
) -> AxonResult<McpClientRegistration> {
    let request = DynamicClientRegistrationRequest {
        client_name: OAUTH_CLIENT_NAME,
        application_type: OAUTH_APPLICATION_TYPE_NATIVE,
        redirect_uris: [redirect_uri],
        grant_types: [
            OAUTH_GRANT_TYPE_AUTHORIZATION_CODE,
            OAUTH_GRANT_TYPE_REFRESH_TOKEN,
        ],
        response_types: [OAUTH_RESPONSE_TYPE_CODE],
        token_endpoint_auth_method: OAUTH_TOKEN_ENDPOINT_AUTH_METHOD_NONE,
    };
    ensure_rustls_crypto_provider();
    let body = serde_json::to_string(&request)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 动态客户端注册请求序列化失败"))?;
    let response = shared_http_client()?
        .post(registration_endpoint)
        .timeout(std::time::Duration::from_secs(
            OAUTH_REGISTRATION_TIMEOUT_SECS,
        ))
        .header(
            reqwest::header::CONTENT_TYPE,
            OAUTH_REGISTRATION_CONTENT_TYPE,
        )
        .body(body)
        .send()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 动态客户端注册请求失败"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 动态客户端注册响应读取失败"))?;
    let response: DynamicClientRegistrationResponse =
        serde_json::from_str(&text).map_err(|_| registration_error(status, &text))?;
    let client_id = response.client_id.trim();
    if client_id.is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth 动态客户端注册未返回 client_id",
        ));
    }
    Ok(McpClientRegistration {
        client_id: client_id.to_owned(),
        client_secret: response
            .client_secret
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty()),
        source: McpClientRegistrationSource::DynamicClientRegistration,
    })
}

/// 把动态注册错误响应投影为可读错误，保留 RFC 7591 `error` 与描述。
fn registration_error(status: reqwest::StatusCode, body: &str) -> AxonError {
    let Ok(error) = serde_json::from_str::<OAuthErrorResponse>(body) else {
        return AxonError::InvalidMcpDefinition("MCP OAuth 动态客户端注册响应无效");
    };
    match error
        .error_description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(description) => AxonError::invalid_input(format!(
            "MCP OAuth 动态客户端注册失败：{}（{description}），HTTP {status}",
            error.error
        )),
        None => AxonError::invalid_input(format!(
            "MCP OAuth 动态客户端注册失败：{}，HTTP {status}",
            error.error
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_AUTHORIZATION_SERVER: &str = "https://auth.example.com";
    const TEST_REDIRECT_URI: &str = "http://127.0.0.1:9000/oauth/callback";

    fn test_root(label: &str) -> std::path::PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon-oauth-registration-{label}-{}-{}",
            std::process::id(),
            now_ms()
        ))
    }

    fn metadata(
        registration_endpoint: Option<&str>,
        cimd_supported: bool,
    ) -> DiscoveredAuthorizationServerMetadata {
        DiscoveredAuthorizationServerMetadata {
            issuer: TEST_AUTHORIZATION_SERVER.to_owned(),
            authorization_endpoint: "https://auth.example.com/sso".to_owned(),
            token_endpoint: None,
            registration_endpoint: registration_endpoint.map(str::to_owned),
            client_id_metadata_document_supported: cimd_supported,
            authorization_response_iss_parameter_supported: false,
            scopes_supported: Vec::new(),
        }
    }

    #[test]
    fn pre_registered_client_takes_priority_without_network_access() {
        let root = test_root("pre-registered");
        let registration = resolve_client_registration(
            &root,
            TEST_AUTHORIZATION_SERVER,
            Some("axon-mcp-cli"),
            &metadata(Some("https://auth.example.com/register"), true),
            TEST_REDIRECT_URI,
        )
        .expect("预注册 client 应直接可用");

        assert_eq!(registration.client_id, "axon-mcp-cli");
        assert_eq!(
            registration.source,
            McpClientRegistrationSource::PreRegistered
        );
        assert_eq!(registration.client_secret, None);
        assert!(
            read_oauth_client(&root, TEST_AUTHORIZATION_SERVER, TEST_REDIRECT_URI)
                .expect("缓存读取应成功")
                .is_none()
        );
    }

    #[test]
    fn https_client_id_is_reported_as_metadata_document_when_supported() {
        let root = test_root("cimd");
        let registration = resolve_client_registration(
            &root,
            TEST_AUTHORIZATION_SERVER,
            Some("https://app.example.com/oauth/client-metadata.json"),
            &metadata(None, true),
            TEST_REDIRECT_URI,
        )
        .expect("CIMD client 应直接可用");

        assert_eq!(
            registration.source,
            McpClientRegistrationSource::ClientIdMetadataDocument
        );
    }

    #[test]
    fn missing_registration_endpoint_requires_explicit_client_id() {
        let root = test_root("missing-endpoint");
        let error = resolve_client_registration(
            &root,
            TEST_AUTHORIZATION_SERVER,
            None,
            &metadata(None, false),
            TEST_REDIRECT_URI,
        )
        .expect_err("缺少注册机制时应提示用户提供 client_id");

        assert!(format!("{error:?}").contains("client_id"));
    }

    #[test]
    fn cached_dynamic_registration_is_reused_without_network_access() {
        let root = test_root("cached");
        write_oauth_client(
            &root,
            &McpStoredOAuthClient {
                authorization_server: TEST_AUTHORIZATION_SERVER.to_owned(),
                redirect_uri: TEST_REDIRECT_URI.to_owned(),
                client_id: "mcp-cached-client".to_owned(),
                client_secret: None,
                registration_source: McpClientRegistrationSource::DynamicClientRegistration
                    .as_str()
                    .to_owned(),
                registered_at_ms: now_ms(),
            },
        )
        .expect("缓存写入应成功");

        // 注册端点不可达：命中缓存时不产生任何网络请求。
        let registration = resolve_client_registration(
            &root,
            TEST_AUTHORIZATION_SERVER,
            None,
            &metadata(Some("https://127.0.0.1:9/register"), false),
            TEST_REDIRECT_URI,
        )
        .expect("命中缓存应直接复用 client_id");

        assert_eq!(registration.client_id, "mcp-cached-client");
        assert_eq!(
            registration.source,
            McpClientRegistrationSource::DynamicClientRegistration
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cached_dynamic_registration_is_isolated_by_redirect_uri() {
        let root = test_root("redirect-scope");
        write_oauth_client(
            &root,
            &McpStoredOAuthClient {
                authorization_server: TEST_AUTHORIZATION_SERVER.to_owned(),
                redirect_uri: "http://127.0.0.1:1/oauth/callback".to_owned(),
                client_id: "mcp-other-redirect".to_owned(),
                client_secret: None,
                registration_source: McpClientRegistrationSource::DynamicClientRegistration
                    .as_str()
                    .to_owned(),
                registered_at_ms: now_ms(),
            },
        )
        .expect("缓存写入应成功");

        // redirect URI 不同即视为未注册：仍需访问注册端点并因此失败。
        let error = resolve_client_registration(
            &root,
            TEST_AUTHORIZATION_SERVER,
            None,
            &metadata(Some("https://127.0.0.1:9/register"), false),
            TEST_REDIRECT_URI,
        )
        .expect_err("不同 redirect URI 不应复用缓存 client");

        assert!(format!("{error:?}").contains("动态客户端注册请求失败"));
        let _ = std::fs::remove_dir_all(root);
    }
}
