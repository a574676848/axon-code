//! MCP client capabilities 目标态投影。
//!
//! 该模块只属于 runtime 外层协议适配，不进入 Domain，避免领域层感知 MCP 协议字段。

use serde_json::{Value, json};

/// MCP 客户端能力名称：elicitation。
pub const MCP_CLIENT_CAPABILITY_ELICITATION: &str = "elicitation";
/// MCP 客户端能力名称：roots。
pub const MCP_CLIENT_CAPABILITY_ROOTS: &str = "roots";
/// MCP 客户端能力名称：sampling。
pub const MCP_CLIENT_CAPABILITY_SAMPLING: &str = "sampling";
/// MCP 客户端能力名称：logging。
pub const MCP_CLIENT_CAPABILITY_LOGGING: &str = "logging";
/// MCP 客户端能力策略状态。
pub const MCP_CLIENT_CAPABILITY_POLICY_STATUS: &str = "client_capabilities_policy_ready";

/// MCP client capabilities 安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientCapabilitiesProjection {
    /// 策略状态。
    pub status: &'static str,
    /// initialize 中真实声明的能力。
    pub advertised: Vec<&'static str>,
    /// 当前显式拒绝的能力。
    pub denied: Vec<&'static str>,
    /// 是否允许 server 向 client 发起 elicitation。
    pub elicitation: bool,
    /// 是否允许 server 查询 roots。
    pub roots: bool,
    /// 是否允许 server 触发 sampling。
    pub sampling: bool,
    /// 是否允许 server 推送 logging。
    pub logging: bool,
}

/// MCP client capabilities runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpClientCapabilitiesRuntime;

impl McpClientCapabilitiesRuntime {
    /// 返回 Axon 目标态 MCP client capabilities 投影。
    ///
    /// initialize capabilities 保持空对象；运行态投影记录 elicitation、roots、sampling 与 logging 的本地策略状态。
    #[must_use]
    pub fn projection() -> McpClientCapabilitiesProjection {
        McpClientCapabilitiesProjection {
            status: MCP_CLIENT_CAPABILITY_POLICY_STATUS,
            advertised: Vec::new(),
            denied: vec![
                MCP_CLIENT_CAPABILITY_ELICITATION,
                MCP_CLIENT_CAPABILITY_ROOTS,
                MCP_CLIENT_CAPABILITY_SAMPLING,
                MCP_CLIENT_CAPABILITY_LOGGING,
            ],
            elicitation: false,
            roots: false,
            sampling: false,
            logging: false,
        }
    }

    /// 返回 initialize.params.capabilities。
    #[must_use]
    pub fn initialize_capabilities_value() -> Value {
        json!({})
    }
}
