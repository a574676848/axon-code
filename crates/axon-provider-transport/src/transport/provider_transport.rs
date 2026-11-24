use axon_agent_runtime::RunCancelToken;
use axon_domain::{AxonError, AxonResult};
use std::time::Duration;

use reqwest::Response;

use crate::agent_run_transport::mark_websocket_endpoint_fallback;
use crate::{
    ProviderStreamingHttpRequestPlan, ProviderStreamingTransportError,
    ProviderStreamingTransportOutcome,
};

#[path = "provider_response_failure.rs"]
mod provider_response_failure;
#[path = "provider_transport_auth.rs"]
mod provider_transport_auth;
#[path = "provider_transport_non_stream.rs"]
mod provider_transport_non_stream;
#[path = "provider_transport_pool.rs"]
mod provider_transport_pool;
#[path = "provider_transport_rate_limit.rs"]
mod provider_transport_rate_limit;
#[path = "provider_transport_request.rs"]
mod provider_transport_request;
#[path = "provider_transport_stream_response.rs"]
mod provider_transport_stream_response;
#[path = "provider_transport_websocket.rs"]
mod provider_transport_websocket;
use crate::runtime_limits::PROVIDER_HTTP_NON_STREAM_FALLBACK_TIMEOUT_MS;
use provider_transport_auth::{
    bearer_retry_request_plan, outcome_is_success, preferred_auth_request_plan,
    provider_request_id, remember_bearer_auth, should_retry_with_bearer,
};
use provider_transport_non_stream::{
    callback_non_stream_raw_lines, non_stream_conversion_error, non_stream_fallback_failed,
    non_stream_responses_body, openai_responses_non_stream_to_raw_stream,
};
#[cfg(test)]
use provider_transport_pool::shared_http_client_initialized;
use provider_transport_pool::{shared_http_client, shared_runtime};
use provider_transport_rate_limit::{
    observe_response, observe_websocket_rate_limit_event, parse_retry_after,
};
#[cfg(test)]
use provider_transport_request::PROVIDER_HTTP_ZSTD_CONTENT_ENCODING;
use provider_transport_request::{ProviderHttpRequestBody, build_prewarm_request, build_request};
#[cfg(test)]
use provider_transport_stream_response::{normalized_sse_event_data, stream_data_marks_completed};
use provider_transport_stream_response::{
    response_to_raw_stream, response_to_raw_stream_with_callback, response_to_text,
    response_to_text_with_progress,
};

/// Provider HTTP transport 已关闭远控边界。
pub const PROVIDER_HTTP_TRANSPORT_REASON_REMOTE_CLOSED: &str =
    "provider_http_transport:remote_closed";
/// Provider HTTP transport 已收到取消。
pub const PROVIDER_HTTP_TRANSPORT_REASON_CANCELLED: &str = "provider_http_transport:cancelled";
/// Provider WebSocket transport 已关闭远控边界。
pub const PROVIDER_WEBSOCKET_TRANSPORT_REASON_REMOTE_CLOSED: &str =
    "provider_websocket_transport:remote_closed";
/// Provider WebSocket transport 默认完成状态。
pub const PROVIDER_WEBSOCKET_TRANSPORT_OK_STATUS: u16 = 200;

/// Provider HTTP transport 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHttpTransportInput {
    /// 已规划的 HTTP 请求。
    pub request_plan: ProviderStreamingHttpRequestPlan,
    /// HTTP 请求超时时间。
    pub timeout_ms: u64,
    /// 是否以 zstd 压缩 HTTP 请求体。
    pub enable_request_compression: bool,
}

/// Provider WebSocket transport 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderWebSocketTransportInput {
    /// 已规划的 provider 请求。
    pub request_plan: ProviderStreamingHttpRequestPlan,
    /// WebSocket 请求超时时间。
    pub timeout_ms: u64,
    /// 可选 WebSocket URL；为空时从 request URL 推导。
    pub websocket_url: Option<String>,
    /// 真实运行时的逻辑会话；预热连接尚未绑定会话时为空。
    pub logical_session: Option<String>,
}

/// WebSocket 请求使用的连接诊断信息。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderWebSocketConnectionTelemetry {
    /// 本次请求是否复用了既有物理连接。
    pub connection_reused: bool,
    /// 复用连接在请求发出前的存活时长。
    pub connection_age_ms: Option<u64>,
}

/// 通用 provider HTTP transport runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderHttpTransportRuntime;

impl ProviderHttpTransportRuntime {
    /// 向实际 Provider endpoint 发送 HEAD，预热 DNS/TCP/TLS/HTTP 连接。
    pub fn prewarm(&self, input: &ProviderHttpTransportInput) -> AxonResult<()> {
        validate_request(&input.request_plan, input.timeout_ms, "HTTP")?;
        shared_runtime()?.block_on(prewarm_http_async(input.clone()))
    }

    /// 执行 provider HTTP 请求并返回 transport outcome。
    pub fn send(
        &self,
        input: ProviderHttpTransportInput,
        cancel_token: &RunCancelToken,
    ) -> AxonResult<ProviderStreamingTransportOutcome> {
        validate_request(&input.request_plan, input.timeout_ms, "HTTP")?;
        shared_runtime()?.block_on(send_http_async(input, cancel_token))
    }

    /// 执行 provider HTTP 请求，并在每个 SSE data 行到达时调用回调。
    pub fn send_with_line_callback<F>(
        &self,
        input: ProviderHttpTransportInput,
        cancel_token: &RunCancelToken,
        on_sse_line: F,
    ) -> AxonResult<ProviderStreamingTransportOutcome>
    where
        F: FnMut(&str),
    {
        self.send_with_line_and_progress_callback(input, cancel_token, on_sse_line, || {})
    }

    /// 执行 provider HTTP 请求，并分别回调 SSE data 行与独立进度 tick。
    pub fn send_with_line_and_progress_callback<F, G>(
        &self,
        input: ProviderHttpTransportInput,
        cancel_token: &RunCancelToken,
        on_sse_line: F,
        on_progress: G,
    ) -> AxonResult<ProviderStreamingTransportOutcome>
    where
        F: FnMut(&str),
        G: FnMut(),
    {
        validate_request(&input.request_plan, input.timeout_ms, "HTTP")?;
        shared_runtime()?.block_on(send_http_async_with_line_callback(
            input,
            on_sse_line,
            on_progress,
            cancel_token,
        ))
    }
}

/// 通用 provider WebSocket transport runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderWebSocketTransportRuntime;

impl ProviderWebSocketTransportRuntime {
    /// 建立 WebSocket 并用 response.create(generate=false) 预热 Responses 状态。
    pub fn prewarm(&self, input: &ProviderWebSocketTransportInput) -> AxonResult<()> {
        validate_request(&input.request_plan, input.timeout_ms, "WebSocket")?;
        let result = shared_runtime()?.block_on(provider_transport_websocket::prewarm_websocket(
            input.clone(),
        ));
        if matches!(
            &result,
            Err(AxonError::Provider(axon_domain::ProviderError::Transport {
                http_status: Some(426),
                ..
            }))
        ) {
            mark_websocket_endpoint_fallback(&input.request_plan.url);
        }
        result
    }

    /// 返回下一条 WebSocket 请求将使用的连接诊断信息。
    #[must_use]
    pub fn connection_telemetry(
        &self,
        input: &ProviderWebSocketTransportInput,
    ) -> ProviderWebSocketConnectionTelemetry {
        provider_transport_websocket::websocket_connection_telemetry(input)
    }

    /// 执行 WebSocket provider 请求并返回 transport outcome。
    pub fn send(
        &self,
        input: ProviderWebSocketTransportInput,
        cancel_token: &RunCancelToken,
    ) -> AxonResult<ProviderStreamingTransportOutcome> {
        validate_request(&input.request_plan, input.timeout_ms, "WebSocket")?;
        shared_runtime()?.block_on(provider_transport_websocket::send_websocket_async(
            input,
            cancel_token,
        ))
    }

    /// 执行 WebSocket provider 请求，并在每个响应文本帧到达时调用回调。
    pub fn send_with_frame_callback<F>(
        &self,
        input: ProviderWebSocketTransportInput,
        cancel_token: &RunCancelToken,
        on_frame: F,
    ) -> AxonResult<ProviderStreamingTransportOutcome>
    where
        F: FnMut(&str),
    {
        self.send_with_frame_and_progress_callback(input, cancel_token, on_frame, || {})
    }

    /// 执行 WebSocket provider 请求，并分别回调响应帧与独立进度 tick。
    pub fn send_with_frame_and_progress_callback<F, G>(
        &self,
        input: ProviderWebSocketTransportInput,
        cancel_token: &RunCancelToken,
        on_frame: F,
        on_progress: G,
    ) -> AxonResult<ProviderStreamingTransportOutcome>
    where
        F: FnMut(&str),
        G: FnMut(),
    {
        validate_request(&input.request_plan, input.timeout_ms, "WebSocket")?;
        shared_runtime()?.block_on(
            provider_transport_websocket::send_websocket_async_with_frame_and_progress_callback(
                input,
                cancel_token,
                on_frame,
                on_progress,
            ),
        )
    }
}

fn validate_request(
    request_plan: &ProviderStreamingHttpRequestPlan,
    timeout_ms: u64,
    transport_name: &str,
) -> AxonResult<()> {
    if request_plan.url.trim().is_empty() || request_plan.method != "POST" {
        return Err(AxonError::provider_failure(Box::leak(
            format!("provider {transport_name} transport 只支持非空 POST 请求").into_boxed_str(),
        )));
    }
    if request_plan.remote_request_body_allowed {
        return Err(AxonError::provider_failure(Box::leak(
            format!("provider {transport_name} transport 禁止开放远控 request body")
                .into_boxed_str(),
        )));
    }
    if timeout_ms == 0 {
        return Err(AxonError::provider_failure(Box::leak(
            format!("provider {transport_name} transport timeout 必须大于 0").into_boxed_str(),
        )));
    }
    Ok(())
}

pub(super) async fn wait_for_rate_token(
    request_plan: &ProviderStreamingHttpRequestPlan,
    cancel_token: &RunCancelToken,
) -> bool {
    provider_transport_rate_limit::wait_for_rate_token(request_plan, cancel_token).await
}

pub(super) fn observe_websocket_rate_limit(
    request_plan: &ProviderStreamingHttpRequestPlan,
    payload: &str,
) {
    observe_websocket_rate_limit_event(request_plan, payload);
}

async fn prewarm_http_async(input: ProviderHttpTransportInput) -> AxonResult<()> {
    let client = shared_http_client()?;
    let cancel_token = RunCancelToken::default();
    if !wait_for_rate_token(&input.request_plan, &cancel_token).await {
        return Ok(());
    }
    let outbound = build_prewarm_request(&client, &input.request_plan, input.timeout_ms)
        .map_err(|_| AxonError::provider_failure("provider HTTP 预热请求构造失败"))?;
    let response = outbound
        .send()
        .await
        .map_err(|_| AxonError::provider_failure("provider HTTP 预热连接失败"))?;
    response
        .bytes()
        .await
        .map_err(|_| AxonError::provider_failure("provider HTTP 预热响应读取失败"))?;
    Ok(())
}

async fn send_http_async(
    input: ProviderHttpTransportInput,
    cancel_token: &RunCancelToken,
) -> AxonResult<ProviderStreamingTransportOutcome> {
    let client = shared_http_client()?;
    let request_body = match ProviderHttpRequestBody::encode(
        &input.request_plan.body_text,
        input.enable_request_compression,
    ) {
        Ok(request_body) => request_body,
        Err(outcome) => return Ok(outcome),
    };
    if cancel_token.is_cancelled() {
        return Ok(transport_error("provider streaming 已取消", None));
    }
    if !wait_for_rate_token(&input.request_plan, cancel_token).await {
        return Ok(transport_error("provider streaming 已取消", None));
    }
    let request_plan = preferred_auth_request_plan(&input.request_plan);
    let outbound = match build_request(&client, &request_plan, &request_body, None) {
        Ok(outbound) => outbound,
        Err(outcome) => return Ok(outcome),
    };
    let response = match send_request_with_cancel(outbound, cancel_token).await {
        Ok(response) => response,
        Err(error) => {
            return fallback_to_non_stream_responses(&input, error, cancel_token).await;
        }
    };
    let status = response.status();
    observe_response(&request_plan, status, response.headers());
    if status.is_success() {
        return Ok(response_to_raw_stream(response, input.timeout_ms, cancel_token).await);
    }
    let parts = match status_response_parts(response, cancel_token).await {
        Ok(parts) => parts,
        Err(error) => return fallback_to_non_stream_responses(&input, error, cancel_token).await,
    };
    if should_retry_with_bearer(&request_plan, &parts) {
        let retry_plan = bearer_retry_request_plan(&request_plan);
        let retry_outcome = send_http_retry(
            &client,
            &retry_plan,
            &request_body,
            input.timeout_ms,
            cancel_token,
        )
        .await;
        if let Some(outcome) = retry_outcome {
            if outcome_is_success(&outcome) {
                remember_bearer_auth(&request_plan);
            }
            return Ok(outcome);
        }
    }
    let error = parts.into_error();
    fallback_to_non_stream_responses(&input, error, cancel_token).await
}

async fn send_http_async_with_line_callback<F, G>(
    input: ProviderHttpTransportInput,
    mut on_sse_line: F,
    mut on_progress: G,
    cancel_token: &RunCancelToken,
) -> AxonResult<ProviderStreamingTransportOutcome>
where
    F: FnMut(&str),
    G: FnMut(),
{
    let client = shared_http_client()?;
    let request_body = match ProviderHttpRequestBody::encode(
        &input.request_plan.body_text,
        input.enable_request_compression,
    ) {
        Ok(request_body) => request_body,
        Err(outcome) => return Ok(outcome),
    };
    if cancel_token.is_cancelled() {
        return Ok(transport_error("provider streaming 已取消", None));
    }
    if !wait_for_rate_token(&input.request_plan, cancel_token).await {
        return Ok(transport_error("provider streaming 已取消", None));
    }
    let request_plan = preferred_auth_request_plan(&input.request_plan);
    let outbound = match build_request(&client, &request_plan, &request_body, None) {
        Ok(outbound) => outbound,
        Err(outcome) => return Ok(outcome),
    };
    let response =
        match send_request_with_cancel_and_progress(outbound, cancel_token, &mut on_progress).await
        {
            Ok(response) => response,
            Err(error) => {
                return fallback_to_non_stream_responses_with_callback(
                    &input,
                    error,
                    cancel_token,
                    &mut on_sse_line,
                    &mut on_progress,
                )
                .await;
            }
        };
    let status = response.status();
    observe_response(&request_plan, status, response.headers());
    if status.is_success() {
        return Ok(response_to_raw_stream_with_callback(
            response,
            input.timeout_ms,
            &mut on_sse_line,
            &mut on_progress,
            cancel_token,
        )
        .await);
    }
    let parts = match status_response_parts(response, cancel_token).await {
        Ok(parts) => parts,
        Err(error) => {
            return fallback_to_non_stream_responses_with_callback(
                &input,
                error,
                cancel_token,
                &mut on_sse_line,
                &mut on_progress,
            )
            .await;
        }
    };
    if should_retry_with_bearer(&request_plan, &parts) {
        let retry_plan = bearer_retry_request_plan(&request_plan);
        if let Some(outcome) = send_http_retry_with_callback(
            &client,
            &retry_plan,
            &request_body,
            input.timeout_ms,
            cancel_token,
            &mut on_sse_line,
            &mut on_progress,
        )
        .await
        {
            if outcome_is_success(&outcome) {
                remember_bearer_auth(&request_plan);
            }
            return Ok(outcome);
        }
    }
    let error = parts.into_error();
    fallback_to_non_stream_responses_with_callback(
        &input,
        error,
        cancel_token,
        &mut on_sse_line,
        &mut on_progress,
    )
    .await
}

async fn send_http_retry(
    client: &reqwest::Client,
    request_plan: &ProviderStreamingHttpRequestPlan,
    request_body: &ProviderHttpRequestBody,
    timeout_ms: u64,
    cancel_token: &RunCancelToken,
) -> Option<ProviderStreamingTransportOutcome> {
    let outbound = build_request(client, request_plan, request_body, None).ok()?;
    let response = send_request_with_cancel(outbound, cancel_token)
        .await
        .ok()?;
    let status = response.status();
    observe_response(request_plan, status, response.headers());
    if status.is_success() {
        Some(response_to_raw_stream(response, timeout_ms, cancel_token).await)
    } else {
        Some(status_response_error(response, cancel_token).await)
    }
}

async fn send_http_retry_with_callback<F, G>(
    client: &reqwest::Client,
    request_plan: &ProviderStreamingHttpRequestPlan,
    request_body: &ProviderHttpRequestBody,
    timeout_ms: u64,
    cancel_token: &RunCancelToken,
    on_sse_line: &mut F,
    on_progress: &mut G,
) -> Option<ProviderStreamingTransportOutcome>
where
    F: FnMut(&str),
    G: FnMut(),
{
    let outbound = build_request(client, request_plan, request_body, None).ok()?;
    let response = send_request_with_cancel_and_progress(outbound, cancel_token, on_progress)
        .await
        .ok()?;
    let status = response.status();
    observe_response(request_plan, status, response.headers());
    if status.is_success() {
        Some(
            response_to_raw_stream_with_callback(
                response,
                timeout_ms,
                on_sse_line,
                on_progress,
                cancel_token,
            )
            .await,
        )
    } else {
        Some(status_response_error(response, cancel_token).await)
    }
}

async fn send_request_with_cancel(
    outbound: reqwest::RequestBuilder,
    cancel_token: &RunCancelToken,
) -> Result<Response, ProviderStreamingTransportOutcome> {
    tokio::select! {
        response = outbound.send() => response.map_err(|error| {
            transport_error(error.to_string(), error.status().map(|status| status.as_u16()))
        }),
        () = cancel_token.cancelled() => Err(transport_error("provider streaming 已取消", None)),
    }
}

async fn send_request_with_cancel_and_progress<F>(
    outbound: reqwest::RequestBuilder,
    cancel_token: &RunCancelToken,
    on_progress: &mut F,
) -> Result<Response, ProviderStreamingTransportOutcome>
where
    F: FnMut(),
{
    let send = outbound.send();
    tokio::pin!(send);
    let mut progress_sleep = Box::pin(tokio::time::sleep(Duration::from_millis(250)));
    loop {
        tokio::select! {
            response = &mut send => {
                return response.map_err(|error| {
                    transport_error(error.to_string(), error.status().map(|status| status.as_u16()))
                });
            }
            () = &mut progress_sleep => {
                on_progress();
                progress_sleep
                    .as_mut()
                    .reset(tokio::time::Instant::now() + Duration::from_millis(250));
            }
            () = cancel_token.cancelled() => {
                return Err(transport_error("provider streaming 已取消", None));
            }
        }
    }
}

async fn status_response_error(
    response: Response,
    cancel_token: &RunCancelToken,
) -> ProviderStreamingTransportOutcome {
    match status_response_parts(response, cancel_token).await {
        Ok(parts) => parts.into_error(),
        Err(error) => error,
    }
}

struct StatusResponseParts {
    status: u16,
    retry_after_ms: Option<u64>,
    request_id: Option<String>,
    message: String,
}

impl StatusResponseParts {
    fn into_error(self) -> ProviderStreamingTransportOutcome {
        ProviderStreamingTransportOutcome::Error(
            ProviderStreamingTransportError::new(self.message, Some(self.status))
                .with_retry_after_ms(self.retry_after_ms)
                .with_request_id(self.request_id),
        )
    }
}

async fn status_response_parts(
    response: Response,
    cancel_token: &RunCancelToken,
) -> Result<StatusResponseParts, ProviderStreamingTransportOutcome> {
    let status = response.status().as_u16();
    let retry_after_ms = parse_retry_after(response.headers())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    let request_id = provider_request_id(response.headers());
    let message = match response_to_text(response, cancel_token).await {
        Ok(message) if !message.trim().is_empty() => message,
        Ok(_) => "provider 返回错误状态".to_owned(),
        Err(error) => return Err(error),
    };
    Ok(StatusResponseParts {
        status,
        retry_after_ms,
        request_id,
        message,
    })
}

pub(super) fn transport_error(
    message: impl Into<String>,
    http_status: Option<u16>,
) -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
        message,
        http_status,
    ))
}

async fn fallback_to_non_stream_responses(
    input: &ProviderHttpTransportInput,
    original_error: ProviderStreamingTransportOutcome,
    cancel_token: &RunCancelToken,
) -> AxonResult<ProviderStreamingTransportOutcome> {
    fallback_to_non_stream_responses_with_callback(
        input,
        original_error,
        cancel_token,
        &mut |_| {},
        &mut || {},
    )
    .await
}

fn non_stream_fallback_timeout_ms(timeout_ms: u64) -> u64 {
    timeout_ms.min(PROVIDER_HTTP_NON_STREAM_FALLBACK_TIMEOUT_MS)
}

async fn fallback_to_non_stream_responses_with_callback<F, G>(
    input: &ProviderHttpTransportInput,
    original_error: ProviderStreamingTransportOutcome,
    cancel_token: &RunCancelToken,
    on_sse_line: &mut F,
    on_progress: &mut G,
) -> AxonResult<ProviderStreamingTransportOutcome>
where
    F: FnMut(&str),
    G: FnMut(),
{
    let Some(body_text) = non_stream_responses_body(&input.request_plan) else {
        return Ok(original_error);
    };
    let fallback_client = shared_http_client()?;
    if cancel_token.is_cancelled() {
        return Ok(transport_error("provider streaming 已取消", None));
    }
    if !wait_for_rate_token(&input.request_plan, cancel_token).await {
        return Ok(transport_error("provider streaming 已取消", None));
    }
    let fallback_timeout_ms = non_stream_fallback_timeout_ms(input.timeout_ms);
    let request_body =
        match ProviderHttpRequestBody::encode(&body_text, input.enable_request_compression) {
            Ok(request_body) => request_body,
            Err(outcome) => return Ok(non_stream_fallback_failed(original_error, outcome)),
        };
    let outbound = match build_request(
        &fallback_client,
        &input.request_plan,
        &request_body,
        Some(fallback_timeout_ms),
    ) {
        Ok(outbound) => outbound,
        Err(outcome) => return Ok(non_stream_fallback_failed(original_error, outcome)),
    };
    let response =
        match send_request_with_cancel_and_progress(outbound, cancel_token, on_progress).await {
            Ok(response) => response,
            Err(error) => return Ok(non_stream_fallback_failed(original_error, error)),
        };
    let status = response.status();
    observe_response(&input.request_plan, status, response.headers());
    if !status.is_success() {
        let error = status_response_error(response, cancel_token).await;
        return Ok(non_stream_fallback_failed(original_error, error));
    }
    let status = status.as_u16();
    let body = match response_to_text_with_progress(response, cancel_token, on_progress).await {
        Ok(body) => body,
        Err(error) => return Ok(non_stream_fallback_failed(original_error, error)),
    };
    let raw_stream = match openai_responses_non_stream_to_raw_stream(&body) {
        Ok(raw_stream) => raw_stream,
        Err(error) => {
            return Ok(non_stream_fallback_failed(
                original_error,
                non_stream_conversion_error(error, status),
            ));
        }
    };
    callback_non_stream_raw_lines(&raw_stream, on_sse_line);
    Ok(ProviderStreamingTransportOutcome::StreamCompleted {
        http_status: status,
    })
}

#[cfg(test)]
#[path = "provider_transport_tests.rs"]
mod tests;
