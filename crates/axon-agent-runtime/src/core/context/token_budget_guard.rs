//! Token Budget 感知与窗口轮转边界策略。
//!
//! 阈值采用 Codex 的“距离自动换窗还剩多少 token”语义，且当前仅由运行时持有，
//! 不进入用户配置、请求参数或 Workspace 稳定前缀。

/// 距离自动换窗触发线的固定预警阈值。
pub(crate) const TOKEN_BUDGET_REMINDER_THRESHOLD_TOKENS: u64 = 6_144;
/// fallback 后允许完成一次短操作的固定缓冲。
pub(crate) const TOKEN_BUDGET_FALLBACK_BUFFER_TOKENS: u64 = 16_384;

/// Token Budget 内部运行时策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenBudgetRuntimePolicy {
    reminder_threshold_tokens: u64,
    fallback_buffer_tokens: u64,
}

impl Default for TokenBudgetRuntimePolicy {
    fn default() -> Self {
        Self {
            reminder_threshold_tokens: TOKEN_BUDGET_REMINDER_THRESHOLD_TOKENS,
            fallback_buffer_tokens: TOKEN_BUDGET_FALLBACK_BUFFER_TOKENS,
        }
    }
}

impl TokenBudgetRuntimePolicy {
    /// 计算距离自动换窗触发线的剩余 token。
    #[must_use]
    pub(crate) const fn remaining_before_rollover(
        trigger_tokens: u64,
        current_context_tokens: u64,
    ) -> u64 {
        trigger_tokens.saturating_sub(current_context_tokens)
    }

    /// 是否应发送常规预警；调用方负责同一窗口只发送一次。
    #[must_use]
    pub(crate) const fn should_emit_reminder(
        self,
        trigger_tokens: u64,
        current_context_tokens: u64,
    ) -> bool {
        let remaining = Self::remaining_before_rollover(trigger_tokens, current_context_tokens);
        current_context_tokens < trigger_tokens && remaining <= self.reminder_threshold_tokens
    }

    /// fallback 缓冲耗尽时的强制轮转线，不得越过模型完整上下文窗口。
    #[must_use]
    pub(crate) const fn forced_rollover_tokens(
        self,
        trigger_tokens: u64,
        context_window: u64,
    ) -> u64 {
        let buffered_tokens = trigger_tokens.saturating_add(self.fallback_buffer_tokens);
        if buffered_tokens < context_window {
            buffered_tokens
        } else {
            context_window
        }
    }

    /// 达到触发线但尚未耗尽缓冲时，是否应发送一次 fallback。
    #[must_use]
    pub(crate) const fn should_emit_fallback(
        self,
        trigger_tokens: u64,
        context_window: u64,
        current_context_tokens: u64,
        fallback_emitted: bool,
    ) -> bool {
        !fallback_emitted
            && current_context_tokens >= trigger_tokens
            && current_context_tokens < self.forced_rollover_tokens(trigger_tokens, context_window)
    }
}
