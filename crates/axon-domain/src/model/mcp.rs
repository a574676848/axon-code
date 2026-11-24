//! MCP server 领域合同。

use crate::{AxonError, AxonResult};

/// MCP 配置文件名。
pub const MCP_CONFIG_FILE: &str = ".mcp.json";

/// 运行时 MCP server 定义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerDefinition {
    /// MCP server 稳定 ID。
    pub server_id: String,
}

impl McpServerDefinition {
    /// 创建并校验 MCP server 定义。
    pub fn new(server_id: impl Into<String>) -> AxonResult<Self> {
        let server_id = server_id.into();
        if server_id.trim().is_empty() {
            return Err(AxonError::InvalidMcpDefinition("MCP server ID 不能为空"));
        }
        Ok(Self { server_id })
    }
}
