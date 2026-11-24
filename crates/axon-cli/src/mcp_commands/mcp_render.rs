use std::fmt::Write as _;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::json_result,
    mcp_commands::{
        McpGetReport, McpListReport, McpMutationReport, mcp_auth_commands::McpOAuthReport,
    },
};

pub(super) fn render_list(format: OutputFormat, report: &McpListReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.servers.is_empty() {
                stdout.push_str("未找到 MCP server\n");
            } else {
                for server in &report.servers {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\tactive={}\tbusy={}",
                        server.server_id,
                        server.doctor_status,
                        server.active_session_count,
                        server.busy_session_count
                    );
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

pub(super) fn render_get(format: OutputFormat, report: &McpGetReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "MCP server: {}\n- transport: {}\n- config_ref: {}\n- url: {}\n- bearer_token_env_var: {}\n- command_config_exposed: {}\n- raw_config_exposed: {}\n- token_value_exposed: {}\n- remote_trigger_allowed: {}\n",
                report.server.server_id,
                report.transport,
                report.server.config_ref,
                report.url.as_deref().unwrap_or(""),
                report.bearer_token_env_var.as_deref().unwrap_or(""),
                report.command_config_exposed,
                report.raw_config_exposed,
                report.token_value_exposed,
                report.remote_trigger_allowed
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_oauth(format: OutputFormat, report: &McpOAuthReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "MCP oauth: {}\n- status: {}\n- credential_status: {}\n- pending_ref: {}\n- access_token_stored: {}\n- token_value_exposed: {}\n- next_action: {}\n- remote_trigger_allowed: {}\n",
                report.server.server_id,
                report.status,
                report.credential_status,
                report.pending_ref.as_deref().unwrap_or(""),
                report.access_token_stored,
                report.token_value_exposed,
                report.next_action,
                report.remote_trigger_allowed
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_mutation(format: OutputFormat, report: &McpMutationReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "MCP {status}: {server_id}\n- config_ref: {config_ref}\n- remote_trigger_allowed: {remote}\n",
                status = report.status,
                server_id = report.server.server_id,
                config_ref = report.config_ref,
                remote = report.remote_trigger_allowed
            ),
            stderr: String::new(),
        },
    }
}
