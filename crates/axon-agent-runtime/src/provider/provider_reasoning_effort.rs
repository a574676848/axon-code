//! Provider reasoning effort 能力协商。

use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

use axon_config::{
    ANTHROPIC_REASONING_EFFORT_VALUES, OPENAI_REASONING_EFFORT_VALUES, REASONING_EFFORT_VALUES,
    normalize_reasoning_effort,
};

use crate::{ProviderStreamProtocol, ProviderStreamingTransportError};

const REASONING_EFFORT_ERROR_HTTP_STATUS: &[u16] = &[400, 422];
const REASONING_EFFORT_ERROR_FIELDS: &[&str] = &[
    "reasoning_effort",
    "reasoning.effort",
    "output_config.effort",
];
const REASONING_EFFORT_ERROR_MARKERS: &[&str] = &[
    "unsupported",
    "not supported",
    "does not support",
    "invalid",
    "must be one of",
    "allowed values",
];

/// 一次请求最多需要尝试的 effort 降级次数。
pub(super) const REASONING_EFFORT_MAX_DOWNGRADE_ATTEMPTS: usize =
    OPENAI_REASONING_EFFORT_VALUES.len();

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReasoningEffortCacheKey {
    provider_name: String,
    protocol: &'static str,
    model_id: String,
}

type ReasoningEffortCache = HashMap<ReasoningEffortCacheKey, Option<String>>;

static REASONING_EFFORT_CACHE: OnceLock<RwLock<ReasoningEffortCache>> = OnceLock::new();

/// 单个 Provider/协议/模型请求的 reasoning effort 协商状态。
pub(super) struct ProviderReasoningEffortNegotiation {
    key: Option<ReasoningEffortCacheKey>,
    protocol: ProviderStreamProtocol,
    effective: Option<String>,
    capability_downgraded: bool,
}

impl ProviderReasoningEffortNegotiation {
    pub(super) fn new(
        provider_name: &str,
        protocol: ProviderStreamProtocol,
        model_id: &str,
        requested: Option<&str>,
    ) -> Self {
        let Some(requested) = requested else {
            return Self {
                key: None,
                protocol,
                effective: None,
                capability_downgraded: false,
            };
        };
        let effective = Some(requested.to_owned());
        let Some(normalized) = normalize_reasoning_effort(requested) else {
            return Self {
                key: None,
                protocol,
                effective,
                capability_downgraded: false,
            };
        };
        if !REASONING_EFFORT_VALUES.contains(&normalized) {
            return Self {
                key: None,
                protocol,
                effective,
                capability_downgraded: false,
            };
        }
        let effective = protocol_values(protocol)
            .contains(&normalized)
            .then(|| normalized.to_owned());
        let key = ReasoningEffortCacheKey {
            provider_name: provider_name.to_owned(),
            protocol: protocol_code(protocol),
            model_id: model_id.to_owned(),
        };
        let cached = reasoning_effort_cache()
            .read()
            .ok()
            .and_then(|cache| cache.get(&key).cloned());
        Self {
            key: Some(key),
            protocol,
            effective: cached
                .map(|cached| {
                    effective.as_deref().and_then(|requested| {
                        cap_to_requested(protocol, requested, cached.as_deref())
                    })
                })
                .unwrap_or(effective),
            capability_downgraded: false,
        }
    }

    pub(super) fn effective(&self) -> Option<&str> {
        self.effective.as_deref()
    }

    pub(super) fn downgrade_for(&mut self, error: &ProviderStreamingTransportError) -> bool {
        if !is_reasoning_effort_capability_error(error) || self.key.is_none() {
            return false;
        }
        let Some(current) = self.effective.as_deref() else {
            return false;
        };
        self.effective = next_lower_effort(self.protocol, current).map(str::to_owned);
        self.capability_downgraded = true;
        true
    }

    pub(super) fn remember_success(&self) {
        if !self.capability_downgraded {
            return;
        }
        let Some(key) = self.key.as_ref() else {
            return;
        };
        if let Ok(mut cache) = reasoning_effort_cache().write() {
            cache.insert(key.clone(), self.effective.clone());
        }
    }
}

fn reasoning_effort_cache() -> &'static RwLock<ReasoningEffortCache> {
    REASONING_EFFORT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn protocol_values(protocol: ProviderStreamProtocol) -> &'static [&'static str] {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
            OPENAI_REASONING_EFFORT_VALUES
        }
        ProviderStreamProtocol::AnthropicMessages => ANTHROPIC_REASONING_EFFORT_VALUES,
    }
}

fn protocol_code(protocol: ProviderStreamProtocol) -> &'static str {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => "openai-responses",
        ProviderStreamProtocol::OpenAiCompatible => "openai-chat-completions",
        ProviderStreamProtocol::AnthropicMessages => "anthropic-messages",
    }
}

fn cap_to_requested(
    protocol: ProviderStreamProtocol,
    requested: &str,
    cached: Option<&str>,
) -> Option<String> {
    let cached = cached?;
    let values = protocol_values(protocol);
    let requested_index = values.iter().position(|value| *value == requested)?;
    let cached_index = values.iter().position(|value| *value == cached)?;
    Some(values[requested_index.min(cached_index)].to_string())
}

fn next_lower_effort(protocol: ProviderStreamProtocol, current: &str) -> Option<&'static str> {
    let values = protocol_values(protocol);
    let index = values.iter().position(|value| *value == current)?;
    index.checked_sub(1).map(|next| values[next])
}

fn is_reasoning_effort_capability_error(error: &ProviderStreamingTransportError) -> bool {
    if !error
        .http_status
        .is_some_and(|status| REASONING_EFFORT_ERROR_HTTP_STATUS.contains(&status))
    {
        return false;
    }
    let message = error.message.to_ascii_lowercase();
    let param_matches = serde_json::from_str::<serde_json::Value>(&error.message)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/param")
                .and_then(serde_json::Value::as_str)
                .map(str::to_ascii_lowercase)
        })
        .is_some_and(|param| {
            REASONING_EFFORT_ERROR_FIELDS
                .iter()
                .any(|field| param == *field)
        });
    param_matches
        || (REASONING_EFFORT_ERROR_FIELDS
            .iter()
            .any(|field| message.contains(field))
            && REASONING_EFFORT_ERROR_MARKERS
                .iter()
                .any(|marker| message.contains(marker)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROTOCOL: ProviderStreamProtocol = ProviderStreamProtocol::OpenAiCompatible;

    #[test]
    fn capability_downgrade_is_cached_after_success() {
        let provider = "effort-capability-downgrade";
        let model = "test-model";
        let mut negotiation =
            ProviderReasoningEffortNegotiation::new(provider, PROTOCOL, model, Some("max"));
        let error = capability_error();

        assert!(negotiation.downgrade_for(&error));
        assert_eq!(negotiation.effective(), Some("xhigh"));
        negotiation.remember_success();

        let cached =
            ProviderReasoningEffortNegotiation::new(provider, PROTOCOL, model, Some("max"));
        assert_eq!(cached.effective(), Some("xhigh"));
    }

    #[test]
    fn lower_request_does_not_reduce_existing_capability_ceiling() {
        let provider = "effort-existing-capability-ceiling";
        let model = "test-model";
        let mut negotiation =
            ProviderReasoningEffortNegotiation::new(provider, PROTOCOL, model, Some("max"));
        assert!(negotiation.downgrade_for(&capability_error()));
        negotiation.remember_success();

        let lower = ProviderReasoningEffortNegotiation::new(provider, PROTOCOL, model, Some("low"));
        assert_eq!(lower.effective(), Some("low"));
        lower.remember_success();

        let cached =
            ProviderReasoningEffortNegotiation::new(provider, PROTOCOL, model, Some("max"));
        assert_eq!(cached.effective(), Some("xhigh"));
    }

    fn capability_error() -> ProviderStreamingTransportError {
        ProviderStreamingTransportError::new(
            r#"{"error":{"message":"Unsupported value","param":"reasoning_effort"}}"#,
            Some(400),
        )
    }
}
