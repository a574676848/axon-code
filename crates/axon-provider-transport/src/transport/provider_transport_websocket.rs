use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::{Duration, Instant},
};

use axon_config::provider_credential;
use axon_domain::{AxonError, AxonResult};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use tokio::sync::Mutex;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        error::Error as TungsteniteError,
        http::{HeaderName, HeaderValue},
    },
};
use url::Url;

use crate::runtime_limits::{
    PROVIDER_WEBSOCKET_CONNECT_TIMEOUT_MS, PROVIDER_WEBSOCKET_CONNECTION_MAX_AGE_MS,
    PROVIDER_WEBSOCKET_SESSION_IDLE_TIMEOUT_MS,
};
use crate::{
    ProviderStreamingHttpRequestPlan, ProviderStreamingTransportError,
    ProviderStreamingTransportOutcome,
};
use axon_agent_runtime::{
    ProviderStreamingCredentialPlan, ProviderStreamingTransportFailureKind, RunCancelToken,
};

use super::{
    PROVIDER_WEBSOCKET_TRANSPORT_OK_STATUS, ProviderWebSocketTransportInput,
    observe_websocket_rate_limit, transport_error, wait_for_rate_token,
};
use crate::provider_transport::ProviderWebSocketConnectionTelemetry;

#[path = "provider_response_chain.rs"]
mod provider_response_chain;
#[path = "provider_transport_websocket_stream.rs"]
mod provider_transport_websocket_stream;
#[path = "provider_transport_websocket_pump.rs"]
mod provider_websocket_pump;
use provider_response_chain::{
    PreparedWebSocketRequest, WebSocketResponseChain, WebSocketResponseState,
    observe_response_event, prepare_request, prepare_warmup_request,
};
use provider_transport_websocket_stream::read_websocket_stream;
use provider_websocket_pump::ProviderWebSocketPump;

#[cfg(test)]
#[path = "provider_transport_websocket_failure_tests.rs"]
mod failure_tests;
#[cfg(test)]
#[path = "provider_transport_websocket_tests.rs"]
mod tests;

#[derive(Clone, PartialEq, Eq, Hash)]
struct WebSocketSessionKey {
    logical_session: Option<String>,
    url: String,
    headers: Vec<(String, String)>,
}

struct WebSocketSession {
    state: Mutex<WebSocketSessionState>,
    last_used_at: StdMutex<Instant>,
}

const PROVIDER_WEBSOCKET_MAX_CACHED_SESSIONS: usize = 128;

impl WebSocketSession {
    fn touch(&self, now: Instant) {
        *self
            .last_used_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = now;
    }

    fn idle_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_used_at())
            >= Duration::from_millis(PROVIDER_WEBSOCKET_SESSION_IDLE_TIMEOUT_MS)
    }

    fn last_used_at(&self) -> Instant {
        *self
            .last_used_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Default)]
struct WebSocketSessionState {
    socket: Option<ProviderWebSocketPump>,
    chain: Option<WebSocketResponseChain>,
    connected_at: Option<Instant>,
}

struct WebSocketReadResult {
    response_state: WebSocketResponseState,
    closed_before_first_response_frame: bool,
    close_detail: Option<String>,
}

pub(super) async fn prewarm_websocket(input: ProviderWebSocketTransportInput) -> AxonResult<()> {
    let cancel_token = RunCancelToken::default();
    let request = build_websocket_request(&input.request_plan, input.websocket_url.as_deref())
        .map_err(|_| AxonError::provider_failure("provider websocket 预热请求无效"))?;
    let session = websocket_session(prewarm_session_key(&request));
    let mut state = session.state.lock().await;
    if state
        .socket
        .as_ref()
        .is_none_or(ProviderWebSocketPump::is_closed)
        || websocket_connection_expired(&state, Instant::now())
    {
        if !wait_for_rate_token(&input.request_plan, &cancel_token).await {
            return Ok(());
        }
        ensure_connected_with_cancel(
            &mut state,
            request,
            Duration::from_millis(input.timeout_ms),
            &cancel_token,
        )
        .await?;
    }
    if !input.request_plan.server_conversation_state_allowed {
        state.chain = None;
        session.touch(Instant::now());
        return Ok(());
    }
    if state.chain.is_some() {
        return Ok(());
    }
    let Some(prepared) = prepare_warmup_request(&input.request_plan.body_text) else {
        return Ok(());
    };
    let socket = state
        .socket
        .as_mut()
        .ok_or_else(|| AxonError::provider_failure("provider websocket 预热连接不可用"))?;
    socket
        .send(Message::Text(prepared.wire_text.into()))
        .await
        .map_err(|_| AxonError::provider_failure("provider websocket 预热请求发送失败"))?;
    let result = read_websocket_stream(
        socket,
        &input.request_plan,
        Duration::from_millis(input.timeout_ms),
        &cancel_token,
        &mut |_| {},
        &mut || {},
    )
    .await?;
    if result.closed_before_first_response_frame {
        crate::agent_run_transport::mark_websocket_endpoint_fallback(&input.request_plan.url);
        reset_session_state(&mut state);
        return Err(AxonError::provider_transport(
            "provider websocket 预热在首个响应帧前关闭",
            None,
        ));
    }
    let failed = result.response_state.failure.is_some();
    let completed = result
        .response_state
        .completed
        .filter(|_| !failed)
        .ok_or_else(|| AxonError::provider_failure("provider websocket 预热未返回 response id"))?;
    let request = prepared
        .full_request
        .ok_or_else(|| AxonError::provider_failure("provider websocket 预热请求状态无效"))?;
    state.chain = Some(WebSocketResponseChain::bootstrap(request, completed));
    session.touch(Instant::now());
    Ok(())
}

pub(super) async fn send_websocket_async(
    input: ProviderWebSocketTransportInput,
    cancel_token: &RunCancelToken,
) -> AxonResult<ProviderStreamingTransportOutcome> {
    send_websocket_async_with_frame_and_progress_callback(input, cancel_token, |_| {}, || {}).await
}

pub(super) async fn send_websocket_async_with_frame_and_progress_callback<F, G>(
    input: ProviderWebSocketTransportInput,
    cancel_token: &RunCancelToken,
    mut on_frame: F,
    mut on_progress: G,
) -> AxonResult<ProviderStreamingTransportOutcome>
where
    F: FnMut(&str),
    G: FnMut(),
{
    if input
        .logical_session
        .as_deref()
        .is_none_or(|session| session.trim().is_empty())
    {
        return Err(AxonError::provider_failure(
            "provider websocket 真实请求缺少逻辑会话",
        ));
    }
    if cancel_token.is_cancelled() {
        return Ok(transport_error("provider websocket streaming 已取消", None));
    }
    if !wait_for_rate_token(&input.request_plan, cancel_token).await {
        return Ok(transport_error("provider websocket streaming 已取消", None));
    }
    send_websocket_request(input, cancel_token, &mut on_frame, &mut on_progress).await
}

async fn send_websocket_request<F, G>(
    input: ProviderWebSocketTransportInput,
    cancel_token: &RunCancelToken,
    on_frame: &mut F,
    on_progress: &mut G,
) -> AxonResult<ProviderStreamingTransportOutcome>
where
    F: FnMut(&str),
    G: FnMut(),
{
    let request = match build_websocket_request(&input.request_plan, input.websocket_url.as_deref())
    {
        Ok(request) => request,
        Err(outcome) => return Ok(outcome),
    };
    let stream_idle_timeout = Duration::from_millis(input.timeout_ms);
    let connect_timeout = Duration::from_millis(PROVIDER_WEBSOCKET_CONNECT_TIMEOUT_MS);
    let session = websocket_session(session_key(&request, input.logical_session.as_deref()));
    let mut state = lock_session_state(&session, cancel_token).await?;
    ensure_connected_with_cancel(&mut state, request, connect_timeout, cancel_token).await?;

    let prepared = prepare_request(
        &input.request_plan.body_text,
        state.chain.as_ref(),
        input.request_plan.server_conversation_state_allowed,
    );
    let result = dispatch_request(
        &mut state,
        &input.request_plan,
        &prepared,
        stream_idle_timeout,
        cancel_token,
        on_frame,
        on_progress,
    )
    .await?;
    if result.closed_before_first_response_frame {
        let close_detail = result
            .close_detail
            .map(|detail| format!("；{detail}"))
            .unwrap_or_default();
        reset_session_state(&mut state);
        return Ok(ProviderStreamingTransportOutcome::Error(
            ProviderStreamingTransportError::new(
                format!("provider websocket 在首个响应帧前关闭{close_detail}"),
                None,
            )
            .with_failure_kind(ProviderStreamingTransportFailureKind::Capability),
        ));
    }
    update_response_chain(
        &mut state,
        prepared.full_request,
        &result.response_state,
        input.request_plan.server_conversation_state_allowed,
    );
    session.touch(Instant::now());

    if let Some(failure) = result.response_state.failure {
        return Ok(ProviderStreamingTransportOutcome::Error(
            ProviderStreamingTransportError::new(
                failure.message,
                Some(PROVIDER_WEBSOCKET_TRANSPORT_OK_STATUS),
            )
            .with_failure_kind(failure.kind),
        ));
    }
    if result.response_state.completed.is_none() {
        return Ok(ProviderStreamingTransportOutcome::Error(
            ProviderStreamingTransportError::new(
                "provider websocket streaming 缺少 response.completed",
                Some(PROVIDER_WEBSOCKET_TRANSPORT_OK_STATUS),
            )
            .with_failure_kind(ProviderStreamingTransportFailureKind::Incomplete),
        ));
    }
    Ok(ProviderStreamingTransportOutcome::StreamCompleted {
        http_status: PROVIDER_WEBSOCKET_TRANSPORT_OK_STATUS,
    })
}

async fn ensure_connected_with_cancel(
    state: &mut WebSocketSessionState,
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    timeout: Duration,
    cancel_token: &RunCancelToken,
) -> AxonResult<()> {
    if state
        .socket
        .as_ref()
        .is_some_and(ProviderWebSocketPump::is_closed)
        || websocket_connection_expired(state, Instant::now())
    {
        reset_session_state(state);
    }
    if state.socket.is_none() {
        state.socket = Some(connect_socket_with_cancel(request, timeout, cancel_token).await?);
        state.chain = None;
        state.connected_at = Some(Instant::now());
    }
    Ok(())
}

async fn lock_session_state<'a>(
    session: &'a WebSocketSession,
    cancel_token: &RunCancelToken,
) -> AxonResult<tokio::sync::MutexGuard<'a, WebSocketSessionState>> {
    tokio::select! {
        state = session.state.lock() => Ok(state),
        () = cancel_token.cancelled() => Err(AxonError::provider_transport(
            "provider websocket streaming 已取消",
            None,
        )),
    }
}

fn websocket_connection_expired(state: &WebSocketSessionState, now: Instant) -> bool {
    state.connected_at.is_some_and(|connected_at| {
        now.saturating_duration_since(connected_at)
            >= Duration::from_millis(PROVIDER_WEBSOCKET_CONNECTION_MAX_AGE_MS)
    })
}

pub(super) fn websocket_connection_telemetry(
    input: &ProviderWebSocketTransportInput,
) -> ProviderWebSocketConnectionTelemetry {
    let Ok(request) = build_websocket_request(&input.request_plan, input.websocket_url.as_deref())
    else {
        return ProviderWebSocketConnectionTelemetry::default();
    };
    let session = websocket_session(session_key(&request, input.logical_session.as_deref()));
    let Ok(state) = session.state.try_lock() else {
        return ProviderWebSocketConnectionTelemetry::default();
    };
    let now = Instant::now();
    let connection_reused = state
        .socket
        .as_ref()
        .is_some_and(|socket| !socket.is_closed())
        && !websocket_connection_expired(&state, now);
    let connection_age_ms = connection_reused.then(|| {
        state
            .connected_at
            .map(|connected_at| {
                u64::try_from(now.saturating_duration_since(connected_at).as_millis())
                    .unwrap_or(u64::MAX)
            })
            .unwrap_or_default()
    });
    ProviderWebSocketConnectionTelemetry {
        connection_reused,
        connection_age_ms,
    }
}

async fn dispatch_request<F, G>(
    state: &mut WebSocketSessionState,
    request_plan: &ProviderStreamingHttpRequestPlan,
    prepared: &PreparedWebSocketRequest,
    timeout: Duration,
    cancel_token: &RunCancelToken,
    on_frame: &mut F,
    on_progress: &mut G,
) -> AxonResult<WebSocketReadResult>
where
    F: FnMut(&str),
    G: FnMut(),
{
    let send_result = match state.socket.as_mut() {
        Some(socket) => {
            socket
                .send(Message::Text(prepared.wire_text.clone().into()))
                .await
        }
        None => Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed),
    };
    if send_result.is_err() {
        reset_session_state(state);
        return Err(AxonError::provider_transport(
            "provider websocket 请求发送失败，当前 attempt 已终止",
            None,
        ));
    }
    let socket = state
        .socket
        .as_mut()
        .ok_or_else(|| AxonError::provider_failure("provider websocket 会话不可用"))?;
    match read_websocket_stream(
        socket,
        request_plan,
        timeout,
        cancel_token,
        on_frame,
        on_progress,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            reset_session_state(state);
            Err(error)
        }
    }
}

fn update_response_chain(
    state: &mut WebSocketSessionState,
    request: Option<serde_json::Value>,
    response: &WebSocketResponseState,
    server_conversation_state_allowed: bool,
) {
    if !server_conversation_state_allowed {
        state.chain = None;
        return;
    }
    state.chain = match (
        request,
        response.completed.as_ref(),
        response.failure.is_none(),
    ) {
        (Some(request), Some(completed), true) => Some(WebSocketResponseChain::completed(
            request,
            completed.clone(),
        )),
        _ => None,
    };
}

fn reset_session_state(state: &mut WebSocketSessionState) {
    state.socket = None;
    state.chain = None;
    state.connected_at = None;
}

async fn connect_socket_with_cancel(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    timeout: Duration,
    cancel_token: &RunCancelToken,
) -> AxonResult<ProviderWebSocketPump> {
    tokio::select! {
        result = connect_socket(request, timeout) => result,
        () = cancel_token.cancelled() => Err(AxonError::provider_transport(
            "provider websocket streaming 已取消",
            None,
        )),
    }
}

async fn connect_socket(
    request: tokio_tungstenite::tungstenite::http::Request<()>,
    timeout: Duration,
) -> AxonResult<ProviderWebSocketPump> {
    ensure_rustls_crypto_provider();
    match tokio::time::timeout(timeout, connect_async(request)).await {
        Ok(Ok((socket, _))) => Ok(ProviderWebSocketPump::new(socket)),
        Ok(Err(TungsteniteError::Http(response))) => Err(AxonError::provider_transport(
            "provider websocket 握手失败",
            Some(response.status().as_u16()),
        )),
        Ok(Err(_)) => Err(AxonError::provider_transport(
            "provider websocket 握手失败",
            None,
        )),
        Err(_) => Err(AxonError::provider_transport(
            "provider websocket 握手超时",
            None,
        )),
    }
}

fn websocket_session(key: WebSocketSessionKey) -> Arc<WebSocketSession> {
    static SESSIONS: OnceLock<StdMutex<HashMap<WebSocketSessionKey, Arc<WebSocketSession>>>> =
        OnceLock::new();
    let sessions = SESSIONS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut sessions = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    prune_websocket_sessions(&mut sessions, now);
    if let Some(session) = sessions.get(&key) {
        session.touch(now);
        return Arc::clone(session);
    }
    if key.logical_session.is_some() {
        let prewarmed_key = WebSocketSessionKey {
            logical_session: None,
            url: key.url.clone(),
            headers: key.headers.clone(),
        };
        if let Some(session) = sessions.remove(&prewarmed_key) {
            session.touch(now);
            sessions.insert(key, Arc::clone(&session));
            return session;
        }
    }
    sessions
        .entry(key)
        .or_insert_with(|| {
            Arc::new(WebSocketSession {
                state: Mutex::new(WebSocketSessionState::default()),
                last_used_at: StdMutex::new(now),
            })
        })
        .clone()
}

fn prune_websocket_sessions(
    sessions: &mut HashMap<WebSocketSessionKey, Arc<WebSocketSession>>,
    now: Instant,
) {
    sessions.retain(|_, session| Arc::strong_count(session) > 1 || !session.idle_at(now));
    while sessions.len() >= PROVIDER_WEBSOCKET_MAX_CACHED_SESSIONS {
        let oldest_idle_key = sessions
            .iter()
            .filter(|(_, session)| Arc::strong_count(session) == 1)
            .min_by_key(|(_, session)| session.last_used_at())
            .map(|(key, _)| key.clone());
        let Some(key) = oldest_idle_key else {
            break;
        };
        sessions.remove(&key);
    }
}

fn prewarm_session_key(
    request: &tokio_tungstenite::tungstenite::http::Request<()>,
) -> WebSocketSessionKey {
    session_key(request, None)
}

fn session_key(
    request: &tokio_tungstenite::tungstenite::http::Request<()>,
    logical_session: Option<&str>,
) -> WebSocketSessionKey {
    const TRANSIENT_HANDSHAKE_HEADER: &str = "sec-websocket-key";
    let mut headers = request
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() != TRANSIENT_HANDSHAKE_HEADER)
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                value.to_str().unwrap_or_default().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    headers.sort();
    WebSocketSessionKey {
        logical_session: logical_session.map(ToOwned::to_owned),
        url: request.uri().to_string(),
        headers,
    }
}

fn build_websocket_request(
    request_plan: &ProviderStreamingHttpRequestPlan,
    websocket_url_override: Option<&str>,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, ProviderStreamingTransportOutcome> {
    let source_url = websocket_url_override.unwrap_or(&request_plan.url);
    let ws_url = websocket_url(source_url)
        .map_err(|error| transport_error(format!("provider websocket URL 无效：{error}"), None))?;
    let mut request = ws_url
        .as_str()
        .into_client_request()
        .map_err(|error| transport_error(error.to_string(), None))?;
    for header in &request_plan.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name.as_bytes()),
            HeaderValue::from_str(&header.value),
        ) {
            request.headers_mut().insert(name, value);
        }
    }
    if let Some(auth_header) = request_plan.auth_header.as_ref() {
        let api_key = resolve_auth_credential(&auth_header.credential)?;
        let name = HeaderName::from_bytes(auth_header.header_name.as_bytes())
            .map_err(|_| transport_error("provider websocket 鉴权 header 名称无效", None))?;
        let value = HeaderValue::from_str(&format!("{}{}", auth_header.value_prefix, api_key))
            .map_err(|_| transport_error("provider websocket 鉴权 header 值无效", None))?;
        request.headers_mut().insert(name, value);
    }
    Ok(request)
}

fn resolve_auth_credential(
    credential: &ProviderStreamingCredentialPlan,
) -> Result<String, ProviderStreamingTransportOutcome> {
    match credential {
        ProviderStreamingCredentialPlan::ApiKey(value) => Ok(value.clone()),
        ProviderStreamingCredentialPlan::Env { name } => provider_credential(name)
            .ok_or_else(|| transport_error(format!("API key 环境变量未设置：{name}"), None)),
    }
}

fn websocket_url(url: &str) -> Result<Url, url::ParseError> {
    let mut url = Url::parse(url)?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        _ => return Ok(url),
    };
    let _ = url.set_scheme(scheme);
    Ok(url)
}

#[cfg(test)]
mod connection_lifetime_tests {
    use super::*;

    #[test]
    fn websocket_connection_reconnects_before_upstream_connection_limit() {
        let now = Instant::now();
        let state = WebSocketSessionState {
            socket: None,
            chain: None,
            connected_at: Some(
                now.checked_sub(Duration::from_millis(
                    PROVIDER_WEBSOCKET_CONNECTION_MAX_AGE_MS,
                ))
                .unwrap_or(now),
            ),
        };

        assert!(websocket_connection_expired(&state, now));
    }

    #[test]
    fn websocket_session_cache_evicts_oldest_inactive_session_at_capacity() {
        let now = Instant::now();
        let oldest_key = WebSocketSessionKey {
            logical_session: Some("oldest".to_owned()),
            url: "wss://provider.example/v1/responses".to_owned(),
            headers: Vec::new(),
        };
        let recent = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        let oldest = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
        let mut sessions = HashMap::new();
        sessions.insert(oldest_key.clone(), cached_session(oldest));
        for index in 1..PROVIDER_WEBSOCKET_MAX_CACHED_SESSIONS {
            sessions.insert(
                WebSocketSessionKey {
                    logical_session: Some(format!("session-{index}")),
                    url: "wss://provider.example/v1/responses".to_owned(),
                    headers: Vec::new(),
                },
                cached_session(recent),
            );
        }

        prune_websocket_sessions(&mut sessions, now);

        assert_eq!(sessions.len(), PROVIDER_WEBSOCKET_MAX_CACHED_SESSIONS - 1);
        assert!(!sessions.contains_key(&oldest_key));
    }

    fn cached_session(last_used_at: Instant) -> Arc<WebSocketSession> {
        Arc::new(WebSocketSession {
            state: Mutex::new(WebSocketSessionState::default()),
            last_used_at: StdMutex::new(last_used_at),
        })
    }
}
