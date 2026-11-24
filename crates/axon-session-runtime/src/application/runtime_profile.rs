//! 主 turn 与后台 Execution 共用的 runtime profile。

/// Agent runtime 的身份 profile。
///
/// profile 由应用运行时选择，不从 Interaction payload 或 transport context 反序列化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProfile {
    /// 用户可见的统一主代理。
    Primary,
}

impl RuntimeProfile {
    /// 返回稳定的 profile 标识。
    ///
    /// 取值 `Axon` 是既有的产品身份字面量，会被写进 provider 请求与运行态事实；
    /// 它与存储层的旧表前缀无关，改名需要单独的数据迁移，因此保持原样。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "Axon",
        }
    }
}

/// runtime profile 选择器。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeProfileSelector;

impl RuntimeProfileSelector {
    /// 主 turn 固定使用主代理 profile。
    #[must_use]
    pub const fn for_primary_turn() -> RuntimeProfile {
        RuntimeProfile::Primary
    }

    /// 后台 Execution 固定使用主代理 profile。
    #[must_use]
    pub const fn for_background_execution() -> RuntimeProfile {
        RuntimeProfile::Primary
    }
}
