//! MCP 运行态转发。
//!
//! 桌面端不自己连 MCP：服务器由 daemon 的启动快照装载，桌面只把同一份结论取回来。
//! 因此这里全是纯转发，网关路由见 `axon-cli` 的 `mcp_http_adapter`。
//!
//! `cwd` 由界面给出（当前工作区）：同一台机器上的多个工程各自读自己的 `.mcp.json`，
//! 不会把 A 工程的服务器算到 B 工程头上。

use serde_json::json;

/// MCP 运行态路由；与 daemon 侧的 `MCP_PATH` 同源。
const MCP_STATUS_PATH: &str = "/mcp/status";
const MCP_REFRESH_PATH: &str = "/mcp/refresh";

/// 读取当前工作区已加载的 MCP 服务器。
pub fn load_status(cwd: &str) -> Result<serde_json::Value, String> {
    crate::gateway::post_json(MCP_STATUS_PATH, &json!({ "cwd": cwd }))
}

/// 清健康负缓存并重新探测 MCP，返回重建之后的结论。
pub fn reload(cwd: &str) -> Result<serde_json::Value, String> {
    crate::gateway::post_json(MCP_REFRESH_PATH, &json!({ "cwd": cwd }))
}

/// Tauri IPC 命令：读取 MCP 运行态。
#[tauri::command(async)]
pub fn mcp_runtime_status(cwd: String) -> Result<serde_json::Value, String> {
    load_status(&cwd)
}

/// Tauri IPC 命令：重新加载 MCP 列表。
#[tauri::command(async)]
pub fn mcp_runtime_refresh(cwd: String) -> Result<serde_json::Value, String> {
    reload(&cwd)
}
