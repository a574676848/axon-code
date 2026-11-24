//! Provider streaming 瞬时重试与错误投影。

use super::{
    PROVIDER_STREAMING_REASON_PROVIDER_ERROR, PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED,
    ProviderStreamingAttemptPlan, ProviderStreamingRunnerContext, ProviderStreamingRunnerDecision,
    ProviderStreamingTransportError, ProviderStreamingTransportFailureKind,
    is_provider_context_overflow_message, provider_streaming_retry_policy,
};

/// 流内终态失败在同渠道允许消耗的重试次数。
///
/// Provider 已经用 `response.failed` / `response.incomplete` 宣告结论，说明流建立后上游链路已断：
/// 继续在同渠道重试通常只是重复等待，把重试预算留给外层换渠道才是正确动作。
const STREAM_TERMINAL_IN_CHANNEL_RETRIES: u32 = 1;

pub(super) fn handle_transport_error(
    _context: &ProviderStreamingRunnerContext,
    plan: ProviderStreamingAttemptPlan,
    error: ProviderStreamingTransportError,
) -> ProviderStreamingRunnerDecision {
    if error.failure_kind == ProviderStreamingTransportFailureKind::ContextOverflow
        || is_provider_context_overflow_message(&error.message)
    {
        return failed_with_reason(error);
    }
    if error.failure_kind.is_retryable() {
        if stream_terminal_retry_budget_exhausted(&plan, &error) {
            return failed_with_reason(error);
        }
        retry_transient_error(plan, error)
    } else {
        failed_with_reason(error)
    }
}

/// 流内终态失败是否已在同渠道用完允许的重试次数。
fn stream_terminal_retry_budget_exhausted(
    plan: &ProviderStreamingAttemptPlan,
    error: &ProviderStreamingTransportError,
) -> bool {
    if error.provider_event_type.is_none() {
        return false;
    }
    let policy = provider_streaming_retry_policy();
    let used = policy
        .stream_max_retries
        .saturating_sub(plan.transient_retry_budget_remaining);
    used >= STREAM_TERMINAL_IN_CHANNEL_RETRIES
}

fn retry_transient_error(
    mut plan: ProviderStreamingAttemptPlan,
    error: ProviderStreamingTransportError,
) -> ProviderStreamingRunnerDecision {
    if is_provider_empty_output(&error.message) {
        return failed_with_reason(error);
    }
    if plan.transient_retry_budget_remaining == 0 {
        return failed_with_reason(ProviderStreamingTransportError {
            message: format!(
                "provider transient retry budget exhausted；last_error={}",
                error.message
            ),
            ..error
        });
    }
    plan.reasons
        .push(PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED.to_owned());
    plan.transient_retry_budget_remaining = plan.transient_retry_budget_remaining.saturating_sub(1);
    ProviderStreamingRunnerDecision::Retry(Box::new(plan))
}

fn is_provider_empty_output(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("emptyinput") || normalized.contains("empty input")
}

fn failed_with_reason(error: ProviderStreamingTransportError) -> ProviderStreamingRunnerDecision {
    ProviderStreamingRunnerDecision::Failed(ProviderStreamingTransportError {
        message: format!(
            "{}；reason={PROVIDER_STREAMING_REASON_PROVIDER_ERROR}",
            error.message
        ),
        ..error
    })
}
