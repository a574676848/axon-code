use std::path::Path;

use axon_agent_runtime::{AgentToolRegistryRuntime, ProviderToolCall, ToolRegistry};
use axon_mcp_runtime::RuntimeMcpServerLoadReport;

pub(crate) struct RuntimeExecutionToolSnapshot {
    pub(crate) registry: ToolRegistry,
    pub(crate) mcp_schema_cache: Vec<RuntimeMcpServerLoadReport>,
}

pub(crate) fn build_runtime_tool_registry_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
) -> RuntimeExecutionToolSnapshot {
    build_runtime_tool_registry_snapshot_with_refresh(config_path, cwd, false)
}

/// 构建执行工具快照；`refresh_mcp_health=true` 时绕过 MCP 健康负缓存与 schema 缓存现场重探。
pub(crate) fn build_runtime_tool_registry_snapshot_with_refresh(
    config_path: &Path,
    cwd: Option<&Path>,
    refresh_mcp_health: bool,
) -> RuntimeExecutionToolSnapshot {
    try_build_runtime_tool_registry_snapshot(config_path, cwd, refresh_mcp_health).unwrap_or_else(
        |_| {
            let mut registry = ToolRegistry::new();
            let _ = AgentToolRegistryRuntime::register_builtin_tools(&mut registry);
            RuntimeExecutionToolSnapshot {
                registry,
                mcp_schema_cache: Vec::new(),
            }
        },
    )
}

pub(crate) fn try_build_runtime_tool_registry_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
    refresh_mcp_health: bool,
) -> Result<RuntimeExecutionToolSnapshot, String> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)?;
    #[cfg(test)]
    let build = axon_mcp_runtime::build_runtime_tool_registry_snapshot_with_home_for_tests(
        config_path,
        workspace.as_deref(),
        config_path.parent(),
    )?;
    #[cfg(not(test))]
    let build = axon_mcp_runtime::build_runtime_tool_registry_snapshot_with_refresh(
        config_path,
        workspace.as_deref(),
        refresh_mcp_health,
    )?;
    #[cfg(test)]
    let _ = refresh_mcp_health;
    Ok(RuntimeExecutionToolSnapshot {
        registry: build.registry,
        mcp_schema_cache: build.mcp_schema_cache,
    })
}

pub(crate) fn build_execution_tool_registry_for_session(
    config_path: Option<&Path>,
    cwd: Option<&Path>,
    _session_id: &str,
) -> ToolRegistry {
    runtime_tool_registry_or_builtin_with_cwd(config_path, cwd)
}

#[cfg(test)]
pub(crate) fn execute_runtime_mcp_tool_call(
    config_path: &Path,
    cwd: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
) -> Result<axon_mcp_runtime::RuntimeMcpToolCallOutput, axon_mcp_runtime::RuntimeMcpToolCallError> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)
        .ok()
        .flatten();
    let isolated_home = config_path.parent();
    axon_mcp_runtime::execute_runtime_mcp_tool_call_with_home_for_tests(
        config_path,
        workspace.as_deref(),
        isolated_home,
        execution_registry,
        tool_call,
    )
}

#[cfg(not(test))]
pub(crate) fn execute_runtime_mcp_tool_call_with_cancel(
    config_path: &Path,
    cwd: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
    cancel_token: Option<&axon_agent_runtime::McpCancelToken>,
) -> Result<axon_mcp_runtime::RuntimeMcpToolCallOutput, axon_mcp_runtime::RuntimeMcpToolCallError> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)
        .ok()
        .flatten();
    axon_mcp_runtime::execute_runtime_mcp_tool_call_with_cancel(
        config_path,
        workspace.as_deref(),
        execution_registry,
        tool_call,
        cancel_token,
    )
}

#[cfg(test)]
pub(crate) fn execute_runtime_mcp_tool_call_with_cancel(
    config_path: &Path,
    cwd: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
    _cancel_token: Option<&axon_agent_runtime::McpCancelToken>,
) -> Result<axon_mcp_runtime::RuntimeMcpToolCallOutput, axon_mcp_runtime::RuntimeMcpToolCallError> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)
        .ok()
        .flatten();
    let isolated_home = config_path.parent();
    axon_mcp_runtime::execute_runtime_mcp_tool_call_with_home_for_tests(
        config_path,
        workspace.as_deref(),
        isolated_home,
        execution_registry,
        tool_call,
    )
}

pub(crate) fn execute_runtime_mcp_list_resources(
    config_path: &Path,
    cwd: Option<&Path>,
    server: Option<&str>,
    cursor: Option<&str>,
) -> Result<axon_mcp_runtime::RuntimeMcpResourceToolOutput, String> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)?;
    #[cfg(test)]
    let result = axon_mcp_runtime::execute_runtime_mcp_list_resources_with_home_for_tests(
        config_path,
        workspace.as_deref(),
        config_path.parent(),
        server,
        cursor,
    );
    #[cfg(not(test))]
    let result = axon_mcp_runtime::execute_runtime_mcp_list_resources(
        config_path,
        workspace.as_deref(),
        server,
        cursor,
    );
    result
}

pub(crate) fn execute_runtime_mcp_read_resource(
    config_path: &Path,
    cwd: Option<&Path>,
    server: &str,
    uri: &str,
) -> Result<axon_mcp_runtime::RuntimeMcpResourceToolOutput, String> {
    let workspace = super::resolve_workspace_cwd(config_path, cwd)?;
    #[cfg(test)]
    let result = axon_mcp_runtime::execute_runtime_mcp_read_resource_with_home_for_tests(
        config_path,
        workspace.as_deref(),
        config_path.parent(),
        server,
        uri,
    );
    #[cfg(not(test))]
    let result = axon_mcp_runtime::execute_runtime_mcp_read_resource(
        config_path,
        workspace.as_deref(),
        server,
        uri,
    );
    result
}

pub(crate) fn runtime_tool_registry_or_builtin(config_path: Option<&Path>) -> ToolRegistry {
    #[cfg(test)]
    let result = axon_mcp_runtime::runtime_tool_registry_or_builtin_with_home_for_tests(
        config_path,
        None,
        config_path.and_then(Path::parent),
        false,
    );
    #[cfg(not(test))]
    let result = axon_mcp_runtime::runtime_tool_registry_or_builtin(config_path);
    result
}

pub(crate) fn runtime_tool_registry_or_builtin_with_cwd(
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> ToolRegistry {
    let workspace =
        config_path.and_then(|path| super::resolve_workspace_cwd(path, cwd).ok().flatten());
    #[cfg(test)]
    if let Some(config_path) = config_path {
        return axon_mcp_runtime::runtime_tool_registry_or_builtin_with_home_for_tests(
            Some(config_path),
            workspace.as_deref(),
            config_path.parent(),
            false,
        );
    }
    axon_mcp_runtime::runtime_tool_registry_or_builtin_with_cwd(config_path, workspace.as_deref())
}
