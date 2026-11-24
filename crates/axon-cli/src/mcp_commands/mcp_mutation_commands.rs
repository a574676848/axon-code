use std::path::Path;

use axon_agent_runtime::mcp_server_id;
use axon_mcp_runtime::McpServerSource;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::cli_error,
    mcp_command_plan::server_config_from_add,
    mcp_commands::{
        McpAddCommand, McpMutationReport, McpRemoveCommand, discover_mcp_servers, display_path,
        server_view,
    },
    mcp_local_store::{remove_user_mcp_server, write_user_mcp_server},
};

use super::mcp_render::render_mutation;

pub(super) fn run_mcp_add(
    format: OutputFormat,
    root: &Path,
    command: &McpAddCommand,
) -> CliRunResult {
    let server_id = match mcp_server_id(&command.server_id) {
        Ok(server_id) => server_id,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let server_config = match server_config_from_add(command) {
        Ok(config) => config,
        Err(error) => return cli_error(error),
    };
    let discovered = match discover_mcp_servers(root, None) {
        Ok(discovered) => discovered,
        Err(error) => return cli_error(error),
    };
    if discovered
        .get(&server_id)
        .is_some_and(|server| server.source == McpServerSource::User)
    {
        return cli_error(format!("用户级 MCP server 已存在：{server_id}"));
    }
    let config_path = match write_user_mcp_server(root, &server_id, &server_config) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let discovered = match discover_mcp_servers(root, None) {
        Ok(discovered) => discovered,
        Err(error) => return cli_error(error),
    };
    let Some(server) = discovered.get(&server_id) else {
        return cli_error("MCP server 配置写入后无法读取".to_owned());
    };
    let report = McpMutationReport {
        status: "mcp_added",
        server: server_view(&server_id, Some(server), None),
        config_ref: server.config_ref.clone(),
        config_path: display_path(&config_path),
        config_written: true,
        removed: false,
        command_config_exposed: false,
        raw_config_exposed: false,
        remote_trigger_allowed: false,
    };
    render_mutation(format, &report)
}

pub(super) fn run_mcp_remove(
    format: OutputFormat,
    root: &Path,
    command: &McpRemoveCommand,
) -> CliRunResult {
    let server_id = match mcp_server_id(&command.server_id) {
        Ok(server_id) => server_id,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let discovered = match discover_mcp_servers(root, None) {
        Ok(discovered) => discovered,
        Err(error) => return cli_error(error),
    };
    let Some(discovered_server) = discovered.get(&server_id) else {
        return cli_error(format!("未找到 MCP server：{server_id}"));
    };
    if discovered_server.source != McpServerSource::User {
        return cli_error(format!(
            "MCP remove 仅删除用户级配置，Workspace server 不受影响：{server_id}"
        ));
    }
    let server = server_view(&server_id, Some(discovered_server), None);
    let config_path = match remove_user_mcp_server(root, &server_id) {
        Ok(path) => path,
        Err(error) => return cli_error(error),
    };
    let report = McpMutationReport {
        status: "mcp_removed",
        config_ref: server.config_ref.clone(),
        config_path: display_path(&config_path),
        server,
        config_written: true,
        removed: true,
        command_config_exposed: false,
        raw_config_exposed: false,
        remote_trigger_allowed: false,
    };
    render_mutation(format, &report)
}
