//! Provider stream token usage 解析。

use crate::token_usage::{ProviderTokenUsage, ProviderTokenUsageSemantics, TokenUsageTelemetry};

pub(super) fn parse_openai_stream_usage(value: &serde_json::Value) -> Option<TokenUsageTelemetry> {
    value
        .get("usage")
        .and_then(usage_from_openai_compatible_usage)
}

fn usage_from_openai_compatible_usage(usage: &serde_json::Value) -> Option<TokenUsageTelemetry> {
    usage_from_openai_chat_usage(usage)
}

fn usage_from_openai_chat_usage(usage: &serde_json::Value) -> Option<TokenUsageTelemetry> {
    let prompt_tokens = token_field(usage, "prompt_tokens", "promptTokens");
    let completion_tokens = token_field(usage, "completion_tokens", "completionTokens");
    let prompt_cache_read_tokens = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("promptTokensDetails"))
        .map_or(0, |details| {
            token_field(details, "cached_tokens", "cachedTokens")
        });
    let provider_cache_read_tokens =
        token_field(usage, "cache_read_input_tokens", "cacheReadInputTokens");
    let prompt_cache_write_tokens = usage
        .get("prompt_tokens_details")
        .or_else(|| usage.get("promptTokensDetails"))
        .map_or(0, |details| {
            token_field(details, "cache_write_tokens", "cacheWriteTokens")
        });
    let provider_cache_write_tokens = token_field(
        usage,
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
    );
    let cached_prompt_tokens = prompt_cache_read_tokens.max(provider_cache_read_tokens);
    let cache_write_tokens = prompt_cache_write_tokens.max(provider_cache_write_tokens);
    let reasoning_completion_tokens = usage
        .get("completion_tokens_details")
        .or_else(|| usage.get("completionTokensDetails"))
        .map_or(0, |details| {
            token_field(details, "reasoning_tokens", "reasoningTokens")
        });
    let provider_usage = ProviderTokenUsage {
        input_tokens: prompt_tokens,
        output_tokens: completion_tokens,
        total_tokens: token_field(usage, "total_tokens", "totalTokens"),
        cache_read_input_tokens: cached_prompt_tokens,
        cache_creation_input_tokens: cache_write_tokens,
        reasoning_output_tokens: reasoning_completion_tokens,
    };
    Some(
        provider_usage
            .normalize(ProviderTokenUsageSemantics::OpenAiCompatibleChat)
            .telemetry,
    )
    .filter(|usage| usage.has_tokens())
    .or_else(|| usage_from_total_only(usage))
}

pub(super) fn parse_openai_responses_usage(
    response: Option<&serde_json::Value>,
) -> Option<TokenUsageTelemetry> {
    let response = response?;
    response
        .get("usage")
        .or_else(|| response.pointer("/response/usage"))
        .and_then(usage_from_openai_responses_usage)
}

fn usage_from_openai_responses_usage(usage: &serde_json::Value) -> Option<TokenUsageTelemetry> {
    let details_cached_tokens = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("inputTokensDetails"))
        .map_or(0, |details| {
            token_field(details, "cached_tokens", "cachedTokens")
        });
    let provider_cache_read_tokens =
        token_field(usage, "cache_read_input_tokens", "cacheReadInputTokens");
    let details_cache_write_tokens = usage
        .get("input_tokens_details")
        .or_else(|| usage.get("inputTokensDetails"))
        .map_or(0, |details| {
            token_field(details, "cache_write_tokens", "cacheWriteTokens")
        });
    let provider_cache_write_tokens = token_field(
        usage,
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
    );
    let provider_usage = ProviderTokenUsage {
        input_tokens: token_field(usage, "input_tokens", "inputTokens"),
        output_tokens: token_field(usage, "output_tokens", "outputTokens"),
        total_tokens: token_field(usage, "total_tokens", "totalTokens"),
        cache_read_input_tokens: details_cached_tokens.max(provider_cache_read_tokens),
        cache_creation_input_tokens: details_cache_write_tokens.max(provider_cache_write_tokens),
        reasoning_output_tokens: usage
            .get("output_tokens_details")
            .or_else(|| usage.get("outputTokensDetails"))
            .map_or(0, |details| {
                token_field(details, "reasoning_tokens", "reasoningTokens")
            }),
    };
    Some(
        provider_usage
            .normalize(ProviderTokenUsageSemantics::OpenAiResponses)
            .telemetry,
    )
    .filter(|usage| usage.has_tokens())
    .or_else(|| usage_from_total_only(usage))
}

pub(super) fn merge_anthropic_usage(
    token_usage: &mut Option<TokenUsageTelemetry>,
    usage: Option<&serde_json::Value>,
) {
    let Some(usage) = usage else {
        return;
    };
    let current = token_usage.unwrap_or_default();
    let input_tokens =
        optional_token_field(usage, "input_tokens", "inputTokens").unwrap_or(current.input_tokens);
    let cache_creation_tokens = optional_token_field(
        usage,
        "cache_creation_input_tokens",
        "cacheCreationInputTokens",
    );
    let cache_read_tokens =
        optional_token_field(usage, "cache_read_input_tokens", "cacheReadInputTokens");
    let output_tokens = optional_token_field(usage, "output_tokens", "outputTokens")
        .unwrap_or(current.output_tokens);
    let provider_usage = ProviderTokenUsage {
        input_tokens,
        output_tokens,
        total_tokens: token_field(usage, "total_tokens", "totalTokens"),
        cache_read_input_tokens: cache_read_tokens.unwrap_or(0),
        cache_creation_input_tokens: cache_creation_tokens.unwrap_or(0),
        reasoning_output_tokens: current.reasoning_output_tokens,
    };
    let normalized = provider_usage
        .normalize(ProviderTokenUsageSemantics::AnthropicMessages)
        .telemetry;

    let merged = TokenUsageTelemetry::new(
        normalized.input_tokens,
        if normalized.cache_read_input_tokens == 0 {
            current.cache_read_input_tokens
        } else {
            normalized.cache_read_input_tokens
        },
        if normalized.cache_write_input_tokens == 0 {
            current.cache_write_input_tokens
        } else {
            normalized.cache_write_input_tokens
        },
        normalized.output_tokens,
        normalized.reasoning_output_tokens,
    );
    let current_context_tokens = if provider_usage.total_tokens > 0 {
        provider_usage.total_tokens
    } else {
        merged.total_tokens()
    };
    *token_usage = Some(merged.with_current_context_tokens(current_context_tokens))
        .filter(|usage| usage.has_tokens())
        .or_else(|| usage_from_total_only(usage));
}

fn usage_from_total_only(usage: &serde_json::Value) -> Option<TokenUsageTelemetry> {
    let total_tokens = optional_token_field(usage, "total_tokens", "totalTokens")?;
    (total_tokens > 0).then_some(
        TokenUsageTelemetry::new(total_tokens, 0, 0, 0, 0)
            .with_current_context_tokens(total_tokens),
    )
}

fn token_field(value: &serde_json::Value, snake_case: &str, camel_case: &str) -> u64 {
    optional_token_field(value, snake_case, camel_case).unwrap_or(0)
}

fn optional_token_field(
    value: &serde_json::Value,
    snake_case: &str,
    camel_case: &str,
) -> Option<u64> {
    value
        .get(snake_case)
        .or_else(|| value.get(camel_case))
        .and_then(serde_json::Value::as_u64)
}
