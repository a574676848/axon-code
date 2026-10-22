use std::{collections::BTreeMap, path::Path};

use axon_agent_runtime::{
    MCP_CONFIG_REF_PREFIX, McpPoolStoreRecord, McpPoolStoreRuntime, McpServerConfig,
};
pub(super) use axon_display::display_path;
use axon_mcp_runtime::{DiscoveredRuntimeMcpServer, McpServerSource};
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, resolved_config_path},
    runtime_paths::runtime_axon_root_path,
    runtime_tools,
};

mod mcp_auth_commands;
mod mcp_browser_launch;
mod mcp_cli_args;
mod mcp_mutation_commands;
mod mcp_render;
pub(crate) use mcp_auth_commands::{perform_browser_mcp_login, perform_mcp_logout};
use mcp_auth_commands::{run_mcp_login, run_mcp_logout};
pub use mcp_cli_args::*;
use mcp_mutation_commands::{run_mcp_add, run_mcp_remove};
use mcp_render::{render_get, render_list};

const MCP_CLI_DEFAULT_LIMIT: usize = 50;
const MCP_CLI_MAX_LIMIT: usize = 100;
const MCP_STATUS_UNKNOWN: &str = "unknown";

pub(crate) fn run_mcp(
    command: &McpCommand,
    format: OutputFormat,
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> CliRunResult {
    let path = match resolved_config_path(config_path) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let root = match runtime_axon_root_path(&path) {
        Ok(root) => root,
        Err(error) => return cli_error(error),
    };
    let subcommand = command
        .command
        .as_ref()
        .unwrap_or(&McpSubcommand::List(McpListCommand {
            limit: Some(MCP_CLI_DEFAULT_LIMIT),
        }));
    match subcommand {
        McpSubcommand::List(command) => run_mcp_list(format, &root, cwd, command),
        McpSubcommand::Get(command) => run_mcp_get(format, &root, cwd, command),
        McpSubcommand::Add(command) => run_mcp_add(format, &root, command),
        McpSubcommand::Remove(command) => run_mcp_remove(format, &root, command),
        McpSubcommand::Login(command) => run_mcp_login(format, &root, cwd, command),
        McpSubcommand::Logout(command) => run_mcp_logout(format, &root, cwd, command),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpListReport {
    status: &'static str,
    pool_store_path: String,
    limit: usize,
    total_servers: usize,
    servers: Vec<McpServerView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpGetReport {
    status: &'static str,
    pool_store_path: String,
    server: McpServerView,
    transport: &'static str,
    url: Option<String>,
    bearer_token_env_var: Option<String>,
    command_config_exposed: bool,
    raw_config_exposed: bool,
    token_value_exposed: bool,
    remote_trigger_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct McpMutationReport {
    status: &'static str,
    server: McpServerView,
    config_ref: String,
    config_path: String,
    config_written: bool,
    removed: bool,
    command_config_exposed: bool,
    raw_config_exposed: bool,
    remote_trigger_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct McpServerView {
    pub(super) server_id: String,
    pub(super) source: String,
    pub(super) config_ref: String,
    pub(super) discovered: bool,
    pub(super) doctor_status: String,
    pub(super) doctor_badge: String,
    pub(super) recorded_at_ms: Option<u64>,
    pub(super) active_session_count: usize,
    pub(super) busy_session_count: usize,
    pub(super) failure_count: u32,
    pub(super) backoff_remaining_ms: Option<u64>,
    pub(super) terminal_main_loop_continues: bool,
    pub(super) remote_trigger_allowed: bool,
}

fn run_mcp_list(
    format: OutputFormat,
    root: &Path,
    cwd: Option<&Path>,
    command: &McpListCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let pool_store_path = McpPoolStoreRuntime::resolve_store_path(root);
    let discovered = match discover_mcp_servers(root, cwd) {
        Ok(discovered) => discovered,
        Err(error) => return cli_error(error),
    };
    let latest_records = match latest_pool_records(&pool_store_path) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let total_servers = discovered.len();
    let servers = discovered
        .iter()
        .take(limit)
        .map(|(server_id, server)| {
            server_view(server_id, Some(server), latest_records.get(server_id))
        })
        .collect();
    let report = McpListReport {
        status: "mcp_ready",
        pool_store_path: display_path(&pool_store_path),
        limit,
        total_servers,
        servers,
    };
    render_list(format, &report)
}

fn run_mcp_get(
    format: OutputFormat,
    root: &Path,
    cwd: Option<&Path>,
    command: &McpGetCommand,
) -> CliRunResult {
    let pool_store_path = McpPoolStoreRuntime::resolve_store_path(root);
    let discovered = match discover_mcp_servers(root, cwd) {
        Ok(discovered) => discovered,
        Err(error) => return cli_error(error),
    };
    let latest_records = match latest_pool_records(&pool_store_path) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let Some(discovered_server) = discovered.get(&command.server_id) else {
        return cli_error(format!("未找到 MCP server：{}", command.server_id));
    };
    let (transport, url, bearer_token_env_var) = match &discovered_server.config {
        McpServerConfig::Stdio(_) => ("stdio", None, None),
        McpServerConfig::StreamableHttp(http) => (
            "streamable_http",
            Some(http.url.clone()),
            http.bearer_token_env_var.clone(),
        ),
    };
    let report = McpGetReport {
        status: "mcp_server_ready",
        pool_store_path: display_path(&pool_store_path),
        server: server_view(
            &command.server_id,
            Some(discovered_server),
            latest_records.get(&command.server_id),
        ),
        transport,
        url,
        bearer_token_env_var,
        command_config_exposed: false,
        raw_config_exposed: false,
        token_value_exposed: false,
        remote_trigger_allowed: false,
    };
    render_get(format, &report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiscoveredMcpServer {
    pub(super) server_id: String,
    pub(super) source: McpServerSource,
    pub(super) config_ref: String,
    pub(super) config: McpServerConfig,
}

pub(super) fn discover_mcp_servers(
    root: &Path,
    cwd: Option<&Path>,
) -> Result<BTreeMap<String, DiscoveredMcpServer>, String> {
    let home = runtime_tools::runtime_home();
    let current_dir = cwd
        .is_none()
        .then(std::env::current_dir)
        .transpose()
        .map_err(|_| "无法解析当前工作目录".to_owned())?;
    let discovery_cwd = cwd.or(current_dir.as_deref());
    axon_mcp_runtime::discover_runtime_mcp_servers(root, discovery_cwd, home.as_deref()).map(
        |servers| {
            servers
                .into_iter()
                .map(|(name, server)| (name, DiscoveredMcpServer::from(server)))
                .collect()
        },
    )
}

fn latest_pool_records(path: &Path) -> Result<BTreeMap<String, McpPoolStoreRecord>, String> {
    let records = McpPoolStoreRuntime::read_records(path).map_err(|error| format!("{error:?}"))?;
    let mut latest = BTreeMap::new();
    for record in records {
        let replace = latest
            .get(&record.server_id)
            .is_none_or(|old: &McpPoolStoreRecord| old.recorded_at_ms < record.recorded_at_ms);
        if replace {
            latest.insert(record.server_id.clone(), record);
        }
    }
    Ok(latest)
}

pub(super) fn server_view(
    server_id: &str,
    discovered: Option<&DiscoveredMcpServer>,
    record: Option<&McpPoolStoreRecord>,
) -> McpServerView {
    McpServerView {
        server_id: server_id.to_owned(),
        source: discovered
            .map_or("unknown", |server| server.source.as_str())
            .to_owned(),
        config_ref: discovered.map_or_else(
            || format!("{MCP_CONFIG_REF_PREFIX}{server_id}/.mcp.json"),
            |server| server.config_ref.clone(),
        ),
        discovered: discovered.is_some(),
        doctor_status: record.map_or_else(
            || MCP_STATUS_UNKNOWN.to_owned(),
            |record| record.doctor_status.clone(),
        ),
        doctor_badge: record.map_or_else(
            || MCP_STATUS_UNKNOWN.to_owned(),
            |record| record.doctor_badge.clone(),
        ),
        recorded_at_ms: record.map(|record| record.recorded_at_ms),
        active_session_count: record.map_or(0, |record| record.active_session_count),
        busy_session_count: record.map_or(0, |record| record.busy_session_count),
        failure_count: record.map_or(0, |record| record.failure_count),
        backoff_remaining_ms: record.and_then(|record| record.backoff_remaining_ms),
        terminal_main_loop_continues: record
            .is_none_or(|record| record.terminal_main_loop_continues),
        remote_trigger_allowed: false,
    }
}

impl From<DiscoveredRuntimeMcpServer> for DiscoveredMcpServer {
    fn from(server: DiscoveredRuntimeMcpServer) -> Self {
        Self {
            server_id: server.server_id,
            source: server.source,
            config_ref: server.config_ref,
            config: server.config,
        }
    }
}

fn bounded_limit(limit: Option<usize>) -> Result<usize, &'static str> {
    let limit = limit.unwrap_or(MCP_CLI_DEFAULT_LIMIT);
    if limit == 0 || limit > MCP_CLI_MAX_LIMIT {
        return Err("mcp limit 必须在 1..=100 范围内");
    }
    Ok(limit)
}
