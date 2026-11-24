//! MCP runtime 执行面。
//!
//! CLI 只负责命令解析与输出，本 crate 负责 turn-time MCP tool registry、
//! Axon MCP 发现、MCP bridge 执行与健康负缓存。

#[path = "runtime/axon_mcp_config.rs"]
mod axon_mcp_config;
#[path = "runtime/discovery.rs"]
mod discovery;
#[path = "runtime/mcp.rs"]
mod mcp;
#[path = "runtime/mcp_call_support.rs"]
mod mcp_call_support;
#[path = "runtime/mcp_health_cache.rs"]
mod mcp_health_cache;
#[path = "runtime/mcp_prompt_tools.rs"]
mod mcp_prompt_tools;
#[path = "runtime/mcp_resource_tools.rs"]
mod mcp_resource_tools;
#[path = "runtime/mcp_runtime_oauth.rs"]
mod mcp_runtime_oauth;
#[path = "runtime/mcp_schema_cache.rs"]
mod mcp_schema_cache;
#[path = "runtime/mcp_tool_output.rs"]
mod mcp_tool_output;

pub use discovery::{DiscoveredRuntimeMcpServer, McpServerSource, discover_runtime_mcp_servers};
pub use mcp::{
    RuntimeMcpServerLoadReport, RuntimeToolRegistryBuild, build_runtime_tool_registry,
    build_runtime_tool_registry_snapshot, build_runtime_tool_registry_snapshot_with_home_for_tests,
    build_runtime_tool_registry_snapshot_with_refresh,
    build_runtime_tool_registry_with_home_for_tests, clear_runtime_mcp_health_cache,
    execute_runtime_mcp_tool_call, execute_runtime_mcp_tool_call_with_cancel,
    execute_runtime_mcp_tool_call_with_home_for_tests, runtime_tool_registry_or_builtin,
    runtime_tool_registry_or_builtin_with_cwd,
    runtime_tool_registry_or_builtin_with_home_for_tests,
    runtime_tool_registry_or_builtin_with_refresh,
};
pub use mcp_call_support::{RuntimeMcpToolCallError, RuntimeMcpToolCallFailureKind};
pub use mcp_prompt_tools::{
    RuntimeMcpPromptToolOutput, execute_runtime_mcp_get_prompt,
    execute_runtime_mcp_get_prompt_with_home_for_tests, execute_runtime_mcp_list_prompts,
    execute_runtime_mcp_list_prompts_with_home_for_tests,
};
pub use mcp_resource_tools::{
    RuntimeMcpResourceToolOutput, execute_runtime_mcp_list_resource_templates,
    execute_runtime_mcp_list_resource_templates_with_home_for_tests,
    execute_runtime_mcp_list_resources, execute_runtime_mcp_list_resources_with_home_for_tests,
    execute_runtime_mcp_read_resource, execute_runtime_mcp_read_resource_with_home_for_tests,
};
pub use mcp_tool_output::RuntimeMcpToolCallOutput;
