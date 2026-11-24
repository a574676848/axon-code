//! MCP OAuth 本地凭据存储。

use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_crypto::{Sha256Hex, sha256_hex_prefix};
use axon_domain::{AxonError, AxonResult};
use axon_time::unix_now_ms;
use serde::{Deserialize, Serialize};

const OAUTH_STATE_DIR: &str = "mcp-oauth";
const OAUTH_PENDING_DIR: &str = "pending";
const OAUTH_TOKEN_DIR: &str = "tokens";
const OAUTH_CLIENT_DIR: &str = "clients";

/// 已保存的 MCP OAuth token。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStoredOAuthToken {
    /// MCP server 名称。
    pub server_name: String,
    /// MCP Streamable HTTP endpoint URL。
    pub url: String,
    /// 通过 issuer 一致性校验的授权服务器标识符；多授权服务器场景下用于隔离凭据。
    #[serde(default)]
    pub issuer: Option<String>,
    /// OAuth access token。
    pub access_token: String,
    /// OAuth refresh token。
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// OAuth token type。
    #[serde(default)]
    pub token_type: Option<String>,
    /// OAuth scope 字符串。
    #[serde(default)]
    pub scope: Option<String>,
    /// access token 过期时间。
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
    /// OAuth token endpoint，仅运行态凭据刷新使用。
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// OAuth public client_id，仅运行态凭据刷新使用。
    #[serde(default)]
    pub client_id: Option<String>,
    /// OAuth client_secret；public client 为空。
    #[serde(default)]
    pub client_secret: Option<String>,
    /// OAuth resource indicator，仅运行态凭据刷新使用。
    #[serde(default)]
    pub resource: Option<String>,
    /// 客户端注册来源，用于审计。
    #[serde(default)]
    pub registration_source: Option<String>,
}

/// 已缓存的 MCP OAuth 客户端注册结果。
///
/// 动态客户端注册每次登录都重新注册会在授权服务器累积无用 client，并多出一次
/// 网络往返，因此按「授权服务器 + redirect URI」缓存注册结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpStoredOAuthClient {
    /// OAuth authorization server 标识符。
    pub authorization_server: String,
    /// 注册时声明的 redirect URI。
    pub redirect_uri: String,
    /// OAuth client_id。
    pub client_id: String,
    /// OAuth client_secret；public client 为空。
    #[serde(default)]
    pub client_secret: Option<String>,
    /// 注册来源，用于审计。
    pub registration_source: String,
    /// 注册时间。
    pub registered_at_ms: u64,
}

pub(crate) fn token_path(root: &Path, server_name: &str, url: &str) -> PathBuf {
    root.join(OAUTH_STATE_DIR)
        .join(OAUTH_TOKEN_DIR)
        .join(format!("{}.json", short_oauth_hash(server_name, url)))
}

pub(crate) fn pending_path(root: &Path, server_name: &str, url: &str) -> PathBuf {
    root.join(OAUTH_STATE_DIR)
        .join(OAUTH_PENDING_DIR)
        .join(format!("{}.json", short_oauth_hash(server_name, url)))
}

pub(crate) fn oauth_ref(server_name: &str, url: &str) -> String {
    short_oauth_hash(server_name, url)
}

pub(crate) fn read_oauth_token(
    root: &Path,
    server_name: &str,
    url: &str,
) -> AxonResult<Option<McpStoredOAuthToken>> {
    let path = token_path(root, server_name, url);
    if !path.exists() {
        return Ok(None);
    }
    read_json(&path).map(Some)
}

pub(crate) fn client_path(root: &Path, authorization_server: &str, redirect_uri: &str) -> PathBuf {
    root.join(OAUTH_STATE_DIR)
        .join(OAUTH_CLIENT_DIR)
        .join(format!(
            "{}.json",
            short_oauth_hash(authorization_server, redirect_uri)
        ))
}

/// 读取缓存的客户端注册结果；文件缺失时返回 `None`。
pub(crate) fn read_oauth_client(
    root: &Path,
    authorization_server: &str,
    redirect_uri: &str,
) -> AxonResult<Option<McpStoredOAuthClient>> {
    let path = client_path(root, authorization_server, redirect_uri);
    if !path.exists() {
        return Ok(None);
    }
    read_json(&path).map(Some)
}

pub(crate) fn write_oauth_client(root: &Path, client: &McpStoredOAuthClient) -> AxonResult<()> {
    write_json(
        &client_path(root, &client.authorization_server, &client.redirect_uri),
        client,
    )
}

pub(crate) fn read_json<T>(path: &Path) -> AxonResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    let content = fs::read_to_string(path)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 凭据读取失败"))?;
    serde_json::from_str(&content)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 凭据 JSON 无效"))
}

pub(crate) fn write_json<T>(path: &Path, value: &T) -> AxonResult<()>
where
    T: Serialize,
{
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 凭据目录创建失败"))?;
    }
    let content = serde_json::to_string(value)
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 凭据序列化失败"))?;
    fs::write(path, content).map_err(|_| AxonError::InvalidMcpDefinition("MCP OAuth 凭据写入失败"))
}

pub(crate) fn remove_file_if_exists(path: &Path) -> AxonResult<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(AxonError::InvalidMcpDefinition("MCP OAuth 凭据删除失败")),
    }
}

pub(crate) fn now_ms() -> u64 {
    unix_now_ms()
}

fn short_oauth_hash(server_name: &str, url: &str) -> String {
    let mut hasher = Sha256Hex::new();
    hasher.update(server_name.as_bytes());
    hasher.update(b"\0");
    hasher.update(url.as_bytes());
    sha256_hex_prefix(hasher.finalize(), 32)
}
