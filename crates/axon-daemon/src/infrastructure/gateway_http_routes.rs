//! Gateway HTTP 路由、准入与访问日志。
//!
//! HTTP 语义（路径匹配、方法、连接管理）全部交给 axum；这里只保留网关自己的
//! 准入规则与业务分派。同步业务统一在线程池执行，运行时的 IO 线程不再被
//! 慢请求占住，因此单个慢请求不会堵住其后所有请求。

use super::gateway_http_events::{GatewayRoute, normalized_event};
use super::gateway_http_response::{
    accepted, bearer, declared_length, from_extension, json_error, json_ok, media_type,
    request_head_bytes, request_target, unauthorized, unified_not_found,
};
use super::{GatewayHttpState, MAX_HEADER_BYTES, SATURATION_WAIT, SLOW_REQUEST_MS};
use crate::infrastructure::gateway_auth::query_token;
use crate::infrastructure::gateway_kernel::GatewayKernelError;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, Method, Uri};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::any;
use std::sync::Arc;
use std::time::Instant;

/// 构造网关路由。
pub(super) fn router(state: Arc<GatewayHttpState>, max_body_bytes: usize) -> Router {
    Router::new()
        .route("/gateway/v1/events", any(event_generic))
        .route("/gateway/v1/events/{provider}", any(event_envelope))
        .route("/gateway/v1/events/{provider}/raw", any(event_raw))
        .route(
            "/gateway/v1/interactions/{interaction_id}",
            any(interaction),
        )
        .route("/api/v1/voice", any(super::gateway_voice::voice_ws))
        .route(
            "/api/v1/voice/tts",
            any(super::gateway_voice_tts::voice_tts),
        )
        .route(
            "/api/v1/voice/cue",
            any(super::gateway_voice_tts::voice_cue),
        )
        .route(
            "/api/v1/voice/presets",
            any(super::gateway_voice_tts::voice_presets),
        )
        .route("/api/v1/{*rest}", any(custom))
        .fallback(unknown)
        .with_state(Arc::clone(&state))
        .layer(middleware::from_fn_with_state(state, access_log))
        .layer(DefaultBodyLimit::max(max_body_bytes))
}

/// 访问日志：状态码与耗时落在同一行，偶发卡顿事后可归因。
async fn access_log(
    State(state): State<Arc<GatewayHttpState>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let elapsed_ms = started.elapsed().as_millis();
    let status = response.status().as_u16();
    let level = if status >= 400 || elapsed_ms >= SLOW_REQUEST_MS {
        "WARNING"
    } else {
        "INFO"
    };
    state.log_access(
        level,
        &format!(
            "gateway-http method={method} path={path} status={status} elapsed_ms={elapsed_ms}"
        ),
    );
    response
}

async fn event_generic(State(state): State<Arc<GatewayHttpState>>, request: Request) -> Response {
    serve_route(state, request, GatewayRoute::Generic).await
}

async fn event_envelope(
    State(state): State<Arc<GatewayHttpState>>,
    Path(provider): Path<String>,
    request: Request,
) -> Response {
    match GatewayRoute::provider(&provider, false) {
        Some(route) => serve_route(state, request, route).await,
        None => json_error(404, "not_found"),
    }
}

async fn event_raw(
    State(state): State<Arc<GatewayHttpState>>,
    Path(provider): Path<String>,
    request: Request,
) -> Response {
    match GatewayRoute::provider(&provider, true) {
        Some(route) => serve_route(state, request, route).await,
        None => json_error(404, "not_found"),
    }
}

async fn interaction(
    State(state): State<Arc<GatewayHttpState>>,
    Path(interaction_id): Path<String>,
    request: Request,
) -> Response {
    match GatewayRoute::interaction(&interaction_id) {
        Some(route) => serve_route(state, request, route).await,
        None => json_error(404, "not_found"),
    }
}

/// 网关自身路由的统一分派：准入、方法、鉴权、报文校验，最后落到阻塞池执行业务。
async fn serve_route(
    state: Arc<GatewayHttpState>,
    request: Request,
    route: GatewayRoute,
) -> Response {
    // 连接名额先于一切：与既有实现一致，拥塞时不会先做鉴权与解析。
    let permit = match state.context.try_acquire_connection() {
        Ok(permit) => permit,
        Err(GatewayKernelError::ConnectionLimitReached(_)) => {
            return json_error(429, "gateway_busy");
        }
        Err(_) => return json_error(503, "gateway_unavailable"),
    };
    let (parts, body) = request.into_parts();
    if request_head_bytes(&parts.method, &parts.uri, &parts.headers) > MAX_HEADER_BYTES {
        return json_error(431, "headers_too_large");
    }
    // 方法口径与既有实现逐分支一致：GET 只服务交互查询，POST 只服务事件提交，
    // 命中了路由但方法不匹配时按路径族分别给 404 或 405。
    let is_event = !matches!(route, GatewayRoute::Interaction(_));
    match parts.method.as_str() {
        "GET" => {
            if is_event {
                return json_error(404, "not_found");
            }
        }
        "POST" => {
            if !is_event {
                return json_error(404, "not_found");
            }
        }
        _ => return json_error(405, "method_not_allowed"),
    }
    if let Some(response) = guard_event_request(&state, &parts.method, &parts.uri, &parts.headers) {
        return response;
    }
    let body = match axum::body::to_bytes(body, state.max_body_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => return json_error(413, "body_too_large"),
    };
    let job_state = Arc::clone(&state);
    run_blocking(&state, move || {
        // 名额持有到响应生成，连接上限覆盖整个请求生命周期。
        let _permit = permit;
        match route {
            GatewayRoute::Interaction(id) => interaction_response(&job_state, &id),
            other => submit_event(&job_state, &other, &body),
        }
    })
    .await
}

/// 事件/交互路由的鉴权与报文校验；通过时返回 None。
fn guard_event_request(
    state: &GatewayHttpState,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
) -> Option<Response> {
    if !state
        .context
        .authorize_request(bearer(headers), query_token(request_target(uri)))
    {
        return Some(json_error(401, "unauthorized"));
    }
    if method != Method::POST {
        return None;
    }
    if !media_type(headers).eq_ignore_ascii_case("application/json") {
        return Some(json_error(415, "content_type_required"));
    }
    let Some(content_length) = declared_length(headers) else {
        return Some(json_error(411, "content_length_required"));
    };
    (content_length > state.max_body_bytes).then(|| json_error(413, "body_too_large"))
}

/// 自定义扩展路由族：`/api/v1/*` 一律交给注入的处理器，未命中按统一模型 404。
async fn custom(State(state): State<Arc<GatewayHttpState>>, request: Request) -> Response {
    let permit = match state.context.try_acquire_connection() {
        Ok(permit) => permit,
        Err(GatewayKernelError::ConnectionLimitReached(_)) => {
            return json_error(429, "gateway_busy");
        }
        Err(_) => return json_error(503, "gateway_unavailable"),
    };
    let (parts, body) = request.into_parts();
    if request_head_bytes(&parts.method, &parts.uri, &parts.headers) > MAX_HEADER_BYTES {
        return json_error(431, "headers_too_large");
    }
    if !state.context.authorize_request(
        bearer(&parts.headers),
        query_token(request_target(&parts.uri)),
    ) {
        return unauthorized();
    }
    if declared_length(&parts.headers).unwrap_or(0) > state.max_body_bytes {
        return json_error(413, "body_too_large");
    }
    let Some(handler) = state.route_handler.clone() else {
        return unified_not_found();
    };
    let method = parts.method.as_str().to_owned();
    let path = parts.uri.path().to_owned();
    let query = parts.uri.query().map(str::to_owned);
    let body = match axum::body::to_bytes(body, state.max_body_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => return json_error(413, "body_too_large"),
    };
    run_blocking(&state, move || {
        let _permit = permit;
        handler
            .handle_request(&method, &path, query.as_deref(), &body)
            .map_or_else(unified_not_found, from_extension)
    })
    .await
}

/// 未命中任何路由。
///
/// `/api/v1/` 前缀内一律按自定义路由族处理（鉴权后统一 404），前缀之外按
/// 既有口径给 404 或 405。
async fn unknown(State(state): State<Arc<GatewayHttpState>>, request: Request) -> Response {
    let (parts, _body) = request.into_parts();
    if request_head_bytes(&parts.method, &parts.uri, &parts.headers) > MAX_HEADER_BYTES {
        return json_error(431, "headers_too_large");
    }
    if parts.uri.path().starts_with("/api/v1/") {
        if !state.context.authorize_request(
            bearer(&parts.headers),
            query_token(request_target(&parts.uri)),
        ) {
            return unauthorized();
        }
        return unified_not_found();
    }
    match parts.method.as_str() {
        "GET" | "POST" => json_error(404, "not_found"),
        _ => json_error(405, "method_not_allowed"),
    }
}

/// 交互结果查询。
fn interaction_response(state: &GatewayHttpState, interaction_id: &str) -> Response {
    let Ok(id) = axon_domain::InteractionId::new(interaction_id) else {
        return json_error(400, "invalid_interaction_id");
    };
    match state.context.get_interaction_response(&id) {
        Ok(Some(response)) => json_ok(&response),
        Ok(None) => json_error(404, "interaction_not_found"),
        Err(_) => json_error(500, "gateway_error"),
    }
}

/// 外部事件提交。
fn submit_event(state: &GatewayHttpState, route: &GatewayRoute, body: &[u8]) -> Response {
    let event = match normalized_event(route, body, &state.providers) {
        Ok(event) => event,
        Err(response) => return response,
    };
    match state.context.submit_external_event(event) {
        Ok(receipt) => accepted(&receipt),
        Err(GatewayKernelError::Ingress(_)) => json_error(403, "event_rejected"),
        Err(GatewayKernelError::Preparation(_)) => json_error(503, "event_prepare_failed"),
        Err(GatewayKernelError::InvalidLifecycle(_)) => json_error(503, "gateway_unavailable"),
        Err(GatewayKernelError::ConnectionLimitReached(_)) => json_error(429, "gateway_busy"),
        Err(
            GatewayKernelError::Adapter { .. }
            | GatewayKernelError::DuplicateAdapter(_)
            | GatewayKernelError::EmptyAdapterId,
        ) => json_error(503, "gateway_unavailable"),
    }
}

/// 在阻塞池执行同步业务；阻塞池满载时按排队预算快速失败，不无限排队。
async fn run_blocking<F>(state: &GatewayHttpState, job: F) -> Response
where
    F: FnOnce() -> Response + Send + 'static,
{
    let permit =
        match tokio::time::timeout(SATURATION_WAIT, Arc::clone(&state.blocking).acquire_owned())
            .await
        {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) => return json_error(503, "gateway_unavailable"),
            Err(_) => return json_error(429, "gateway_busy"),
        };
    let response = tokio::task::spawn_blocking(job)
        .await
        .unwrap_or_else(|_| json_error(500, "gateway_error"));
    drop(permit);
    response
}
