use super::*;

pub(super) fn context_tokens_for_next_request(
    messages: &[AgentModelMessage],
    provider_context_tokens: Option<u64>,
    provider_accounted_message_count: Option<usize>,
    input: &ProviderStreamingTransportInput,
) -> Option<u64> {
    let format = provider_wire_format(input.protocol);
    if let Some(provider_tokens) = provider_context_tokens.filter(|tokens| *tokens > 0) {
        let tail_start = provider_accounted_message_count
            .unwrap_or(messages.len())
            .min(messages.len());
        let local_tail_tokens = messages[tail_start..]
            .iter()
            .fold(0_u64, |tokens, message| {
                tokens.saturating_add(
                    crate::token_estimator::estimated_message_tokens_with_capabilities(
                        message,
                        format,
                        &input.model_tools,
                        input.supports_vision,
                    ),
                )
            });
        return Some(provider_tokens.saturating_add(local_tail_tokens));
    }
    let estimate = estimated_history_tokens_for_provider_with_capabilities(
        messages,
        format,
        &input.model_tools,
        input.supports_vision,
    );
    (estimate > 0).then_some(estimate)
}

pub(super) fn token_usage_option(usage: TokenUsageTelemetry) -> Option<TokenUsageTelemetry> {
    usage.has_tokens().then_some(usage)
}

pub(super) fn refresh_prompt_cache_plan(
    input: &mut ProviderStreamingTransportInput,
) -> AxonResult<()> {
    input.prompt_cache_plan = provider_streaming_prompt_cache_plan(input)?;
    Ok(())
}

pub(super) fn provider_wire_format(
    protocol: crate::ProviderStreamProtocol,
) -> crate::PromptCacheProviderWireFormat {
    match protocol {
        crate::ProviderStreamProtocol::OpenAiResponses => {
            crate::PromptCacheProviderWireFormat::OpenAiResponses
        }
        crate::ProviderStreamProtocol::OpenAiCompatible => {
            crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat
        }
        crate::ProviderStreamProtocol::AnthropicMessages => {
            crate::PromptCacheProviderWireFormat::AnthropicMessages
        }
    }
}

pub(super) fn append_provider_retry_events(
    turn_events: &mut Vec<AgentTurnEvent>,
    turn_id: TurnId,
    report: &ProviderStreamingTransportReport,
    model_id: &str,
) {
    let attempts = report.attempt_count.saturating_sub(1);
    if attempts == 0 {
        return;
    }

    let retry_reasons = retry_reasons(&report.reasons);
    for (index, reason) in retry_reasons.iter().take(attempts).enumerate() {
        let attempt_index = index + 2;
        turn_events.push(
            AgentTurnEvent::new(
                turn_id,
                AgentTurnEventKind::ProviderRetry,
                provider_retry_summary(reason, attempt_index, report.attempt_count),
            )
            .with_part_id(agent_turn_provider_retry_part_id(turn_id, attempt_index))
            .with_tool_name(model_id)
            .with_phase("streaming")
            .with_status("retrying")
            .with_metadata(serde_json::json!({
                "reason": reason,
                "attemptIndex": attempt_index,
                "attemptCount": report.attempt_count,
                "remainingBudget": report.attempt_count.saturating_sub(attempt_index),
            })),
        );
    }
}

fn retry_reasons(reasons: &[String]) -> Vec<String> {
    reasons
        .iter()
        .filter(|reason| is_provider_retry_reason(reason))
        .cloned()
        .collect()
}

fn is_provider_retry_reason(reason: &str) -> bool {
    reason.contains(":overflow_retry_planned") || reason.contains(":transient_retry_planned")
}

fn provider_retry_summary(reason: &str, attempt_index: usize, attempt_count: usize) -> String {
    format!(
        "Provider 重试 {}/{}：{}",
        attempt_index.saturating_sub(1),
        attempt_count.saturating_sub(1),
        provider_retry_reason_label(reason)
    )
}

fn provider_retry_reason_label(reason: &str) -> &'static str {
    if reason.contains(":overflow_retry_planned") {
        "上下文溢出恢复"
    } else if reason.contains(":transient_retry_planned") {
        "瞬时传输错误"
    } else {
        "Provider 重试"
    }
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
