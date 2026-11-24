//! Provider HTTP 鉴权重试与端点级鉴权记忆。

use super::{StatusResponseParts, provider_transport_request::resolve_auth_credential};
use crate::{ProviderStreamingHttpRequestPlan, ProviderStreamingTransportOutcome};
use reqwest::header::HeaderMap;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{Mutex, OnceLock},
};

pub(super) fn should_retry_with_bearer(
    request_plan: &ProviderStreamingHttpRequestPlan,
    response: &StatusResponseParts,
) -> bool {
    if response.status != 401 {
        return false;
    }
    let Some(auth_header) = request_plan.auth_header.as_ref() else {
        return false;
    };
    if !auth_header.header_name.eq_ignore_ascii_case("x-api-key") {
        return false;
    }
    let message = response.message.to_ascii_lowercase();
    (message.contains("bearer")
        && (message.contains("authorization") || message.contains("must start")))
        || message.contains("missing_api_key")
        || message.contains("invalid_api_key")
}

pub(super) fn preferred_auth_request_plan(
    request_plan: &ProviderStreamingHttpRequestPlan,
) -> ProviderStreamingHttpRequestPlan {
    if bearer_auth_is_remembered(request_plan) {
        bearer_retry_request_plan(request_plan)
    } else {
        request_plan.clone()
    }
}

fn auth_memory() -> &'static Mutex<HashMap<u64, bool>> {
    static AUTH_MEMORY: OnceLock<Mutex<HashMap<u64, bool>>> = OnceLock::new();
    AUTH_MEMORY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn auth_memory_key(request_plan: &ProviderStreamingHttpRequestPlan) -> Option<u64> {
    let auth = request_plan.auth_header.as_ref()?;
    let credential = resolve_auth_credential(&auth.credential).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request_plan.url.hash(&mut hasher);
    credential.hash(&mut hasher);
    Some(hasher.finish())
}

fn bearer_auth_is_remembered(request_plan: &ProviderStreamingHttpRequestPlan) -> bool {
    auth_memory_key(request_plan)
        .and_then(|key| auth_memory().lock().ok()?.get(&key).copied())
        .unwrap_or(false)
}

pub(super) fn remember_bearer_auth(request_plan: &ProviderStreamingHttpRequestPlan) {
    if let Some(key) = auth_memory_key(request_plan)
        && let Ok(mut memory) = auth_memory().lock()
    {
        memory.insert(key, true);
    }
}

pub(super) fn outcome_is_success(outcome: &ProviderStreamingTransportOutcome) -> bool {
    match outcome {
        ProviderStreamingTransportOutcome::StreamCompleted { .. }
        | ProviderStreamingTransportOutcome::AggregatedStream { .. } => true,
        ProviderStreamingTransportOutcome::VisibleOutput(inner) => outcome_is_success(inner),
        ProviderStreamingTransportOutcome::Error(_) => false,
    }
}

pub(super) fn bearer_retry_request_plan(
    request_plan: &ProviderStreamingHttpRequestPlan,
) -> ProviderStreamingHttpRequestPlan {
    let mut retry_plan = request_plan.clone();
    if let Some(auth_header) = retry_plan.auth_header.as_mut() {
        auth_header.header_name = "Authorization".to_owned();
        auth_header.value_prefix = "Bearer ".to_owned();
    }
    retry_plan
}

pub(super) fn provider_request_id(headers: &HeaderMap) -> Option<String> {
    const REQUEST_ID_HEADERS: &[&str] =
        &["x-request-id", "openai-request-id", "request-id", "cf-ray"];
    REQUEST_ID_HEADERS.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned)
    })
}
