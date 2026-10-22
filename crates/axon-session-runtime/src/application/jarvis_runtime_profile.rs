//! 主 turn 与后台 Execution 共用的 JARVIS runtime profile。

/// Agent runtime 的身份 profile。
///
/// profile 由应用运行时选择，不从 Interaction payload 或 transport context 反序列化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JarvisRuntimeProfile {
    /// 用户可见的统一 JARVIS 主代理。
    Jarvis,
}

impl JarvisRuntimeProfile {
    /// 返回稳定的 profile 标识。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jarvis => "jarvis",
        }
    }
}

/// JARVIS runtime profile 选择器。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JarvisRuntimeSelector;

impl JarvisRuntimeSelector {
    /// 主 turn 固定使用 JARVIS profile。
    #[must_use]
    pub const fn for_primary_turn() -> JarvisRuntimeProfile {
        JarvisRuntimeProfile::Jarvis
    }

    /// 后台 Execution 固定使用 JARVIS profile。
    #[must_use]
    pub const fn for_background_execution() -> JarvisRuntimeProfile {
        JarvisRuntimeProfile::Jarvis
    }
}
