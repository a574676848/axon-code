//! Provider token usage 统一归一化模型。

use serde::{Deserialize, Serialize};

/// provider token usage 原始语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderTokenUsageSemantics {
    /// OpenAI-compatible chat completions usage。
    OpenAiCompatibleChat,
    /// OpenAI Responses usage。
    OpenAiResponses,
    /// Anthropic Messages usage。
    AnthropicMessages,
}

/// provider token usage 原始观测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProviderTokenUsage {
    /// 非缓存输入 token。
    pub input_tokens: u64,
    /// provider 输出 token 原始计数。
    pub output_tokens: u64,
    /// provider total token 原始计数。
    pub total_tokens: u64,
    /// provider cache read token。
    pub cache_read_input_tokens: u64,
    /// provider cache write token。
    pub cache_creation_input_tokens: u64,
    /// provider reasoning output token。
    pub reasoning_output_tokens: u64,
}

/// 归一化后的 provider usage。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedProviderTokenUsage {
    /// 原始 provider token 语义。
    pub semantics: ProviderTokenUsageSemantics,
    /// 归一化 telemetry。
    pub telemetry: TokenUsageTelemetry,
    /// provider 原始 total_tokens。
    pub provider_total_tokens: u64,
    /// 归一化总 token 是否与 provider total 一致。
    pub total_matches_provider: bool,
}

/// 模型 token usage 观测。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageTelemetry {
    /// 输入 token。
    pub input_tokens: u64,
    /// cache read 输入 token。
    pub cache_read_input_tokens: u64,
    /// cache write 输入 token。
    pub cache_write_input_tokens: u64,
    /// 输出 token。
    pub output_tokens: u64,
    /// reasoning token。
    pub reasoning_output_tokens: u64,
    /// provider 观测到的当前上下文 token。
    #[serde(default)]
    pub current_context_tokens: u64,
    /// 完整输入 token，包含非缓存、缓存读取和缓存写入 token。
    #[serde(default)]
    pub total_input_tokens: u64,
    /// 上下文构成估算：注入的系统提示词与上下文 fragment。
    #[serde(default)]
    pub system_prompt_tokens: u64,
    /// 上下文构成估算：模型可见的工具定义。
    #[serde(default)]
    pub tool_definition_tokens: u64,
    /// 上下文构成估算：真实对话消息（含工具回执）。
    #[serde(default)]
    pub conversation_tokens: u64,
}

impl ProviderTokenUsage {
    /// 按 provider 语义归一化为 runtime 可消费的 token usage。
    #[must_use]
    pub fn normalize(self, semantics: ProviderTokenUsageSemantics) -> NormalizedProviderTokenUsage {
        match semantics {
            ProviderTokenUsageSemantics::OpenAiCompatibleChat
            | ProviderTokenUsageSemantics::OpenAiResponses => {
                self.normalize_cache_inclusive(semantics)
            }
            ProviderTokenUsageSemantics::AnthropicMessages => {
                self.normalize_cache_disjoint(semantics)
            }
        }
    }

    fn normalize_cache_inclusive(
        self,
        semantics: ProviderTokenUsageSemantics,
    ) -> NormalizedProviderTokenUsage {
        let cache_read_input_tokens = self.cache_read_input_tokens.min(self.input_tokens);
        let cache_write_input_tokens = self
            .cache_creation_input_tokens
            .min(self.input_tokens.saturating_sub(cache_read_input_tokens));
        let input_tokens = self
            .input_tokens
            .saturating_sub(cache_read_input_tokens)
            .saturating_sub(cache_write_input_tokens);
        let output_tokens = self
            .output_tokens
            .saturating_sub(self.reasoning_output_tokens);
        self.to_normalized(
            semantics,
            TokenUsageTelemetry::new(
                input_tokens,
                cache_read_input_tokens,
                cache_write_input_tokens,
                output_tokens,
                self.reasoning_output_tokens,
            )
            .with_total_input_tokens(self.input_tokens),
        )
    }

    fn normalize_cache_disjoint(
        self,
        semantics: ProviderTokenUsageSemantics,
    ) -> NormalizedProviderTokenUsage {
        self.to_normalized(
            semantics,
            TokenUsageTelemetry::new(
                self.input_tokens,
                self.cache_read_input_tokens,
                self.cache_creation_input_tokens,
                self.output_tokens,
                self.reasoning_output_tokens,
            ),
        )
    }

    fn to_normalized(
        self,
        semantics: ProviderTokenUsageSemantics,
        telemetry: TokenUsageTelemetry,
    ) -> NormalizedProviderTokenUsage {
        let current_context_tokens = if self.total_tokens > 0 {
            self.total_tokens
        } else {
            telemetry.total_tokens()
        };
        NormalizedProviderTokenUsage {
            semantics,
            telemetry: telemetry.with_current_context_tokens(current_context_tokens),
            provider_total_tokens: self.total_tokens,
            total_matches_provider: telemetry.total_tokens() == self.total_tokens,
        }
    }
}

impl TokenUsageTelemetry {
    /// 创建 token usage 观测。
    #[must_use]
    pub const fn new(
        input_tokens: u64,
        cache_read_input_tokens: u64,
        cache_write_input_tokens: u64,
        output_tokens: u64,
        reasoning_output_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            cache_read_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            current_context_tokens: 0,
            total_input_tokens: input_tokens
                .saturating_add(cache_read_input_tokens)
                .saturating_add(cache_write_input_tokens),
            system_prompt_tokens: 0,
            tool_definition_tokens: 0,
            conversation_tokens: 0,
        }
    }

    /// 设置 provider 观测到的当前上下文 token。
    #[must_use]
    pub const fn with_current_context_tokens(mut self, current_context_tokens: u64) -> Self {
        self.current_context_tokens = current_context_tokens;
        self
    }

    /// 设置 provider 归一化后的完整输入 token。
    #[must_use]
    pub const fn with_total_input_tokens(mut self, total_input_tokens: u64) -> Self {
        self.total_input_tokens = total_input_tokens;
        self
    }

    /// 记录本次请求的上下文构成估算。
    ///
    /// 构成是**当前快照**而不是累加量：每次请求都整组覆盖，读数永远对应最近一次
    /// 真正发出去的请求。因此 `add_assign` 刻意不碰这三个字段。
    pub fn set_context_composition(&mut self, composition: crate::ContextTokenComposition) {
        self.system_prompt_tokens = composition.system_prompt_tokens;
        self.tool_definition_tokens = composition.tool_definition_tokens;
        self.conversation_tokens = composition.conversation_tokens;
    }

    /// 是否包含上下文构成观测。
    #[must_use]
    pub const fn has_context_composition(self) -> bool {
        self.system_prompt_tokens > 0
            || self.tool_definition_tokens > 0
            || self.conversation_tokens > 0
    }

    /// 完整输入 token，包含所有可计费输入部分。
    #[must_use]
    pub const fn complete_input_tokens(self) -> u64 {
        self.total_input_tokens
    }

    /// 缓存读取占完整输入的命中率，单位为 basis points。
    #[must_use]
    pub fn cache_hit_rate_basis_points(self) -> u16 {
        axon_domain::ratio_basis_points(self.cache_read_input_tokens, self.complete_input_tokens())
    }

    /// 当前上下文 token。provider 未返回原始 total 时回退到归一化总量。
    #[must_use]
    pub const fn current_context_tokens(self) -> u64 {
        if self.current_context_tokens > 0 {
            self.current_context_tokens
        } else {
            self.total_tokens()
        }
    }

    /// 计算总 token。
    #[must_use]
    pub const fn total_tokens(self) -> u64 {
        self.complete_input_tokens()
            .saturating_add(self.output_tokens)
            .saturating_add(self.reasoning_output_tokens)
    }

    /// 是否包含有效 token 观测。
    #[must_use]
    pub const fn has_tokens(self) -> bool {
        self.total_tokens() > 0
    }

    /// 累加另一份 token usage 观测。
    pub fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
        self.total_input_tokens = self
            .total_input_tokens
            .saturating_add(other.complete_input_tokens());
        if other.current_context_tokens > 0 {
            self.current_context_tokens = other.current_context_tokens;
        }
        // 上下文构成是快照不是累加量，且只由请求侧设置，因此这里不参与累加。
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderTokenUsage, ProviderTokenUsageSemantics};

    #[test]
    fn openai_compatible_cache_is_included_in_prompt_tokens() {
        let usage = ProviderTokenUsage {
            input_tokens: 30,
            output_tokens: 12,
            total_tokens: 42,
            cache_read_input_tokens: 4,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 2,
        }
        .normalize(ProviderTokenUsageSemantics::OpenAiCompatibleChat)
        .telemetry;

        assert_eq!(usage.input_tokens, 26);
        assert_eq!(usage.cache_read_input_tokens, 4);
        assert_eq!(usage.cache_write_input_tokens, 0);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(usage.reasoning_output_tokens, 2);
        assert_eq!(usage.complete_input_tokens(), 30);
        assert_eq!(usage.current_context_tokens(), 42);
    }

    #[test]
    fn anthropic_cache_fields_are_disjoint_from_input_tokens() {
        let usage = ProviderTokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            total_tokens: 0,
            cache_read_input_tokens: 3,
            cache_creation_input_tokens: 2,
            reasoning_output_tokens: 0,
        }
        .normalize(ProviderTokenUsageSemantics::AnthropicMessages)
        .telemetry;

        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.cache_read_input_tokens, 3);
        assert_eq!(usage.cache_write_input_tokens, 2);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.current_context_tokens, 23);
        assert_eq!(usage.complete_input_tokens(), 16);
    }

    #[test]
    fn openai_responses_cache_is_included_in_input_tokens() {
        let usage = ProviderTokenUsage {
            input_tokens: 13,
            output_tokens: 9,
            total_tokens: 22,
            cache_read_input_tokens: 4,
            cache_creation_input_tokens: 0,
            reasoning_output_tokens: 3,
        }
        .normalize(ProviderTokenUsageSemantics::OpenAiResponses)
        .telemetry;

        assert_eq!(usage.input_tokens, 9);
        assert_eq!(usage.cache_read_input_tokens, 4);
        assert_eq!(usage.cache_write_input_tokens, 0);
        assert_eq!(usage.output_tokens, 6);
        assert_eq!(usage.reasoning_output_tokens, 3);
        assert_eq!(usage.current_context_tokens(), 22);
        assert_eq!(usage.complete_input_tokens(), 13);
    }
}
