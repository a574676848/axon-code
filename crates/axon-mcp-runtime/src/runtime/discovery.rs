use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{McpServerConfig, mcp_server_id};
use axon_domain::McpServerDefinition;

/// MCP server 配置来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerSource {
    /// 用户级 `~/.axon/.mcp.json` 提供。
    User,
    /// 当前 Workspace 的 `.mcp.json` 提供。
    Workspace,
}

impl McpServerSource {
    /// 稳定来源代码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Workspace => "workspace",
        }
    }

    /// Provider 动态上下文中的有效作用域。
    #[must_use]
    pub const fn context_scope(self) -> &'static str {
        match self {
            Self::User => "global",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeMcpServer {
    pub(crate) definition: McpServerDefinition,
    pub(crate) source: McpServerSource,
    pub(crate) config_path: PathBuf,
    pub(crate) server_name: String,
    pub(crate) config: McpServerConfig,
}

impl RuntimeMcpServer {
    /// 规范化后的 server ID，用于 OAuth 凭据键与工具命名空间。
    pub(crate) fn server_id(&self) -> &str {
        &self.definition.server_id
    }

    pub(crate) fn config_ref(&self) -> String {
        format!("mcp://{}/{}", self.config_path.display(), self.server_name)
    }

    pub(crate) fn mcp_config(&self) -> Result<McpServerConfig, String> {
        Ok(self.config.clone())
    }

    pub(crate) fn matches_name(&self, candidate: &str) -> bool {
        self.server_name == candidate
            || self.definition.server_id == candidate
            || mcp_server_id(candidate)
                .is_ok_and(|server_id| server_id == self.definition.server_id)
    }
}

/// MCP server 发现结果的安全运行时投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRuntimeMcpServer {
    /// MCP server 稳定 ID。
    pub server_id: String,
    /// MCP server 配置来源。
    pub source: McpServerSource,
    /// 不暴露原始配置内容的配置引用。
    pub config_ref: String,
    /// 已解析的 MCP server 配置。
    pub config: McpServerConfig,
    /// 运行时 MCP server 定义。
    pub definition: McpServerDefinition,
}

pub(crate) fn discover_runtime_servers(
    root: &Path,
    cwd: Option<&Path>,
    _home: Option<&Path>,
) -> Result<Vec<RuntimeMcpServer>, String> {
    let mut servers = BTreeMap::<String, RuntimeMcpServer>::new();
    if let Some(cwd) = cwd {
        load_config_file(
            &cwd.join(".mcp.json"),
            McpServerSource::Workspace,
            &mut servers,
        )?;
    }
    load_config_file(&root.join(".mcp.json"), McpServerSource::User, &mut servers)?;
    Ok(servers.into_values().collect())
}

/// 发现当前 User 与 Workspace 可用的 MCP server。
pub fn discover_runtime_mcp_servers(
    root: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Result<BTreeMap<String, DiscoveredRuntimeMcpServer>, String> {
    discover_runtime_servers(root, cwd, home).map(|servers| {
        servers
            .into_iter()
            .map(|server| {
                let server_id = server.definition.server_id.clone();
                (
                    server_id.clone(),
                    DiscoveredRuntimeMcpServer {
                        server_id,
                        source: server.source,
                        config_ref: server.config_ref(),
                        config: server.config,
                        definition: server.definition,
                    },
                )
            })
            .collect()
    })
}

fn load_config_file(
    path: &Path,
    source: McpServerSource,
    servers: &mut BTreeMap<String, RuntimeMcpServer>,
) -> Result<(), String> {
    for (server_name, config) in crate::axon_mcp_config::mcp_servers_from_path(path) {
        let server_id = mcp_server_id(&server_name)
            .map_err(|error| format!("MCP server ID 无效：{error:?}"))?;
        if let Some(existing) = servers.get(&server_id)
            && existing.source == source
            && existing.server_name != server_name
        {
            return Err(format!("MCP server ID 冲突：{server_id}"));
        }
        let definition = McpServerDefinition::new(&server_id)
            .map_err(|error| format!("MCP server 定义无效：{error:?}"))?;
        servers.insert(
            server_id,
            RuntimeMcpServer {
                definition,
                source,
                config_path: path.to_path_buf(),
                server_name,
                config,
            },
        );
    }
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn runtime_home() -> Option<PathBuf> {
    axon_config::user_home_dir().ok()
}

#[cfg(test)]
pub(crate) fn runtime_home() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn discovers_user_and_workspace_servers_with_user_precedence() {
        let root = test_root("precedence");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"user-db":{"type":"stdio","command":"user-mcp"},"shared":{"type":"stdio","command":"user-shared"}}}"#,
        )
        .unwrap();
        fs::write(
            workspace.join(".mcp.json"),
            r#"{"mcpServers":{"workspace-db":{"type":"streamable_http","url":"http://127.0.0.1:9/mcp"},"shared":{"type":"streamable_http","url":"http://127.0.0.1:10/mcp"}}}"#,
        )
        .unwrap();

        let servers = discover_runtime_mcp_servers(&root, Some(&workspace), None).unwrap();

        assert_eq!(servers["user_db"].source, McpServerSource::User);
        assert_eq!(servers["workspace_db"].source, McpServerSource::Workspace);
        assert_eq!(servers["shared"].source, McpServerSource::User);
        assert_eq!(servers.len(), 3);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn user_server_overrides_workspace_normalized_name() {
        let root = test_root("normalized-id");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"shared-tool":{"type":"stdio","command":"user-mcp"}}}"#,
        )
        .unwrap();
        fs::write(
            workspace.join(".mcp.json"),
            r#"{"mcpServers":{"shared_tool":{"type":"stdio","command":"workspace-mcp"}}}"#,
        )
        .unwrap();

        let servers = discover_runtime_mcp_servers(&root, Some(&workspace), None).unwrap();

        assert_eq!(servers.len(), 1);
        assert_eq!(servers["shared_tool"].source, McpServerSource::User);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_switch_replaces_workspace_servers_and_keeps_user_winners() {
        let root = test_root("workspace-switch");
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(
            root.join(".mcp.json"),
            r#"{"mcpServers":{"shared":{"type":"stdio","command":"user-shared"}}}"#,
        )
        .unwrap();
        fs::write(
            first.join(".mcp.json"),
            r#"{"mcpServers":{"shared":{"type":"stdio","command":"first-shared"},"first-only":{"type":"stdio","command":"first-mcp"}}}"#,
        )
        .unwrap();
        fs::write(
            second.join(".mcp.json"),
            r#"{"mcpServers":{"shared":{"type":"stdio","command":"second-shared"},"second-only":{"type":"stdio","command":"second-mcp"}}}"#,
        )
        .unwrap();

        let first_servers = discover_runtime_mcp_servers(&root, Some(&first), None).unwrap();
        let second_servers = discover_runtime_mcp_servers(&root, Some(&second), None).unwrap();

        assert!(first_servers.contains_key("first_only"));
        assert!(!second_servers.contains_key("first_only"));
        assert!(second_servers.contains_key("second_only"));
        assert_eq!(second_servers["shared"].source, McpServerSource::User);
        assert_eq!(second_servers.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    fn test_root(name: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        axon_test_support::test_temp_root().join(format!("axon-mcp-discovery-{name}-{nonce}"))
    }
}
