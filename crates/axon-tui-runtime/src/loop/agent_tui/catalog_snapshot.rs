//! 目录快照：一次后台复读带回来的东西，以及跟着它一起算的配置投影。
//!
//! 这一块与启动参数分开成模块，是因为它同时被启动与运行期复读两条路径使用：
//! skills、MCP 与会话清单来自后台线程，模型清单与两个默认值来自 `config.toml`。
//! 配置没读到时那一块留空，头部保留上一次的模型与强度，而不是把选择器清成空的。

use super::launch::{
    AgentTuiCatalogLoadState, AgentTuiMcpServerCatalogItem, AgentTuiSessionCatalogItem,
};

/// 配置投影：一次 catalog 复读里从配置文件重新算出来的部分。
///
/// 模型清单、启用的供应商与两个默认值全部来自 `config.toml`。它单独成一块，
/// 是因为快照的其余字段只描述 skills 与 MCP；配置没读到时这一块留空，
/// 头部就保留上一次的模型与强度，而不是把选择器清成空的。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentTuiConfigProjection {
    /// 可切换的模型清单（所属供应商已启用、且不是向量模型）。
    pub models: Vec<AgentTuiModelCatalogItem>,
    /// 已启用的 provider 名称。
    pub providers: Vec<String>,
    /// 配置里的主模型展示名；一个可用模型都没有时为空。
    pub default_model: Option<String>,
    /// 配置里的默认思考强度；空值表示配置文件没给结论。
    pub default_reasoning_effort: Option<String>,
}

/// Agent TUI 后台 catalog 加载结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentTuiCatalogLoadSnapshot {
    /// skill 名称。
    pub skills: Vec<String>,
    /// skill 加载状态。
    pub skill_state: AgentTuiCatalogLoadState,
    /// skill 加载错误摘要。
    pub skill_errors: Vec<String>,
    /// MCP server 快照。
    pub mcp_servers: Vec<AgentTuiMcpServerCatalogItem>,
    /// MCP 加载状态。
    pub mcp_state: AgentTuiCatalogLoadState,
    /// 最近 session 元数据。
    pub sessions: Vec<AgentTuiSessionCatalogItem>,
    /// 配置文件投影；`None` 表示这次快照不含配置事实，头部保留现状。
    pub config: Option<AgentTuiConfigProjection>,
}

/// Agent TUI 可切换模型项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiModelCatalogItem {
    /// 展示给用户的模型名称。
    pub label: String,
    /// 运行时模型名称，作为渠道负载均衡路由键。
    pub selector: String,
    /// provider 名称。
    pub provider: String,
    /// 模型 context window，上游配置未知时为空。
    pub context_window: Option<u64>,
    /// 模型默认 reasoning effort，上游配置未知或未启用时为空。
    pub reasoning_effort: Option<String>,
    /// 模型是否声明支持视觉输入；提交含图片附件的草稿前必须据此预检。
    pub supports_vision: bool,
}

impl AgentTuiModelCatalogItem {
    /// 创建模型目录项。
    #[must_use]
    pub fn new(
        label: impl Into<String>,
        selector: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            selector: selector.into(),
            provider: provider.into(),
            context_window: None,
            reasoning_effort: None,
            supports_vision: true,
        }
    }

    /// 创建带 context window 的模型目录项。
    #[must_use]
    pub fn new_with_context_window(
        label: impl Into<String>,
        selector: impl Into<String>,
        provider: impl Into<String>,
        context_window: Option<u64>,
    ) -> Self {
        Self::new_with_runtime_defaults(label, selector, provider, context_window, None)
    }

    /// 创建带运行默认值的模型目录项。
    #[must_use]
    pub fn new_with_runtime_defaults(
        label: impl Into<String>,
        selector: impl Into<String>,
        provider: impl Into<String>,
        context_window: Option<u64>,
        reasoning_effort: Option<String>,
    ) -> Self {
        Self {
            label: label.into(),
            selector: selector.into(),
            provider: provider.into(),
            context_window,
            reasoning_effort,
            supports_vision: true,
        }
    }

    /// 声明该模型的视觉输入能力，供提交前预检使用。
    #[must_use]
    pub fn with_supports_vision(mut self, supports_vision: bool) -> Self {
        self.supports_vision = supports_vision;
        self
    }

    /// 判断用户输入是否命中当前模型项。
    #[must_use]
    pub fn matches(&self, value: &str) -> bool {
        self.label == value
            || self.selector == value
            || format!("{}/{}", self.provider, self.label) == value
            || format!("{}/{}", self.provider, self.selector) == value
    }
}
