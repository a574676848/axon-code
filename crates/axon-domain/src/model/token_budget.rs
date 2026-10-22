//! Token 预算领域对象。

use crate::{AxonError, AxonResult};

/// token 预算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBudget {
    /// 最大 token 数。
    pub max_tokens: u64,
}

impl TokenBudget {
    /// 创建 token 预算。
    ///
    /// # Errors
    ///
    /// 当预算为 0 时返回错误。
    pub fn new(max_tokens: u64) -> AxonResult<Self> {
        if max_tokens == 0 {
            return Err(AxonError::invalid_input("token budget 必须大于 0"));
        }
        Ok(Self { max_tokens })
    }
}
