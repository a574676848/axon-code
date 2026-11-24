//! 内置工具实际运行预算。

use crate::runtime_limits::{
    DEFAULT_NETWORK_SEARCH_RETRY_LIMIT, DEFAULT_NETWORK_SEARCH_TIMEOUT_MS,
    DEFAULT_SHELL_TIMEOUT_MS, DEFAULT_TOOL_RETRY_LIMIT, MIN_SHELL_TIMEOUT_MS,
};

/// 工具预算分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolBudgetKind {
    /// Shell 命令。
    ShellCommand,
    /// 网络查询。
    NetworkSearch,
}

/// 工具运行预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRuntimeBudget {
    /// 默认超时毫秒数。
    pub default_timeout_ms: u64,
    /// 请求级 timeout 最小值。
    pub min_timeout_ms: Option<u64>,
    /// 请求级 timeout 最大值。
    pub max_timeout_ms: Option<u64>,
    /// 重试次数。
    pub retry_limit: u8,
}

impl ToolRuntimeBudget {
    /// 校验请求级 timeout 是否落在当前工具预算边界内。
    pub const fn validate_timeout_ms(self, timeout_ms: u64) -> Result<u64, ToolRuntimeBudgetError> {
        if let Some(min) = self.min_timeout_ms
            && timeout_ms < min
        {
            return Err(ToolRuntimeBudgetError::BelowMinimum { min });
        }
        if let Some(max) = self.max_timeout_ms
            && timeout_ms > max
        {
            return Err(ToolRuntimeBudgetError::AboveMaximum { max });
        }
        Ok(timeout_ms)
    }
}

/// 工具运行预算错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRuntimeBudgetError {
    /// 请求级 timeout 小于最小边界。
    BelowMinimum {
        /// 最小 timeout 毫秒数。
        min: u64,
    },
    /// 请求级 timeout 大于最大边界。
    AboveMaximum {
        /// 最大 timeout 毫秒数。
        max: u64,
    },
}

/// 返回指定工具类型的实际运行预算。
#[must_use]
pub const fn tool_runtime_budget(kind: ToolBudgetKind) -> ToolRuntimeBudget {
    match kind {
        ToolBudgetKind::ShellCommand => ToolRuntimeBudget {
            default_timeout_ms: DEFAULT_SHELL_TIMEOUT_MS,
            min_timeout_ms: Some(MIN_SHELL_TIMEOUT_MS),
            max_timeout_ms: None,
            retry_limit: DEFAULT_TOOL_RETRY_LIMIT,
        },
        ToolBudgetKind::NetworkSearch => ToolRuntimeBudget {
            default_timeout_ms: DEFAULT_NETWORK_SEARCH_TIMEOUT_MS,
            min_timeout_ms: None,
            max_timeout_ms: None,
            retry_limit: DEFAULT_NETWORK_SEARCH_RETRY_LIMIT,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_request_timeout_has_no_axon_specific_upper_bound() {
        let shell = tool_runtime_budget(ToolBudgetKind::ShellCommand);

        assert_eq!(shell.validate_timeout_ms(1_000), Ok(1_000));
        assert_eq!(
            shell.validate_timeout_ms(999),
            Err(ToolRuntimeBudgetError::BelowMinimum { min: 1_000 })
        );
        assert_eq!(shell.validate_timeout_ms(180_000), Ok(180_000));
        assert_eq!(shell.validate_timeout_ms(u64::MAX), Ok(u64::MAX));
    }
}
