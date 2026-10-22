//! MCP server startup timeline 投影 runtime。

use axon_domain::{AxonResult, McpServerDefinition, McpServerStartupStatus, RuntimeCell};

/// MCP 配置引用前缀。
pub const MCP_CONFIG_REF_PREFIX: &str = "mcp://";
/// MCP startup 发现态审计原因。
pub const MCP_STARTUP_REASON_DISCOVERED: &str = "mcp:startup_discovered";

/// MCP startup timeline 投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStartupProjection {
    /// MCP server 稳定 ID。
    pub server_id: String,
    /// MCP 配置引用。
    pub config_ref: String,
    /// startup 状态。
    pub status: McpServerStartupStatus,
    /// 是否真实启动 server。
    pub server_process_started: bool,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 审计摘要。
    pub audit_summary: String,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// MCP startup runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpStartupRuntime;

impl McpStartupRuntime {
    /// 将 MCP 配置发现状态投影进 RuntimeCell timeline；不启动真实 server。
    pub fn project_startup(
        cell: &mut RuntimeCell,
        server: &McpServerDefinition,
    ) -> AxonResult<McpStartupProjection> {
        let server_id = server.server_id.clone();
        let config_ref = format!(
            "{}{}/{}",
            MCP_CONFIG_REF_PREFIX,
            server_id,
            axon_domain::MCP_CONFIG_FILE
        );
        let audit_summary = format!("MCP server {server_id} startup 已进入 timeline");
        cell.project_mcp_server_startup(
            server_id.clone(),
            config_ref.clone(),
            McpServerStartupStatus::Discovered,
            audit_summary.clone(),
        )?;

        Ok(McpStartupProjection {
            server_id,
            config_ref,
            status: McpServerStartupStatus::Discovered,
            server_process_started: false,
            remote_trigger_allowed: false,
            audit_summary,
            reasons: vec![MCP_STARTUP_REASON_DISCOVERED.to_owned()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axon_domain::{CellId, ModelProfileId, PaneId, PermissionSnapshotId, RuntimeCellEvent};

    #[test]
    fn mcp_startup_projects_discovered_state_without_process() {
        let mut cell = RuntimeCell::new(
            CellId(10),
            PaneId(1),
            "E:/workspace",
            ModelProfileId(3),
            PermissionSnapshotId(4),
        );
        cell.start().unwrap();

        let projection = McpStartupRuntime::project_startup(&mut cell, &mcp_report()).unwrap();

        assert_eq!(projection.status, McpServerStartupStatus::Discovered);
        assert!(!projection.server_process_started);
        assert!(!projection.remote_trigger_allowed);
        assert!(cell.events.iter().any(|event| {
            matches!(
                event,
                RuntimeCellEvent::McpServerStartupProjected {
                    server_id,
                    config_ref,
                    status: McpServerStartupStatus::Discovered,
                    ..
                } if server_id == "team-tools"
                    && config_ref == "mcp://team-tools/.mcp.json"
            )
        }));
    }

    fn mcp_report() -> McpServerDefinition {
        McpServerDefinition::new("team-tools").unwrap()
    }
}
