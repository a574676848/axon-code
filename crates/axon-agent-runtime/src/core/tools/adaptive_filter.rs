//! read_file 的自适应过滤策略。

use axon_output_filter::lsp_symbol_filter::LspFilterLevel;

/// 大文件默认不触发同步 documentSymbol 查询。
pub(crate) const DEFAULT_LARGE_FILE_THRESHOLD_BYTES: u64 = 100 * 1024;
const FILTER_STRATEGY_ENV: &str = "AXON_FILTER_STRATEGY";

/// read_file 过滤策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SmartFilterStrategy {
    /// 沿用调用方级别，并对大文件自动降级。
    Auto,
    /// 禁用过滤。
    Disabled,
    /// 仅使用轻量过滤。
    Minimal,
    /// 请求 documentSymbol 过滤，但仍受大文件边界约束。
    Aggressive,
}

impl SmartFilterStrategy {
    /// 从环境变量读取策略；未配置时保持 Phase 1 的禁用行为。
    pub(crate) fn from_env() -> Self {
        match std::env::var(FILTER_STRATEGY_ENV)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "auto" => Self::Auto,
            "minimal" => Self::Minimal,
            "aggressive" => Self::Aggressive,
            _ => Self::Disabled,
        }
    }

    pub(crate) fn decide(
        self,
        requested: Option<LspFilterLevel>,
        file_size: u64,
    ) -> LspFilterLevel {
        let requested = requested.unwrap_or(match self {
            Self::Disabled => LspFilterLevel::None,
            Self::Minimal => LspFilterLevel::Minimal,
            Self::Aggressive => LspFilterLevel::Aggressive,
            Self::Auto => LspFilterLevel::None,
        });
        if !matches!(requested, LspFilterLevel::Aggressive)
            || file_size <= DEFAULT_LARGE_FILE_THRESHOLD_BYTES
        {
            return requested;
        }
        LspFilterLevel::Minimal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_filter_strategy_decides_filter_level() {
        assert_eq!(
            SmartFilterStrategy::Aggressive.decide(None, DEFAULT_LARGE_FILE_THRESHOLD_BYTES + 1,),
            LspFilterLevel::Minimal
        );
        assert_eq!(
            SmartFilterStrategy::Aggressive.decide(None, DEFAULT_LARGE_FILE_THRESHOLD_BYTES),
            LspFilterLevel::Aggressive
        );
        assert_eq!(
            SmartFilterStrategy::Aggressive.decide(
                Some(LspFilterLevel::None),
                DEFAULT_LARGE_FILE_THRESHOLD_BYTES + 1,
            ),
            LspFilterLevel::None
        );
    }
}
