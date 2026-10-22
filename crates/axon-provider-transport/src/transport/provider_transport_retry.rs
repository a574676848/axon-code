//! Provider transport 统一退避计算。

use std::time::Duration;

use axon_agent_runtime::provider_streaming_retry_policy;

pub(super) fn retry_delay(attempt: u64, requested_delay_ms: Option<u64>) -> Duration {
    let policy = provider_streaming_retry_policy();
    if let Some(requested_delay_ms) = requested_delay_ms {
        return Duration::from_millis(requested_delay_ms.min(policy.max_delay_ms));
    }

    let shift = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(u64::MAX);
    let raw_delay_ms = policy
        .base_delay_ms
        .saturating_mul(multiplier)
        .min(policy.max_delay_ms);
    let jitter_span = policy.jitter_percent.saturating_mul(2).saturating_add(1);
    let mut random = [0_u8; 8];
    let random_value =
        getrandom::fill(&mut random).map_or(attempt, |()| u64::from_le_bytes(random));
    let jitter_offset = random_value % jitter_span;
    let jitter_percent = 100_u64
        .saturating_sub(policy.jitter_percent)
        .saturating_add(jitter_offset);
    Duration::from_millis(raw_delay_ms.saturating_mul(jitter_percent) / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_delay_stays_within_policy_bounds() {
        let policy = provider_streaming_retry_policy();
        let delay = retry_delay(1, None).as_millis() as u64;
        let lower = policy
            .base_delay_ms
            .saturating_mul(100 - policy.jitter_percent)
            / 100;
        let upper = policy
            .base_delay_ms
            .saturating_mul(100 + policy.jitter_percent)
            / 100;

        assert!((lower..=upper).contains(&delay));
    }

    #[test]
    fn requested_delay_is_capped_by_policy() {
        let policy = provider_streaming_retry_policy();

        assert_eq!(
            retry_delay(1, Some(policy.max_delay_ms.saturating_mul(2))),
            Duration::from_millis(policy.max_delay_ms)
        );
    }
}
