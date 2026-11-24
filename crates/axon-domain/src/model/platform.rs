//! 平台领域枚举。

/// Axon 明确支持的平台类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    /// Windows 平台。
    Windows,
    /// macOS 平台。
    MacOs,
    /// Linux 平台。
    Linux,
    /// 当前构建目标不在 Axon 明确支持的平台范围内。
    Unsupported,
}
