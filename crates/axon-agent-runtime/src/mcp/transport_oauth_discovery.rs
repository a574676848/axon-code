//! MCP OAuth 2.1 授权服务器发现与协议校验（对齐 MCP 2026-07-28 授权章节）。
//!
//! 覆盖规范中的关键 MUST 项：
//! - 授权服务器 metadata 多端点发现优先级（RFC 8414 path insertion、OpenID Connect path insertion 与 path appending）；
//! - metadata `issuer` 与发现来源 issuer 的一致性校验，不一致或缺失时拒绝该文档；
//! - RFC 9728 Protected Resource Metadata 的 well-known 回退顺序（端点子路径优先，根路径兜底）；
//! - RFC 8707 `resource` 参数规范化；
//! - RFC 9207 授权响应 `iss` 校验。

use std::time::Duration;

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;
use url::Url;

use crate::mcp_transport_http_client::{MCP_METADATA_TIMEOUT_SECS, shared_http_client};

const OAUTH_AUTHORIZATION_SERVER_WELL_KNOWN: &str = ".well-known/oauth-authorization-server";
const OPENID_CONFIGURATION_WELL_KNOWN: &str = ".well-known/openid-configuration";
const OAUTH_PROTECTED_RESOURCE_WELL_KNOWN: &str = ".well-known/oauth-protected-resource";

/// 授权服务器 metadata 发现端点候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorizationServerMetadataEndpoint {
    /// 完整发现 URL。
    pub url: String,
    /// 候选来源：`oauth_authorization_server` 或 `openid_configuration`。
    pub kind: &'static str,
}

/// 已通过 issuer 校验的授权服务器 metadata。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiscoveredAuthorizationServerMetadata {
    /// 权威 issuer 标识符。
    pub issuer: String,
    /// 授权端点。
    pub authorization_endpoint: String,
    /// token 端点。
    pub token_endpoint: Option<String>,
    /// 动态客户端注册端点。
    pub registration_endpoint: Option<String>,
    /// 是否声明支持 Client ID Metadata Documents。
    pub client_id_metadata_document_supported: bool,
    /// 是否声明支持授权响应 `iss` 参数（RFC 9207）。
    pub authorization_response_iss_parameter_supported: bool,
    /// 授权服务器支持的 scopes。
    pub scopes_supported: Vec<String>,
}

/// 按 MCP 2026-07-28 优先级构造授权服务器 metadata 端点候选。
///
/// 带 path 的 issuer 依次尝试 OAuth Authorization Server Metadata path insertion、
/// OpenID Connect Discovery path insertion 与 path appending；无 path 的 issuer
/// 依次尝试两个根 well-known 端点。
pub(crate) fn authorization_server_metadata_endpoints(
    issuer: &Url,
) -> Vec<AuthorizationServerMetadataEndpoint> {
    let origin = issuer.origin().ascii_serialization();
    let path = issuer.path().trim_end_matches('/');
    if path.is_empty() {
        return vec![
            AuthorizationServerMetadataEndpoint {
                url: format!("{origin}/{OAUTH_AUTHORIZATION_SERVER_WELL_KNOWN}"),
                kind: "oauth_authorization_server",
            },
            AuthorizationServerMetadataEndpoint {
                url: format!("{origin}/{OPENID_CONFIGURATION_WELL_KNOWN}"),
                kind: "openid_configuration",
            },
        ];
    }
    vec![
        AuthorizationServerMetadataEndpoint {
            url: format!("{origin}/{OAUTH_AUTHORIZATION_SERVER_WELL_KNOWN}{path}"),
            kind: "oauth_authorization_server",
        },
        AuthorizationServerMetadataEndpoint {
            url: format!("{origin}/{OPENID_CONFIGURATION_WELL_KNOWN}{path}"),
            kind: "openid_configuration",
        },
        AuthorizationServerMetadataEndpoint {
            url: format!("{origin}{path}/{OPENID_CONFIGURATION_WELL_KNOWN}"),
            kind: "openid_configuration",
        },
    ]
}

/// 发现授权服务器 metadata；全部候选失败时返回 `None`。
///
/// 每个候选文档都必须通过 RFC 8414 §3.3 的 issuer 一致性校验；`issuer` 缺失或
/// 与发现来源不一致的文档会被直接拒绝，不会被采用。
pub(crate) fn discover_authorization_server_metadata(
    authorization_server: &str,
) -> AxonResult<Option<DiscoveredAuthorizationServerMetadata>> {
    let issuer = Url::parse(authorization_server)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth authorization server URL 无效"))?;
    let expected_issuer = issuer.as_str().to_owned();
    for endpoint in authorization_server_metadata_endpoints(&issuer) {
        let Some(document) = fetch_json_document(&endpoint.url)? else {
            continue;
        };
        let Some(received_issuer) = document.get("issuer").and_then(Value::as_str) else {
            continue;
        };
        if !issuer_identifiers_match(received_issuer, &expected_issuer) {
            continue;
        }
        let Some(authorization_endpoint) = string_field(&document, "authorization_endpoint") else {
            continue;
        };
        return Ok(Some(DiscoveredAuthorizationServerMetadata {
            issuer: received_issuer.to_owned(),
            authorization_endpoint,
            token_endpoint: string_field(&document, "token_endpoint"),
            registration_endpoint: string_field(&document, "registration_endpoint"),
            client_id_metadata_document_supported: bool_field(
                &document,
                "client_id_metadata_document_supported",
            ),
            authorization_response_iss_parameter_supported: bool_field(
                &document,
                "authorization_response_iss_parameter_supported",
            ),
            scopes_supported: string_array_field(&document, "scopes_supported"),
        }));
    }
    Ok(None)
}

/// RFC 9728 Protected Resource Metadata 的 well-known 回退候选。
///
/// MCP 端点位于子路径时先探测端点子路径形式，再回退到站点根路径形式。
pub(crate) fn protected_resource_metadata_endpoints(endpoint: &Url) -> Vec<String> {
    let origin = endpoint.origin().ascii_serialization();
    let path = endpoint.path().trim_end_matches('/');
    let mut endpoints = Vec::new();
    if !path.is_empty() {
        endpoints.push(format!(
            "{origin}/{OAUTH_PROTECTED_RESOURCE_WELL_KNOWN}{path}"
        ));
    }
    endpoints.push(format!("{origin}/{OAUTH_PROTECTED_RESOURCE_WELL_KNOWN}"));
    endpoints
}

/// RFC 8707 canonical MCP server URI。
///
/// 规范化去除 fragment 与非语义尾斜杠；scheme 与 host 由 URL 解析器归一化为小写。
/// MCP 规范要求站点根统一使用不带尾斜杠的形式，而 `url::Url` 会把空 path 渲染回
/// `/`，因此这里显式拼装最终字符串。
pub(crate) fn canonical_resource_uri(endpoint: &str) -> AxonResult<String> {
    let mut url = Url::parse(endpoint)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth resource URL 无效"))?;
    if url.fragment().is_some() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth resource URL 不允许 fragment",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth resource URL 不允许 userinfo",
        ));
    }
    let path = url.path();
    if path.ends_with('/') {
        let trimmed = path.trim_end_matches('/').to_owned();
        url.set_path(&trimmed);
    }
    Ok(render_canonical_resource(&url))
}

fn render_canonical_resource(url: &Url) -> String {
    let mut canonical = format!("{}://", url.scheme());
    if let Some(host) = url.host_str() {
        canonical.push_str(host);
    }
    if let Some(port) = url.port() {
        canonical.push(':');
        canonical.push_str(&port.to_string());
    }
    // `url::Url` 会把空 path 规范化为 `/`，站点根按 MCP 规范省略该斜杠。
    let path = url.path();
    if path != "/" {
        canonical.push_str(path);
    }
    if let Some(query) = url.query() {
        canonical.push('?');
        canonical.push_str(query);
    }
    canonical
}

/// RFC 8414 issuer 标识符比较。
///
/// 仅容忍站点根路径的尾斜杠差异，其余字符保持精确比较。
pub(crate) fn issuer_identifiers_match(received_issuer: &str, expected_issuer: &str) -> bool {
    if received_issuer == expected_issuer {
        return true;
    }
    trim_root_slash(received_issuer) == trim_root_slash(expected_issuer)
}

/// RFC 9207 授权响应 `iss` 校验。
///
/// 记录到期望 issuer 时使用逐字符比较，不做任何归一化；`require_iss` 来自
/// 授权服务器 metadata 的 `authorization_response_iss_parameter_supported`。
pub(crate) fn validate_authorization_response_issuer(
    expected_issuer: Option<&str>,
    require_iss: bool,
    received_issuer: Option<&str>,
) -> AxonResult<()> {
    let Some(expected_issuer) = expected_issuer else {
        if received_issuer.is_some() || require_iss {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP OAuth 授权响应 issuer 缺少可校验的期望值",
            ));
        }
        return Ok(());
    };
    let Some(received_issuer) = received_issuer else {
        if require_iss {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP OAuth 授权响应缺少必需的 iss 参数",
            ));
        }
        return Ok(());
    };
    if received_issuer != expected_issuer {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP OAuth 授权响应 iss 与授权服务器不匹配",
        ));
    }
    Ok(())
}

fn trim_root_slash(issuer: &str) -> String {
    issuer
        .strip_suffix('/')
        .filter(|without_slash| {
            Url::parse(without_slash)
                .map(|url| url.path().is_empty() || url.path() == "/")
                .unwrap_or(false)
        })
        .unwrap_or(issuer)
        .to_owned()
}

fn fetch_json_document(url: &str) -> AxonResult<Option<Value>> {
    let client = shared_http_client()?;
    let Ok(response) = client
        .get(url)
        .timeout(Duration::from_secs(MCP_METADATA_TIMEOUT_SECS))
        .send()
    else {
        return Ok(None);
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    let Ok(text) = response.text() else {
        return Ok(None);
    };
    Ok(serde_json::from_str::<Value>(&text).ok())
}

fn string_field(document: &Value, key: &str) -> Option<String> {
    document
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn bool_field(document: &Value, key: &str) -> bool {
    document.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn string_array_field(document: &Value, key: &str) -> Vec<String> {
    document
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("测试 URL 应可解析")
    }

    #[test]
    fn authorization_server_metadata_endpoints_follow_mcp_priority_for_path_issuer() {
        let endpoints =
            authorization_server_metadata_endpoints(&url("https://auth.example.com/tenant1"));
        let urls = endpoints
            .iter()
            .map(|item| item.url.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            [
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn authorization_server_metadata_endpoints_cover_root_issuer() {
        let endpoints = authorization_server_metadata_endpoints(&url("https://auth.example.com"));
        let urls = endpoints
            .iter()
            .map(|item| item.url.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            urls,
            [
                "https://auth.example.com/.well-known/oauth-authorization-server",
                "https://auth.example.com/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn protected_resource_metadata_endpoints_prefer_endpoint_sub_path() {
        let endpoints =
            protected_resource_metadata_endpoints(&url("https://mcp.example.com/public/mcp"));
        assert_eq!(
            endpoints,
            [
                "https://mcp.example.com/.well-known/oauth-protected-resource/public/mcp",
                "https://mcp.example.com/.well-known/oauth-protected-resource",
            ]
        );
    }

    #[test]
    fn protected_resource_metadata_endpoints_fall_back_to_root_only_for_root_endpoint() {
        let endpoints = protected_resource_metadata_endpoints(&url("https://mcp.example.com/mcp"));
        assert_eq!(
            endpoints,
            [
                "https://mcp.example.com/.well-known/oauth-protected-resource/mcp",
                "https://mcp.example.com/.well-known/oauth-protected-resource",
            ]
        );
    }

    #[test]
    fn canonical_resource_uri_drops_fragment_and_trailing_slash() {
        assert_eq!(
            canonical_resource_uri("https://mcp.example.com/").expect("resource 应可规范化"),
            "https://mcp.example.com"
        );
        assert_eq!(
            canonical_resource_uri("https://mcp.example.com/server/mcp/")
                .expect("resource 应可规范化"),
            "https://mcp.example.com/server/mcp"
        );
        assert!(canonical_resource_uri("https://mcp.example.com/mcp#fragment").is_err());
    }

    #[test]
    fn issuer_match_only_tolerates_root_slash_difference() {
        assert!(issuer_identifiers_match(
            "https://auth.example.com",
            "https://auth.example.com/"
        ));
        assert!(issuer_identifiers_match(
            "https://auth.example.com/tenant1",
            "https://auth.example.com/tenant1"
        ));
        assert!(!issuer_identifiers_match(
            "https://auth.example.com/tenant1/",
            "https://auth.example.com/tenant1"
        ));
        assert!(!issuer_identifiers_match(
            "https://evil.example.com",
            "https://auth.example.com"
        ));
    }

    #[test]
    fn authorization_response_issuer_requires_matching_iss_when_required() {
        assert!(
            validate_authorization_response_issuer(
                Some("https://auth.example.com"),
                true,
                Some("https://auth.example.com")
            )
            .is_ok()
        );
        assert!(
            validate_authorization_response_issuer(Some("https://auth.example.com"), true, None)
                .is_err()
        );
        assert!(
            validate_authorization_response_issuer(Some("https://auth.example.com"), false, None)
                .is_ok()
        );
        assert!(
            validate_authorization_response_issuer(
                Some("https://auth.example.com"),
                false,
                Some("https://evil.example.com")
            )
            .is_err()
        );
    }
}
