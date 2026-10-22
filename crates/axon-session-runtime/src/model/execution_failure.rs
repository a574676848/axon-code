//! Turn 执行失败的结构化分类。

use std::fmt;

/// Turn 执行失败类别。
///
/// 类别由执行边界判定，禁止通过错误文本匹配推断：
/// - `Retryable`：可证明不会重复外部副作用的瞬态失败，允许退避重试；
/// - `Permanent`：确定性失败，立即形成失败结论；
/// - `ExternalOutcomeUnknown`：外部副作用可能已生效但结果未知，必须进入 `uncertain`，禁止自动重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnFailureKind {
    /// 可退避重试的内部失败。
    Retryable,
    /// 不可重试的确定性失败。
    Permanent,
    /// 外部副作用结果未知。
    ExternalOutcomeUnknown,
}

impl TurnFailureKind {
    /// 判断失败是否已形成最终结论。
    #[must_use]
    pub const fn is_conclusive(self) -> bool {
        matches!(self, Self::Permanent | Self::ExternalOutcomeUnknown)
    }

    /// 稳定分类标识，用于持久 payload。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::Permanent => "permanent",
            Self::ExternalOutcomeUnknown => "external_outcome_unknown",
        }
    }
}

/// Turn 执行失败的完整事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnExecutionError {
    kind: TurnFailureKind,
    message: String,
}

impl TurnExecutionError {
    /// 构造可重试失败。
    #[must_use]
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            kind: TurnFailureKind::Retryable,
            message: message.into(),
        }
    }

    /// 构造确定性失败。
    #[must_use]
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            kind: TurnFailureKind::Permanent,
            message: message.into(),
        }
    }

    /// 构造外部结果未知失败。
    #[must_use]
    pub fn external_outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            kind: TurnFailureKind::ExternalOutcomeUnknown,
            message: message.into(),
        }
    }

    /// 失败类别。
    #[must_use]
    pub const fn kind(&self) -> TurnFailureKind {
        self.kind
    }

    /// 中文错误事实。
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 取出错误事实。
    #[must_use]
    pub fn into_message(self) -> String {
        self.message
    }
}

impl fmt::Display for TurnExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TurnExecutionError {}
