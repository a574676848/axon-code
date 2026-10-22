//! Work Ledger 标识。

use serde::{Deserialize, Serialize};
use std::fmt;

pub(crate) const MAX_LEDGER_ID_LENGTH: usize = 128;

macro_rules! ledger_id {
    ($name:ident, $label:literal) => {
        #[doc = $label]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// 创建非空且长度受限的标识。
            pub fn new(value: impl Into<String>) -> Result<Self, LedgerIdError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(LedgerIdError::Empty($label));
                }
                if value.chars().count() > MAX_LEDGER_ID_LENGTH {
                    return Err(LedgerIdError::TooLong($label));
                }
                Ok(Self(value))
            }

            /// 返回稳定文本。
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

ledger_id!(GoalId, "Work Ledger Goal 标识。");
ledger_id!(ExecutionId, "Work Ledger Execution 标识。");
ledger_id!(ResultId, "Work Ledger Result 标识。");
ledger_id!(DecisionId, "Work Ledger Decision 标识。");
ledger_id!(JarvisEventId, "JARVIS Event 标识。");

/// Work Ledger 标识错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerIdError {
    /// 标识为空。
    Empty(&'static str),
    /// 标识超过长度限制。
    TooLong(&'static str),
}

impl fmt::Display for LedgerIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(label) => write!(formatter, "{label} 不能为空"),
            Self::TooLong(label) => write!(formatter, "{label} 超过长度上限"),
        }
    }
}

impl std::error::Error for LedgerIdError {}
