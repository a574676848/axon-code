use std::time::Duration;

use axon_agent_runtime::McpServerConfig;

const DEFAULT_MCP_TOOL_CALL_TIMEOUT_SECS: u64 = 300;

/// MCP 工具调用失败分类。
///
/// 分类决定 CLI 回执语义，也决定是否写负缓存：只有发现期失败可以进 server 级熔断，
/// 调用期失败最多只冷却当次工具。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMcpToolCallFailureKind {
    /// 调用参数不符合已发现 schema。
    InvalidInput,
    /// 本地配置或运行环境无效（例如 `toolTimeoutMs` 非法、用户级 Axon 目录不可解析）。
    ///
    /// 失败语义是 fast-fail：既不写 server 级熔断，也不冷却工具，重试前必须先修配置。
    InvalidConfig,
    /// 该 (server, tool) 处于调用失败后的短期冷却，本次未发起真实调用。
    ToolCooling,
    /// 发现期熔断仍在生效，本次调用被快速拒绝。
    ServerCircuitOpen,
    /// 连接、超时或 transport 异常；只冷却当次工具，不改变任何工具注册表。
    Unavailable,
    /// MCP server 已明确拒绝本次业务调用。
    ToolRejected,
}

impl RuntimeMcpToolCallFailureKind {
    /// 稳定失败名，用于日志与诊断，不参与对外契约。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid_input",
            Self::InvalidConfig => "invalid_config",
            Self::ToolCooling => "tool_cooling",
            Self::ServerCircuitOpen => "server_circuit_open",
            Self::Unavailable => "unavailable",
            Self::ToolRejected => "tool_rejected",
        }
    }
}

/// MCP 工具调用失败，保留可供 CLI 决策的分类但不暴露原始 payload。
#[derive(Debug, Clone)]
pub struct RuntimeMcpToolCallError {
    kind: RuntimeMcpToolCallFailureKind,
    message: String,
}

impl RuntimeMcpToolCallError {
    pub(super) fn invalid_input(message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeMcpToolCallFailureKind::InvalidInput,
            message: message.into(),
        }
    }

    pub(super) fn invalid_config(message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeMcpToolCallFailureKind::InvalidConfig,
            message: message.into(),
        }
    }

    pub(super) fn tool_cooling(message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeMcpToolCallFailureKind::ToolCooling,
            message: message.into(),
        }
    }

    pub(super) fn server_circuit_open(message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeMcpToolCallFailureKind::ServerCircuitOpen,
            message: message.into(),
        }
    }

    pub(super) fn tool_rejected(message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeMcpToolCallFailureKind::ToolRejected,
            message: message.into(),
        }
    }

    pub(super) fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: RuntimeMcpToolCallFailureKind::Unavailable,
            message: message.into(),
        }
    }

    /// 失败分类。
    #[must_use]
    pub const fn kind(&self) -> RuntimeMcpToolCallFailureKind {
        self.kind
    }

    /// 已脱敏的错误说明。
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

pub(super) fn normalized_description(value: &str) -> String {
    let normalized = value.trim();
    if normalized.is_empty() {
        "MCP tool".to_owned()
    } else {
        normalized.to_owned()
    }
}

pub(super) fn mcp_call_timeout(config: &McpServerConfig) -> Result<Option<Duration>, String> {
    let configured_timeout_ms = match config {
        McpServerConfig::Stdio(command) => command.tool_timeout_ms,
        McpServerConfig::StreamableHttp(http) => http.tool_timeout_ms,
    };
    let Some(timeout_ms) = configured_timeout_ms else {
        return Ok(Some(Duration::from_secs(
            DEFAULT_MCP_TOOL_CALL_TIMEOUT_SECS,
        )));
    };
    if timeout_ms == 0 {
        return Err("MCP toolTimeoutMs 必须大于 0".to_owned());
    }
    Ok(Some(Duration::from_millis(timeout_ms)))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axon_agent_runtime::McpServerCommand;

    use super::*;

    #[test]
    fn normalized_description_preserves_complete_non_empty_value() {
        let description = "详细工具说明".repeat(100);

        assert_eq!(normalized_description(&description), description);
        assert_eq!(normalized_description("  "), "MCP tool");
    }

    #[test]
    fn mcp_timeout_defaults_to_five_minutes_and_accepts_explicit_positive_value() {
        let config = |tool_timeout_ms| {
            McpServerConfig::Stdio(McpServerCommand {
                program: "fixture".to_owned(),
                args: Vec::new(),
                working_dir: None,
                env: BTreeMap::new(),
                tool_timeout_ms,
            })
        };

        assert_eq!(
            mcp_call_timeout(&config(None)),
            Ok(Some(Duration::from_secs(
                DEFAULT_MCP_TOOL_CALL_TIMEOUT_SECS
            )))
        );
        assert_eq!(
            mcp_call_timeout(&config(Some(750))),
            Ok(Some(Duration::from_millis(750)))
        );
        assert!(mcp_call_timeout(&config(Some(0))).is_err());
    }
}
