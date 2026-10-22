use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axon_remote_feishu::{
    REMOTE_FEISHU_API_BASE_URL, RemoteFeishuBotIdentity, RemoteFeishuBotInfoRequestPlan,
    RemoteFeishuDeliveryRuntime, RemoteFeishuHttpRequestPlan, RemoteFeishuWebSocketDispatchPlan,
    RemoteNotificationAdapterKind, RemoteNotificationRenderArtifact,
};
use axon_remote_runtime::{
    RemoteNotificationDispatchStatus, RemoteNotificationOutboxRecord, RemoteTransportKind,
};
use axon_rustls_provider::ensure_rustls_crypto_provider;

use super::RemoteCliContext;

const FEISHU_IDENTITY_CACHE_TTL: Duration = Duration::from_mins(5);

#[derive(Clone)]
struct CachedIdentity {
    identity: RemoteFeishuBotIdentity,
    valid_until: Instant,
}

static IDENTITY_CACHE: OnceLock<Mutex<HashMap<String, CachedIdentity>>> = OnceLock::new();

pub(super) fn resolve_feishu_bot_identity(
    context: &RemoteCliContext,
) -> Option<RemoteFeishuBotIdentity> {
    let key = format!(
        "{}|{}",
        context.feishu_websocket.app_id, context.feishu_websocket.api_base_url
    );
    let cache = IDENTITY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(entry) = cache.lock().ok()?.get(&key).cloned()
        && Instant::now() < entry.valid_until
    {
        axon_logger::debug("Feishu bot identity 命中 TTL cache");
        return Some(entry.identity);
    }
    let identity = resolve_feishu_bot_identity_uncached(context);
    if let Some(identity) = identity.clone() {
        if let Ok(mut state) = cache.lock() {
            state.insert(
                key,
                CachedIdentity {
                    identity: identity.clone(),
                    valid_until: Instant::now() + FEISHU_IDENTITY_CACHE_TTL,
                },
            );
        }
        return Some(identity);
    }
    let stale = cache.lock().ok()?.get(&key).cloned();
    if let Some(entry) = stale {
        axon_logger::warning("Feishu bot identity 刷新失败，使用 stale TTL cache");
        return Some(entry.identity);
    }
    None
}

fn resolve_feishu_bot_identity_uncached(
    context: &RemoteCliContext,
) -> Option<RemoteFeishuBotIdentity> {
    if !context.feishu_websocket.enabled
        || context.feishu_websocket.app_id.trim().is_empty()
        || context.feishu_websocket.app_secret_env.trim().is_empty()
    {
        return None;
    }
    let app_secret = context.feishu_app_secret()?;
    let dispatch = RemoteFeishuWebSocketDispatchPlan {
        adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
        app_id: context.feishu_websocket.app_id.clone(),
        app_secret_env: context.feishu_websocket.app_secret_env.clone(),
        websocket_url: context.feishu_websocket.websocket_url.clone(),
        artifact: RemoteNotificationRenderArtifact {
            adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
            transport: RemoteTransportKind::Feishu,
            content_type: "application/json".to_owned(),
            payload: "{}".to_owned(),
            network_send_allowed: false,
        },
        notification_sending_enabled: false,
        reasons: vec![],
    };
    let accepted = RemoteNotificationOutboxRecord {
        schema_version: axon_agent_runtime::REMOTE_NOTIFICATION_OUTBOX_SCHEMA_VERSION,
        dispatch_id: "identity-probe".to_owned(),
        session_id: 1,
        subscriber_id: "identity".to_owned(),
        connection_id: "identity".to_owned(),
        pairing_id: "identity".to_owned(),
        transport: RemoteTransportKind::Feishu,
        status: RemoteNotificationDispatchStatus::Attempting,
        delivery_count: 1,
        dropped_count: 0,
        delivery_kinds: vec![],
        recorded_at_ms: 1,
        updated_at_ms: 1,
        attempt_count: 0,
        next_retry_after_ms: None,
        last_error: None,
        failure: None,
        adapter_message_id: None,
        attempt_started_at_ms: None,
        attempt_owner: None,
        recovery_marker: None,
        source_trace: "identity".to_owned(),
        reason: "identity".to_owned(),
        sequence: None,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        raw_body_persisted: false,
        raw_payload_persisted: false,
    };
    let plan = RemoteFeishuDeliveryRuntime::plan_requests(
        &dispatch,
        (!context.feishu_websocket.api_base_url.trim().is_empty())
            .then_some(context.feishu_websocket.api_base_url.as_str()),
        &accepted,
    )
    .ok()?;
    let token_request = RemoteFeishuHttpRequestPlan {
        body_json: serde_json::json!({
            "app_id": context.feishu_websocket.app_id,
            "app_secret": app_secret,
        })
        .to_string(),
        ..plan.token_request
    };
    let token_response = send_http_request(&token_request).ok()?;
    let tenant_access_token = {
        let value = serde_json::from_str::<serde_json::Value>(&token_response.body).ok()?;
        value["tenant_access_token"].as_str()?.to_owned()
    };
    let base = feishu_identity_api_base_url(context);
    let identity_request =
        axon_remote_feishu::RemoteFeishuIdentityRuntime::plan_request(base, &tenant_access_token)
            .ok()?;
    let identity_response = send_http_get_request(&identity_request).ok()?;
    axon_remote_feishu::RemoteFeishuIdentityRuntime::parse_identity(&identity_response).ok()
}

fn feishu_identity_api_base_url(context: &RemoteCliContext) -> &str {
    normalized_feishu_identity_api_base_url(&context.feishu_websocket.api_base_url)
}

fn normalized_feishu_identity_api_base_url(api_base_url: &str) -> &str {
    let configured = api_base_url.trim();
    if configured.is_empty() {
        REMOTE_FEISHU_API_BASE_URL
    } else {
        configured.trim_end_matches('/')
    }
}

fn send_http_request(
    request: &RemoteFeishuHttpRequestPlan,
) -> Result<axon_remote_feishu::RemoteFeishuHttpResponse, String> {
    let agent = feishu_identity_agent();
    let mut req = agent.post(&request.url);
    for (name, value) in &request.headers {
        req = req.header(name, value);
    }
    req.send(&request.body_json)
        .map(read_http_response)
        .map_err(|error| format!("Feishu HTTP 请求失败：{error}"))
}

fn send_http_get_request(
    request: &RemoteFeishuBotInfoRequestPlan,
) -> Result<axon_remote_feishu::RemoteFeishuHttpResponse, String> {
    let agent = feishu_identity_agent();
    let mut req = agent.get(&request.url);
    for (name, value) in &request.headers {
        req = req.header(name, value);
    }
    req.call()
        .map(read_http_response)
        .map_err(|error| format!("Feishu bot info 请求失败：{error}"))
}

fn feishu_identity_agent() -> ureq::Agent {
    ensure_rustls_crypto_provider();
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn read_http_response(
    mut response: ureq::http::Response<ureq::Body>,
) -> axon_remote_feishu::RemoteFeishuHttpResponse {
    axon_remote_feishu::RemoteFeishuHttpResponse {
        status: response.status().as_u16(),
        body: response.body_mut().read_to_string().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_identity_api_base_url_trims_configured_trailing_slash() {
        assert_eq!(
            normalized_feishu_identity_api_base_url("https://example.feishu.test/"),
            "https://example.feishu.test"
        );
    }
}
