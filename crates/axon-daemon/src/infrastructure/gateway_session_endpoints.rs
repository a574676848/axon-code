//! 会话级固定接口：上下文快照读取与静默轮次收口。
//!
//! 两者都是桌面端的工作台契约，数据源在网关自己的 ingress（`DaemonGatewayBridge`）上，
//! 因此不经过注入的扩展路由处理器。准入、鉴权与错误模型复用路由族的既有实现：
//! 未授权是统一错误模型的 401，会话不存在是统一错误模型的 404，业务结论一律 200。
//!
//! 路由表：
//!
//! - `GET /api/v1/sessions/{sessionId}/context-snapshot`
//! - `POST /api/v1/sessions/{sessionId}/settle-silent-turn`

use std::sync::Arc;

use axon_domain::{DaemonSessionId, InteractionId};
use axum::extract::{Path, Request, State};
use axum::http::Method;
use axum::response::Response;
use serde::Deserialize;

use super::super::gateway_http_response::{
    chunked_transfer, declared_length, from_extension, json_ok, media_type, request_head_bytes,
};
use super::super::gateway_route_limits::{BodyAdmission, admit_body, body_limit};
use super::super::{GatewayHttpResponse, GatewayHttpState, MAX_HEADER_BYTES};
use super::{identify_route, json_error, run_blocking};
use crate::infrastructure::gateway_kernel::GatewayKernelError;

/// 会话不存在时的错误码：与扩展路由族共用同一套统一错误模型。
const SESSION_NOT_FOUND_CODE: &str = "SESSION_NOT_FOUND";
/// 会话不存在时的文案。
const SESSION_NOT_FOUND_MESSAGE: &str = "会话不存在";

/// 显式收口请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettleSilentTurnBody {
    /// 要收口的在途轮次。
    interaction_id: String,
}

/// `GET /api/v1/sessions/{sessionId}/context-snapshot`。
pub(super) async fn context_snapshot(
    State(state): State<Arc<GatewayHttpState>>,
    Path(session_id): Path<String>,
    request: Request,
) -> Response {
    serve(state, request, Method::GET, move |state, _body| {
        snapshot_response(state, &session_id)
    })
    .await
}

/// `POST /api/v1/sessions/{sessionId}/settle-silent-turn`。
pub(super) async fn settle_silent_turn(
    State(state): State<Arc<GatewayHttpState>>,
    Path(session_id): Path<String>,
    request: Request,
) -> Response {
    serve(state, request, Method::POST, move |state, body| {
        settle_response(state, &session_id, body)
    })
    .await
}

/// 两条路由共用的准入：连接名额、请求头上限、方法、鉴权、正文上限，最后落到阻塞池。
async fn serve<F>(
    state: Arc<GatewayHttpState>,
    request: Request,
    method: Method,
    job: F,
) -> Response
where
    F: FnOnce(&GatewayHttpState, &[u8]) -> Response + Send + 'static,
{
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
    // 命中了路径但方法不对：与既有路由族同口径，按 405 拒绝。
    if parts.method != method {
        return json_error(405, "method_not_allowed");
    }
    // 身份解析与 `/api/v1/*` 扩展路由族共用同一份规则，管理面通道限制同样生效。
    if let Err(response) = identify_route(&state, &parts.headers, &parts.uri) {
        return response;
    }
    if method == Method::POST
        && !media_type(&parts.headers).eq_ignore_ascii_case("application/json")
    {
        return json_error(415, "content_type_required");
    }
    let limit = match admit_body(
        body_limit(method.as_str(), parts.uri.path()),
        declared_length(&parts.headers),
        chunked_transfer(&parts.headers),
    ) {
        BodyAdmission::Allowed(limit) => limit,
        BodyAdmission::TooLarge => return json_error(413, "body_too_large"),
        BodyAdmission::LengthRequired => return json_error(411, "content_length_required"),
    };
    let body = match axum::body::to_bytes(body, limit).await {
        Ok(body) => body,
        Err(_) => return json_error(413, "body_too_large"),
    };
    let job_state = Arc::clone(&state);
    run_blocking(&state, move || {
        // 名额持有到响应生成：连接上限覆盖整个请求生命周期。
        let _permit = permit;
        job(&job_state, &body)
    })
    .await
}

/// 快照读取：会话存在时永远 200，`snapshot` 为空表示还没有任何可用读数。
fn snapshot_response(state: &GatewayHttpState, session_id: &str) -> Response {
    let Some(session_id) = parse_session_id(session_id) else {
        return json_error(400, "invalid_session_id");
    };
    match state.context.context_snapshot(&session_id) {
        Ok(snapshot) => json_ok(&serde_json::json!({
            "sessionId": session_id.as_str(),
            "snapshot": snapshot.as_ref().map(snapshot_json),
        })),
        Err(GatewayKernelError::NotFound(_)) => session_not_found(),
        Err(_) => json_error(500, "gateway_error"),
    }
}

/// 快照的线上形状：字段名与网关既有 camelCase 契约一致，未知字段写 `null`。
fn snapshot_json(snapshot: &axon_session::ContextSnapshot) -> serde_json::Value {
    serde_json::json!({
        "currentContextTokens": snapshot.current_context_tokens,
        "contextWindow": snapshot.context_window,
        "composition": snapshot.composition,
        "rolloverBeforeTokens": snapshot.rollover_before_tokens,
        "rolloverAfterTokens": snapshot.rollover_after_tokens,
        "updatedAtMs": snapshot.updated_at_ms,
        "source": snapshot.source,
    })
}

/// 显式收口：只收口判据成立的轮次，找不到在途轮次如实回 `settled: false`。
fn settle_response(state: &GatewayHttpState, session_id: &str, body: &[u8]) -> Response {
    let Some(session_id) = parse_session_id(session_id) else {
        return json_error(400, "invalid_session_id");
    };
    let Ok(body) = serde_json::from_slice::<SettleSilentTurnBody>(body) else {
        return json_error(400, "invalid_request_body");
    };
    let Ok(interaction_id) = InteractionId::new(body.interaction_id) else {
        return json_error(400, "invalid_interaction_id");
    };
    match state
        .context
        .settle_silent_turn(&session_id, &interaction_id)
    {
        Ok(outcome) => json_ok(&serde_json::json!({
            "settled": outcome.is_settled(),
            "verdict": outcome.verdict(),
        })),
        Err(GatewayKernelError::NotFound(_)) => session_not_found(),
        Err(_) => json_error(500, "gateway_error"),
    }
}

/// 路径里的会话标识；非法时按 400 拒绝，不把畸形标识当查询条件。
fn parse_session_id(raw: &str) -> Option<DaemonSessionId> {
    DaemonSessionId::new(raw.to_owned()).ok()
}

/// 会话不存在：沿用 `/api/v1/*` 扩展路由族的统一错误模型。
fn session_not_found() -> Response {
    from_extension(GatewayHttpResponse::error(
        404,
        SESSION_NOT_FOUND_CODE,
        SESSION_NOT_FOUND_MESSAGE,
        None,
    ))
}

#[cfg(test)]
#[path = "gateway_session_endpoints_tests.rs"]
mod tests;
