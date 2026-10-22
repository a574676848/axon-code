use std::path::Path;

use axon_agent_runtime::{
    McpAuthDiscoveryProjection, McpAuthDiscoveryRequest, McpAuthRuntime, McpCredentialStatus,
    McpHttpServerConfig, McpOAuthLoginStartProjection, McpOAuthLoginStartRequest,
    McpOAuthLogoutRequest, McpServerConfig,
};
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::cli_error,
    mcp_commands::{
        McpLoginCommand, McpLogoutCommand, McpServerView, discover_mcp_servers, server_view,
    },
};

use super::{mcp_browser_launch::open_authorization_url, mcp_render::render_oauth};

/// MCP OAuth 操作报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct McpOAuthReport {
    pub(super) status: &'static str,
    pub(super) server: McpServerView,
    pub(super) transport: &'static str,
    pub(super) authorization_url: Option<String>,
    pub(super) pending_ref: Option<String>,
    pub(super) redirect_uri: Option<String>,
    pub(super) scopes: Vec<String>,
    pub(super) client_registration_source: Option<String>,
    pub(super) credential_status: &'static str,
    pub(super) access_token_stored: bool,
    pub(super) refresh_token_stored: bool,
    pub(super) removed_token: bool,
    pub(super) removed_pending: bool,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) token_value_exposed: bool,
    pub(super) raw_config_exposed: bool,
    pub(super) command_config_exposed: bool,
    pub(super) remote_trigger_allowed: bool,
    pub(super) next_action: String,
}

pub(super) fn run_mcp_login(
    format: OutputFormat,
    root: &Path,
    cwd: Option<&Path>,
    command: &McpLoginCommand,
) -> CliRunResult {
    let Some((server, http)) = streamable_http_server(root, cwd, &command.server_id) else {
        return cli_error(format!(
            "未找到可 OAuth 登录的 Streamable HTTP MCP server：{}",
            command.server_id
        ));
    };
    let discovery = match discover_authorization(&http.url) {
        Ok(discovery) => discovery,
        Err(error) => return cli_error(error),
    };
    let Some(authorization_server) = discovery.authorization_server.clone() else {
        return cli_error("MCP server 未暴露 OAuth authorization server".to_owned());
    };
    let scopes = login_scopes(&command.scopes, &discovery);
    let session = match McpAuthRuntime::begin_oauth_login(McpOAuthLoginStartRequest {
        root: root.to_path_buf(),
        server_name: command.server_id.clone(),
        url: http.url,
        authorization_server,
        scopes,
        client_id: command.client_id.clone(),
        resource: command.resource.clone(),
        wait_for_callback: command.browser,
    }) {
        Ok(session) => session,
        Err(error) => return cli_error(format!("MCP OAuth 登录初始化失败：{error:?}")),
    };
    if command.browser
        && let Err(error) = open_authorization_url(session.authorization_url())
    {
        return cli_error(error);
    }
    let projection = match McpAuthRuntime::complete_oauth_login(session) {
        Ok(projection) => projection,
        Err(error) => return cli_error(format!("MCP OAuth 登录失败：{error:?}")),
    };
    render_oauth(
        format,
        &McpOAuthReport {
            status: if projection.callback_completed {
                "mcp_oauth_login_ready"
            } else {
                "mcp_oauth_login_pending"
            },
            server,
            transport: "streamable_http",
            authorization_url: Some(projection.authorization_url),
            pending_ref: Some(projection.pending_ref),
            redirect_uri: Some(projection.redirect_uri),
            scopes: projection.scopes,
            client_registration_source: Some(projection.client_registration_source),
            credential_status: if projection.callback_completed {
                credential_status_code(McpCredentialStatus::OAuthReady)
            } else {
                credential_status_code(McpCredentialStatus::OAuthPending)
            },
            access_token_stored: projection.access_token_stored,
            refresh_token_stored: projection.refresh_token_stored,
            removed_token: false,
            removed_pending: false,
            expires_at_ms: projection.expires_at_ms,
            token_value_exposed: projection.token_value_exposed,
            raw_config_exposed: false,
            command_config_exposed: false,
            remote_trigger_allowed: false,
            next_action: if projection.callback_completed {
                "无需操作".to_owned()
            } else {
                "打开 authorizationUrl 完成授权".to_owned()
            },
        },
    )
}

pub(super) fn run_mcp_logout(
    format: OutputFormat,
    root: &Path,
    cwd: Option<&Path>,
    command: &McpLogoutCommand,
) -> CliRunResult {
    let Some((server, http)) = streamable_http_server(root, cwd, &command.server_id) else {
        return cli_error(format!(
            "未找到可登出的 Streamable HTTP MCP server：{}",
            command.server_id
        ));
    };
    let projection = match McpAuthRuntime::logout_oauth(McpOAuthLogoutRequest {
        root: root.to_path_buf(),
        server_name: command.server_id.clone(),
        url: http.url,
    }) {
        Ok(projection) => projection,
        Err(error) => return cli_error(format!("MCP OAuth 登出失败：{error:?}")),
    };
    render_oauth(
        format,
        &McpOAuthReport {
            status: "mcp_oauth_logged_out",
            server,
            transport: "streamable_http",
            authorization_url: None,
            pending_ref: None,
            redirect_uri: None,
            scopes: Vec::new(),
            client_registration_source: None,
            credential_status: credential_status_code(McpCredentialStatus::OAuthAvailable),
            access_token_stored: false,
            refresh_token_stored: false,
            removed_token: projection.removed_token,
            removed_pending: projection.removed_pending,
            expires_at_ms: None,
            token_value_exposed: projection.token_value_exposed,
            raw_config_exposed: false,
            command_config_exposed: false,
            remote_trigger_allowed: false,
            next_action: "如需重新授权，运行 axon mcp login".to_owned(),
        },
    )
}

fn streamable_http_server(
    root: &Path,
    cwd: Option<&Path>,
    server_id: &str,
) -> Option<(McpServerView, McpHttpServerConfig)> {
    let discovered = discover_mcp_servers(root, cwd).ok()?;
    let discovered_server = discovered.get(server_id)?;
    let server = server_view(server_id, Some(discovered_server), None);
    match discovered_server.config.clone() {
        McpServerConfig::StreamableHttp(http) => Some((server, http)),
        McpServerConfig::Stdio(_) => None,
    }
}

/// 发现 MCP endpoint 的 OAuth authorization server 与 scope 候选。
///
/// MCP 2026-07-28 要求优先采用 401/403 challenge 声明的 metadata 与 scope，
/// 因此先做只读探测，再回退到 protected resource metadata。
fn discover_authorization(url: &str) -> Result<McpAuthDiscoveryProjection, String> {
    let www_authenticate = McpAuthRuntime::probe_www_authenticate(url).ok().flatten();
    McpAuthRuntime::discover(McpAuthDiscoveryRequest {
        url: url.to_owned(),
        www_authenticate,
        bearer_token_env_var: None,
    })
    .map_err(|error| format!("MCP OAuth metadata 发现失败：{error:?}"))
}

/// 选择登录 scope：显式参数优先，其次 challenge scope，最后 metadata scope 声明。
fn login_scopes(explicit: &[String], discovery: &McpAuthDiscoveryProjection) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    if !discovery.challenge_scopes.is_empty() {
        return discovery.challenge_scopes.clone();
    }
    discovery.scopes_supported.clone()
}

/// 交互式宿主内的 MCP OAuth 登录：拉起系统浏览器、等待本地回调并保存 token。
///
/// CLI `--browser` 与 TUI `/mcp login` 共用该实现，保证 scope 选择、客户端注册优先级
/// 与 token 落盘行为一致。
///
/// # Errors
/// server 缺失、metadata 发现失败、浏览器无法拉起或回调交换失败时返回可读错误。
pub(crate) fn perform_browser_mcp_login(
    root: &Path,
    cwd: Option<&Path>,
    server_id: &str,
) -> Result<String, String> {
    let (_, http) = streamable_http_server(root, cwd, server_id)
        .ok_or_else(|| format!("未找到可 OAuth 登录的 Streamable HTTP MCP server：{server_id}"))?;
    let discovery = discover_authorization(&http.url)?;
    let Some(authorization_server) = discovery.authorization_server.clone() else {
        return Err("MCP server 未暴露 OAuth authorization server".to_owned());
    };
    let session = McpAuthRuntime::begin_oauth_login(McpOAuthLoginStartRequest {
        root: root.to_path_buf(),
        server_name: server_id.to_owned(),
        url: http.url,
        authorization_server,
        scopes: login_scopes(&[], &discovery),
        client_id: None,
        resource: None,
        wait_for_callback: true,
    })
    .map_err(|error| format!("MCP OAuth 登录初始化失败：{error:?}"))?;
    open_authorization_url(session.authorization_url())?;
    let projection = McpAuthRuntime::complete_oauth_login(session)
        .map_err(|error| format!("MCP OAuth 登录失败：{error:?}"))?;
    Ok(browser_login_summary(server_id, &projection))
}

/// 交互式宿主内的 MCP OAuth 登出：清理本地 token 与待完成会话。
///
/// # Errors
/// server 缺失或本地凭据清理失败时返回可读错误。
pub(crate) fn perform_mcp_logout(
    root: &Path,
    cwd: Option<&Path>,
    server_id: &str,
) -> Result<String, String> {
    let (_, http) = streamable_http_server(root, cwd, server_id)
        .ok_or_else(|| format!("未找到可登出的 Streamable HTTP MCP server：{server_id}"))?;
    let projection = McpAuthRuntime::logout_oauth(McpOAuthLogoutRequest {
        root: root.to_path_buf(),
        server_name: server_id.to_owned(),
        url: http.url,
    })
    .map_err(|error| format!("MCP OAuth 登出失败：{error:?}"))?;
    Ok(format!(
        "{server_id} 已登出：清除 token={}，清除待完成会话={}",
        projection.removed_token, projection.removed_pending
    ))
}

/// 生成浏览器登录完成后的用户可见摘要。
fn browser_login_summary(server_id: &str, projection: &McpOAuthLoginStartProjection) -> String {
    if projection.access_token_stored {
        format!(
            "{server_id} 已完成 OAuth 授权，access token 已保存（client 注册来源：{}）",
            projection.client_registration_source
        )
    } else {
        format!("{server_id} 授权回调已返回，但未保存 access token，请重新发起登录")
    }
}

pub(super) fn credential_status_code(status: McpCredentialStatus) -> &'static str {
    match status {
        McpCredentialStatus::NotRequired => "not_required",
        McpCredentialStatus::EnvReady => "env_ready",
        McpCredentialStatus::EnvMissing => "env_missing",
        McpCredentialStatus::EnvEmpty => "env_empty",
        McpCredentialStatus::OAuthAvailable => "oauth_available",
        McpCredentialStatus::OAuthReady => "oauth_ready",
        McpCredentialStatus::OAuthPending => "oauth_pending",
        McpCredentialStatus::AuthRequired => "auth_required",
    }
}
