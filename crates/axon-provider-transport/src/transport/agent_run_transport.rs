//! Provider transport sending helpers owned by headless agent runs.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::provider_transport_retry::retry_delay;
use crate::runtime_limits::PROVIDER_WEBSOCKET_SESSION_IDLE_TIMEOUT_MS;
use crate::{
    ProviderHttpTransportInput, ProviderHttpTransportRuntime, ProviderStreamingHttpRequestPlan,
    ProviderStreamingTransportError, ProviderStreamingTransportOutcome,
    ProviderStreamingTransportRequest, ProviderStreamingWireTransport,
    ProviderWebSocketTransportInput, ProviderWebSocketTransportRuntime,
};
use axon_agent_runtime::{
    ProviderStreamAccumulator, ProviderStreamProtocol, RunCancelToken,
    provider_streaming_retry_policy,
};

#[path = "agent_run_transport_helpers.rs"]
mod helpers;
pub use helpers::agent_run_provider_transport_error;
use helpers::{
    aggregate_stream_outcome, duration_ms, observe_transport_outcome,
    provider_transport_outcome_from_axon_error, transport_result, websocket_protocol_supported,
};

const PROVIDER_RAW_REQUEST_BYTES_LIMIT: usize = 131_072;
const PROVIDER_RAW_RESPONSE_FRAME_LIMIT: usize = 128;
const PROVIDER_RAW_RESPONSE_BYTES_LIMIT: usize = 131_072;
/// 请求体超限时，中间省略标记预留的字节数上限。
///
/// 标记最长形态为「前缀 29 字节 + 十进制省略字节数 ≤ 20 字节 + 后缀 12 字节」，
/// 预留 96 字节可保证「首部 + 标记 + 尾部」严格落在 `PROVIDER_RAW_REQUEST_BYTES_LIMIT` 内。
const PROVIDER_RAW_REQUEST_ELISION_RESERVE_BYTES: usize = 96;
/// 请求体中间省略标记前缀。
const PROVIDER_RAW_REQUEST_ELISION_PREFIX: &str = "\n…[Axon 报文中间省略 ";
/// 请求体中间省略标记后缀。
const PROVIDER_RAW_REQUEST_ELISION_SUFFIX: &str = " 字节]…\n";

#[derive(Debug)]
struct ProviderRawExchangeCapture {
    protocol: ProviderStreamProtocol,
    request_url: String,
    request_body: String,
    request_truncated: bool,
    response_frames: VecDeque<String>,
    response_bytes: usize,
    response_truncated: bool,
}

impl ProviderRawExchangeCapture {
    fn new(request: &ProviderStreamingTransportRequest) -> Self {
        let (request_body, request_truncated) =
            bounded_head_and_tail(&request.request.body_text, PROVIDER_RAW_REQUEST_BYTES_LIMIT);
        Self {
            protocol: request.attempt.protocol,
            request_url: request.request.url.clone(),
            request_body,
            request_truncated,
            response_frames: VecDeque::new(),
            response_bytes: 0,
            response_truncated: false,
        }
    }

    fn observe(&mut self, frame: &str) {
        let frame = frame.trim();
        if frame.is_empty() {
            return;
        }
        let (frame, frame_truncated) = bounded_suffix(frame, PROVIDER_RAW_RESPONSE_BYTES_LIMIT);
        self.response_truncated |= frame_truncated;
        self.response_bytes = self.response_bytes.saturating_add(frame.len());
        self.response_frames.push_back(frame);
        while self.response_frames.len() > PROVIDER_RAW_RESPONSE_FRAME_LIMIT
            || self.response_bytes > PROVIDER_RAW_RESPONSE_BYTES_LIMIT
        {
            if let Some(removed) = self.response_frames.pop_front() {
                self.response_bytes = self.response_bytes.saturating_sub(removed.len());
                self.response_truncated = true;
            } else {
                break;
            }
        }
    }

    fn log_failure(
        &self,
        request: &ProviderStreamingTransportRequest,
        http_status: Option<u16>,
        error: &str,
        provider_event_type: Option<&str>,
    ) {
        axon_logger::error(&format!(
            "provider_raw_exchange_failed protocol={} turn_id={} attempt={} http_status={http_status:?} provider_event_type={provider_event_type:?} error={error:?} request_url={:?} request_truncated={} request_body={:?} response_truncated={} response_tail={:?}",
            self.protocol.as_code(),
            request.turn_id.0,
            request.attempt_index,
            self.request_url,
            self.request_truncated,
            self.request_body,
            self.response_truncated,
            self.response_frames,
        ));
    }
}

fn bounded_head_and_tail(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value.to_owned(), false);
    }
    let keep = limit.saturating_sub(PROVIDER_RAW_REQUEST_ELISION_RESERVE_BYTES);
    let head_end = floor_char_boundary(value, keep / 2);
    let tail_start = ceil_char_boundary(value, value.len() - (keep - keep / 2));
    let omitted = tail_start.saturating_sub(head_end);
    let mut captured = String::with_capacity(
        head_end + PROVIDER_RAW_REQUEST_ELISION_RESERVE_BYTES + value.len() - tail_start,
    );
    captured.push_str(&value[..head_end]);
    captured.push_str(PROVIDER_RAW_REQUEST_ELISION_PREFIX);
    captured.push_str(&omitted.to_string());
    captured.push_str(PROVIDER_RAW_REQUEST_ELISION_SUFFIX);
    captured.push_str(&value[tail_start..]);
    (captured, true)
}

/// 返回不超过 `limit` 的最大字符边界。
fn floor_char_boundary(value: &str, limit: usize) -> usize {
    value
        .char_indices()
        .take_while(|(index, character)| *index + character.len_utf8() <= limit)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0)
}

/// 返回不小于 `offset` 的最小字符边界。
fn ceil_char_boundary(value: &str, offset: usize) -> usize {
    value
        .char_indices()
        .find(|(index, _)| *index >= offset)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

fn bounded_suffix(value: &str, limit: usize) -> (String, bool) {
    if value.len() <= limit {
        return (value.to_owned(), false);
    }
    let threshold = value.len() - limit;
    let start = value
        .char_indices()
        .find(|(index, _)| *index >= threshold)
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    (value[start..].to_owned(), true)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderTransportSessionKey {
    provider_name: String,
    logical_session: String,
    endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderTransportEndpointKey(String);

const PROVIDER_WEBSOCKET_ENDPOINT_FALLBACK_COOLDOWN_MS: u64 = 300_000;

fn fallback_endpoints() -> &'static Mutex<HashMap<ProviderTransportEndpointKey, Instant>> {
    static FALLBACK_ENDPOINTS: OnceLock<Mutex<HashMap<ProviderTransportEndpointKey, Instant>>> =
        OnceLock::new();
    FALLBACK_ENDPOINTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    for suffix in ["/chat/completions", "/responses", "/messages"] {
        if let Some(base) = trimmed.strip_suffix(suffix) {
            return base.trim_end_matches('/').to_owned();
        }
    }
    trimmed.to_owned()
}

/// 记录 endpoint 级 WebSocket capability fallback，供启动预热与首个逻辑 session 共享。
pub fn mark_websocket_endpoint_fallback(endpoint: &str) {
    let endpoint = ProviderTransportEndpointKey(canonical_endpoint(endpoint));
    let now = Instant::now();
    let mut endpoints = fallback_endpoints()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    retain_active_fallback_endpoints(&mut endpoints, now);
    endpoints.insert(endpoint, now);
}

fn endpoint_has_websocket_fallback(endpoint: &str) -> bool {
    let now = Instant::now();
    let mut endpoints = fallback_endpoints()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    retain_active_fallback_endpoints(&mut endpoints, now);
    endpoints.contains_key(&ProviderTransportEndpointKey(canonical_endpoint(endpoint)))
}

fn retain_active_fallback_endpoints(
    endpoints: &mut HashMap<ProviderTransportEndpointKey, Instant>,
    now: Instant,
) {
    endpoints.retain(|_, activated_at| {
        now.saturating_duration_since(*activated_at)
            < Duration::from_millis(PROVIDER_WEBSOCKET_ENDPOINT_FALLBACK_COOLDOWN_MS)
    });
}

#[derive(Debug)]
struct ProviderTransportSessionState {
    effective_transport: ProviderStreamingWireTransport,
    websocket_failures: u32,
    retry_attempt: u64,
    retry_not_before: Option<Instant>,
    last_used_at: Instant,
}

impl ProviderTransportSessionState {
    fn touch(&mut self) {
        self.last_used_at = Instant::now();
    }

    fn is_idle(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_used_at)
            >= Duration::from_millis(PROVIDER_WEBSOCKET_SESSION_IDLE_TIMEOUT_MS)
    }
}

/// 按 provider、endpoint 与逻辑会话持久保存实际 transport 的状态。
#[derive(Debug, Clone)]
pub struct AgentRunProviderTransportSession {
    state: Arc<Mutex<ProviderTransportSessionState>>,
}

impl AgentRunProviderTransportSession {
    /// 获取或创建一个跨 turn 复用的 transport session。
    pub fn new(
        provider_name: &str,
        logical_session: &str,
        endpoint: &str,
        preferred_transport: ProviderStreamingWireTransport,
    ) -> Self {
        static SESSIONS: OnceLock<
            Mutex<HashMap<ProviderTransportSessionKey, Arc<Mutex<ProviderTransportSessionState>>>>,
        > = OnceLock::new();
        let sessions = SESSIONS.get_or_init(|| Mutex::new(HashMap::new()));
        let key = ProviderTransportSessionKey {
            provider_name: provider_name.to_owned(),
            logical_session: logical_session.to_owned(),
            endpoint: canonical_endpoint(endpoint),
        };
        let mut sessions = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        sessions.retain(|_, state| {
            Arc::strong_count(state) > 1
                || !state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .is_idle(now)
        });
        let state = sessions
            .entry(key)
            .or_insert_with(|| {
                let effective_transport: ProviderStreamingWireTransport = if preferred_transport
                    == ProviderStreamingWireTransport::WebSocket
                    && endpoint_has_websocket_fallback(endpoint)
                {
                    ProviderStreamingWireTransport::Http
                } else {
                    preferred_transport
                };
                Arc::new(Mutex::new(ProviderTransportSessionState {
                    effective_transport,
                    websocket_failures: 0,
                    retry_attempt: 0,
                    retry_not_before: None,
                    last_used_at: now,
                }))
            })
            .clone();
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .touch();
        Self { state }
    }

    /// 返回当前会话实际使用的 transport。
    pub fn effective_transport(&self) -> ProviderStreamingWireTransport {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.touch();
        state.effective_transport
    }

    fn wait_for_retry(&self, cancel_token: &RunCancelToken) -> bool {
        loop {
            if cancel_token.is_cancelled() {
                return false;
            }
            let wait = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let Some(not_before) = state.retry_not_before else {
                    return true;
                };
                let now = Instant::now();
                if now >= not_before {
                    state.retry_not_before = None;
                    return true;
                }
                not_before.saturating_duration_since(now)
            };
            if cancel_token.wait_timeout(wait) {
                return false;
            }
        }
    }

    fn record_success(&self, transport: ProviderStreamingWireTransport) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if transport == ProviderStreamingWireTransport::WebSocket {
            state.websocket_failures = 0;
        }
        state.retry_attempt = 0;
        state.retry_not_before = None;
        state.touch();
    }

    fn record_request_failure(&self, error: &ProviderStreamingTransportError) {
        if !error.failure_kind.is_retryable() {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.retry_attempt = state.retry_attempt.saturating_add(1);
        state.retry_not_before =
            Some(Instant::now() + retry_delay(state.retry_attempt, error.retry_after_ms));
        state.touch();
    }

    fn record_websocket_failure(&self, error: &ProviderStreamingTransportError) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.effective_transport != ProviderStreamingWireTransport::WebSocket {
            return false;
        }
        if error.failure_kind
            == axon_agent_runtime::ProviderStreamingTransportFailureKind::Capability
        {
            state.effective_transport = ProviderStreamingWireTransport::Http;
            state.websocket_failures = 0;
            state.retry_attempt = 0;
            state.retry_not_before = None;
            state.touch();
            return true;
        }
        if !websocket_failure_counts_toward_fallback(error.failure_kind) {
            record_retry_gate(&mut state, error);
            return false;
        }
        state.websocket_failures = state.websocket_failures.saturating_add(1);
        if state.websocket_failures
            >= provider_streaming_retry_policy().websocket_failures_before_fallback
        {
            state.effective_transport = ProviderStreamingWireTransport::Http;
            state.websocket_failures = 0;
            state.retry_attempt = 0;
            state.retry_not_before = None;
            state.touch();
            return true;
        }
        record_retry_gate(&mut state, error);
        false
    }
}

fn websocket_failure_counts_toward_fallback(
    failure_kind: axon_agent_runtime::ProviderStreamingTransportFailureKind,
) -> bool {
    matches!(
        failure_kind,
        axon_agent_runtime::ProviderStreamingTransportFailureKind::Connection
            | axon_agent_runtime::ProviderStreamingTransportFailureKind::Timeout
            | axon_agent_runtime::ProviderStreamingTransportFailureKind::Incomplete
    )
}

fn record_retry_gate(
    state: &mut ProviderTransportSessionState,
    error: &ProviderStreamingTransportError,
) {
    if error.failure_kind.is_retryable() {
        state.retry_attempt = state.retry_attempt.saturating_add(1);
        state.retry_not_before =
            Some(Instant::now() + retry_delay(state.retry_attempt, error.retry_after_ms));
    }
    state.touch();
}

/// 单次 provider 请求的实际 transport 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunProviderTransportResult {
    /// Provider runner 消费的协议结果。
    pub outcome: ProviderStreamingTransportOutcome,
    /// 本次请求最终实际使用的 transport。
    pub effective_transport: ProviderStreamingWireTransport,
    /// 本次请求是否激活了 sticky HTTP fallback。
    pub fallback_activated: bool,
    /// 从进入 retry gate 到请求结束的总耗时。
    pub elapsed_ms: u64,
    /// WebSocket 请求是否复用了既有物理连接；HTTP 请求为空。
    pub connection_reused: Option<bool>,
    /// WebSocket 请求发出前的连接年龄；HTTP 请求为空。
    pub connection_age_ms: Option<u64>,
    /// 上游错误响应携带的请求标识；成功响应未要求提供时为空。
    pub request_id: Option<String>,
}

/// Provider transport request sending input for headless agent runs.
pub struct AgentRunProviderRequestInput<'a> {
    /// Runtime-selected preferred wire transport.
    pub wire_transport: ProviderStreamingWireTransport,
    /// Logical agent session used to isolate persistent websocket chains.
    pub logical_session: &'a str,
    /// Session-scoped effective transport state.
    pub session: &'a AgentRunProviderTransportSession,
    /// Optional websocket endpoint for realtime transports.
    pub websocket_url: Option<&'a str>,
    /// 是否以 zstd 压缩 HTTP 请求体。
    pub enable_request_compression: bool,
    /// Provider streaming request planned by the turn runtime.
    pub transport_request: &'a ProviderStreamingTransportRequest,
    /// Request timeout.
    pub timeout: Duration,
    /// Optional callback for streaming provider response frames.
    pub stream_frame_callback: Option<&'a mut dyn FnMut(&str)>,
    /// Optional transport-owned heartbeat callback, independent of response frames.
    pub stream_progress_callback: Option<&'a mut dyn FnMut()>,
    /// 归一化正文投影回调；不暴露 provider raw frame。
    pub stream_projection_callback: Option<&'a mut dyn FnMut(AgentRunStreamFrame<'_>)>,
    /// 可选取消令牌，退避等待和网络请求共用。
    pub cancel_token: Option<&'a RunCancelToken>,
}

/// Provider callback 的公开元数据；正文已经由统一 accumulator 归一化。
pub struct AgentRunStreamFrame<'a> {
    /// 当前 provider attempt 编号。
    pub attempt_index: usize,
    /// 当前 frame 对应的归一化正文增量。
    pub content_deltas: &'a [axon_agent_runtime::AgentResponseStreamDelta],
}

/// Send a provider request using the session-scoped transport policy.
pub fn agent_run_send_provider_request(
    mut input: AgentRunProviderRequestInput<'_>,
) -> AgentRunProviderTransportResult {
    let started_at = Instant::now();
    let default_cancel_token = RunCancelToken::default();
    let cancel_token = input.cancel_token.unwrap_or(&default_cancel_token);
    if !input.session.wait_for_retry(cancel_token) {
        return transport_result(
            agent_run_provider_transport_error("provider streaming 已取消", None),
            input.session.effective_transport(),
            false,
            started_at,
            None,
        );
    }
    let effective_transport = input.session.effective_transport();
    let selected_transport = if input.wire_transport == ProviderStreamingWireTransport::Http
        || !websocket_protocol_supported(input.transport_request.attempt.protocol)
    {
        ProviderStreamingWireTransport::Http
    } else {
        effective_transport
    };
    if selected_transport == ProviderStreamingWireTransport::Http {
        let mut noop_progress = || {};
        let progress_callback = input
            .stream_progress_callback
            .take()
            .unwrap_or(&mut noop_progress);
        let outcome =
            send_observed_provider_http_request(&mut input, cancel_token, &mut *progress_callback);
        return transport_result(
            outcome,
            ProviderStreamingWireTransport::Http,
            false,
            started_at,
            None,
        );
    }

    let websocket_input = ProviderWebSocketTransportInput {
        request_plan: input.transport_request.request.clone(),
        timeout_ms: duration_ms(input.timeout),
        websocket_url: input.websocket_url.map(str::to_owned),
        logical_session: Some(input.logical_session.to_owned()),
    };
    let connection_telemetry =
        ProviderWebSocketTransportRuntime.connection_telemetry(&websocket_input);
    let mut accumulator = provider_stream_accumulator(input.transport_request);
    let mut raw_exchange = ProviderRawExchangeCapture::new(input.transport_request);
    let attempt_index = input.transport_request.attempt_index;
    let mut noop_progress = || {};
    let progress_callback = input
        .stream_progress_callback
        .take()
        .unwrap_or(&mut noop_progress);
    let mut consume_frame = |frame: &str| {
        raw_exchange.observe(frame);
        let deltas = accumulator.consume_projected(frame);
        if let Some(callback) = input.stream_frame_callback.as_mut() {
            callback(frame);
        }
        if let Some(callback) = input.stream_projection_callback.as_mut() {
            callback(AgentRunStreamFrame {
                attempt_index,
                content_deltas: &deltas,
            });
        }
    };
    let websocket = ProviderWebSocketTransportRuntime
        .send_with_frame_and_progress_callback(
            websocket_input,
            cancel_token,
            &mut consume_frame,
            &mut *progress_callback,
        )
        .unwrap_or_else(provider_transport_outcome_from_axon_error);
    let websocket = aggregate_stream_outcome(
        websocket,
        accumulator,
        &mut raw_exchange,
        input.transport_request,
    );
    match websocket {
        ProviderStreamingTransportOutcome::AggregatedStream { .. } => {
            input
                .session
                .record_success(ProviderStreamingWireTransport::WebSocket);
            transport_result(
                websocket,
                ProviderStreamingWireTransport::WebSocket,
                false,
                started_at,
                Some(connection_telemetry),
            )
        }
        ProviderStreamingTransportOutcome::Error(error) => {
            let fallback_activated = input.session.record_websocket_failure(&error);
            let fallback_before_stream = fallback_activated
                && error.failure_kind
                    == axon_agent_runtime::ProviderStreamingTransportFailureKind::Capability;
            if !fallback_before_stream {
                return transport_result(
                    ProviderStreamingTransportOutcome::Error(error),
                    ProviderStreamingWireTransport::WebSocket,
                    fallback_activated,
                    started_at,
                    Some(connection_telemetry),
                );
            }
            let outcome = send_observed_provider_http_request(
                &mut input,
                cancel_token,
                &mut *progress_callback,
            );
            transport_result(
                outcome,
                ProviderStreamingWireTransport::Http,
                true,
                started_at,
                Some(connection_telemetry),
            )
        }
        ProviderStreamingTransportOutcome::StreamCompleted { .. } => {
            unreachable!("transport 完成信号必须在 attempt 边界聚合")
        }
        ProviderStreamingTransportOutcome::VisibleOutput(_) => {
            unreachable!("可见输出标记只能由 CLI 投影边界附加")
        }
    }
}

fn send_provider_http_request(
    request_plan: &ProviderStreamingHttpRequestPlan,
    timeout_ms: u64,
    enable_request_compression: bool,
    line_callback: &mut dyn FnMut(&str),
    progress_callback: &mut dyn FnMut(),
    cancel_token: &RunCancelToken,
) -> ProviderStreamingTransportOutcome {
    let input = ProviderHttpTransportInput {
        request_plan: request_plan.clone(),
        timeout_ms,
        enable_request_compression,
    };
    ProviderHttpTransportRuntime
        .send_with_line_and_progress_callback(input, cancel_token, line_callback, progress_callback)
        .unwrap_or_else(provider_transport_outcome_from_axon_error)
}

fn send_observed_provider_http_request(
    input: &mut AgentRunProviderRequestInput<'_>,
    cancel_token: &RunCancelToken,
    progress_callback: &mut dyn FnMut(),
) -> ProviderStreamingTransportOutcome {
    let mut accumulator = provider_stream_accumulator(input.transport_request);
    let mut raw_exchange = ProviderRawExchangeCapture::new(input.transport_request);
    let attempt_index = input.transport_request.attempt_index;
    let mut consume_line = |line: &str| {
        raw_exchange.observe(line);
        let deltas = accumulator.consume_projected(line);
        if let Some(callback) = input.stream_frame_callback.as_mut() {
            callback(line);
        }
        if let Some(callback) = input.stream_projection_callback.as_mut() {
            callback(AgentRunStreamFrame {
                attempt_index,
                content_deltas: &deltas,
            });
        }
    };
    let outcome = send_provider_http_request(
        &input.transport_request.request,
        duration_ms(input.timeout),
        input.enable_request_compression,
        &mut consume_line,
        progress_callback,
        cancel_token,
    );
    let outcome = aggregate_stream_outcome(
        outcome,
        accumulator,
        &mut raw_exchange,
        input.transport_request,
    );
    observe_transport_outcome(
        input.session,
        ProviderStreamingWireTransport::Http,
        &outcome,
    );
    outcome
}

fn provider_stream_accumulator(
    request: &ProviderStreamingTransportRequest,
) -> ProviderStreamAccumulator {
    ProviderStreamAccumulator::new(request.attempt.protocol, request.model_tools.clone())
}

#[cfg(test)]
#[path = "agent_run_transport_tests.rs"]
mod tests;
