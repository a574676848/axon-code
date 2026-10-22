//! Agent pane 领域对象。

/// Pane badge 严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSecurityBadgeLevel {
    /// 正常。
    Normal,
    /// 降级。
    Degraded,
    /// 失败。
    Failed,
}

/// Pane 安全 badge。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSecurityBadge {
    /// badge 级别。
    pub level: PaneSecurityBadgeLevel,
    /// 展示标签。
    pub label: String,
    /// 结构化原因。
    pub reasons: Vec<String>,
}
