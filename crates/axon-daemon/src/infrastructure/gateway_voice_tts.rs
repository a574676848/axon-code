//! 朗读合成与音色查询：`/api/v1/voice/tts`、`/cue`、`/preview`、`/presets`、`/status`。
//!
//! 本机合成模型已移除：合成由云端提供方（阿里百炼、火山引擎）承担，当前尚未接入，
//! 因此三个合成接口统一回 `provider_unavailable`，音色清单与运行态照常可读。
//! 朗读稿渲染与人格融合仍是共享基础设施：流式朗读走 `gateway_voice_turn`，
//! 提供方接入后在这里补一次合成调用即可，无需改动帧与回合编排。

use super::GatewayHttpState;
use super::gateway_http_response::bearer;
use super::gateway_voice_ready::provider_ready;
use crate::infrastructure::gateway_auth::query_token;
use axon_voice::VoiceFailure;
use axon_voice::presets;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;
use std::sync::Arc;

/// 一次合成请求的上限。
const TTS_MAX_CHARS: usize = 4_000;

/// `POST /api/v1/voice/tts`：用当前音色预设合成一段朗读音频。
pub(super) async fn voice_tts(
    State(state): State<Arc<GatewayHttpState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if !state.context.authorize_request(
        bearer(&headers),
        query_token(
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
        ),
    ) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error":"unauthorized"}),
        );
    }
    let request = match parse_tts_body(&body) {
        Ok(parsed) => parsed,
        Err((status, error)) => {
            return json_response(status, serde_json::json!({"error": error}));
        }
    };
    if let Some(preset_id) = request.preset_id.as_deref()
        && !preset_id.is_empty()
        && presets::resolve(preset_id).is_none()
    {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error":"unknown_preset"}),
        );
    }
    provider_unavailable_response()
}

/// cue 种类与对应文案。
fn cue_text(preset: presets::VoicePreset, kind: &str) -> Option<&'static str> {
    match kind {
        "ack" => Some(preset.ack_text),
        "thinking" => Some(preset.thinking_text),
        "interrupted_ack" => Some("好，你说"),
        "reunion" => Some("好久不见"),
        "done" => Some("好了"),
        "error_ack" => Some("出了点问题，我再看一下"),
        _ => None,
    }
}

/// `POST /api/v1/voice/cue`：用角色口癖合成一条短反馈。
pub(super) async fn voice_cue(
    State(state): State<Arc<GatewayHttpState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if !state.context.authorize_request(
        bearer(&headers),
        query_token(
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
        ),
    ) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error":"unauthorized"}),
        );
    }
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error":"invalid_json"}),
            );
        }
    };
    let kind = value
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let requested = value
        .get("presetId")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .and_then(presets::resolve);
    let preset = requested.unwrap_or_else(presets::default_preset);
    if cue_text(preset, kind).is_none() {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error":"unknown_cue"}),
        );
    }
    provider_unavailable_response()
}

/// `GET /api/v1/voice/presets`：给设置页列出音色与可用性。
pub(super) async fn voice_presets(
    State(state): State<Arc<GatewayHttpState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if !state.context.authorize_request(
        bearer(&headers),
        query_token(
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
        ),
    ) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error":"unauthorized"}),
        );
    }
    let items: Vec<serde_json::Value> = presets::PRESETS
        .iter()
        .map(|preset| {
            serde_json::json!({
                "id": preset.id,
                "name": preset.name,
                "available": preset.available(),
                "default": preset.is_default,
            })
        })
        .collect();
    json_response(StatusCode::OK, serde_json::json!({ "presets": items }))
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(value.to_string()))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// 合成提供方未接入：三个合成接口的统一回执，文案只有一处来源。
fn provider_unavailable_response() -> Response {
    json_response(
        StatusCode::CONFLICT,
        serde_json::json!({
            "error": "provider_unavailable",
            "message": VoiceFailure::ProviderUnavailable.user_message(),
        }),
    )
}

struct TtsRequest {
    /// 音色预设；空表示跟随人格。
    preset_id: Option<String>,
}

fn parse_tts_body(body: &Bytes) -> Result<TtsRequest, (StatusCode, &'static str)> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_json"))?;
    // 文本只做校验：非空且不超上限；真正送去合成的仍是客户端原文。
    let text = value
        .get("text")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "missing_text"))?;
    if text.chars().count() > TTS_MAX_CHARS {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "text_too_long"));
    }
    let preset = value
        .get("presetId")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    Ok(TtsRequest { preset_id: preset })
}

/// `GET /api/v1/voice/status`：运行时可观测面。
pub(super) async fn voice_status(
    State(state): State<Arc<GatewayHttpState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if !state.context.authorize_request(
        bearer(&headers),
        query_token(
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
        ),
    ) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error":"unauthorized"}),
        );
    }
    // 与 `ready` 帧共用同一判据，不允许「建连说可用、status 说不可用」。
    let available = provider_ready();
    let metrics = state.voice_metrics.snapshot();
    // 降级必须显式可见：提供方不可用时给出稳定原因码，不静默。
    let degraded = (!available).then_some("voice_provider_unavailable");
    json_response(
        StatusCode::OK,
        serde_json::json!({
            "runtimeAvailable": available,
            "buildVersion": env!("CARGO_PKG_VERSION"),
            "presetIds": presets::PRESETS.iter().map(|preset| preset.id).collect::<Vec<_>>(),
            "defaultPreset": presets::default_preset().id,
            "presets": presets::PRESETS.len(),
            "idleTimeoutSec": state.voice_timeouts.idle.as_secs(),
            "sessionTimeoutSec": state.voice_timeouts.session.as_secs(),
            "warmupTimeoutSec": state.voice_timeouts.warmup.as_secs(),
            "metrics": metrics,
            "degraded": degraded,
        }),
    )
}

/// `POST /api/v1/voice/preview`：用指定音色合成一句试听。
pub(super) async fn voice_preview(
    State(state): State<Arc<GatewayHttpState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response {
    if !state.context.authorize_request(
        bearer(&headers),
        query_token(
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
        ),
    ) {
        return json_response(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error":"unauthorized"}),
        );
    }
    let value: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error":"invalid_json"}),
            );
        }
    };
    if let Some(requested) = value
        .get("presetId")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        && presets::resolve(requested).is_none()
    {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error":"unknown_preset"}),
        );
    }
    provider_unavailable_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_body_requires_text() {
        use axum::body::Bytes;
        assert!(parse_tts_body(&Bytes::from_static(b"{}")).is_err());
        assert!(parse_tts_body(&Bytes::from_static(b"{\"text\":\"   \"}")).is_err());
    }

    #[test]
    fn tts_body_reads_preset() {
        use axum::body::Bytes;
        let request = parse_tts_body(&Bytes::from_static(
            b"{\"text\":\"\\u4f60\\u597d\",\"presetId\":\"calm-elegant\"}",
        ))
        .expect("合法请求");
        assert_eq!(request.preset_id.as_deref(), Some("calm-elegant"));
    }

    #[test]
    fn tts_body_accepts_plain_text() {
        use axum::body::Bytes;
        let request = parse_tts_body(&Bytes::from_static(b"{\"text\":\"\\u4f60\\u597d\"}"))
            .expect("合法请求");
        assert!(request.preset_id.is_none(), "没给音色时跟随人格");
    }

    #[test]
    fn tts_body_rejects_oversize() {
        use axum::body::Bytes;
        let long = "好".repeat(4_001);
        let body = format!("{{\"text\":\"{long}\"}}");
        assert!(parse_tts_body(&Bytes::from(body)).is_err());
    }

    #[test]
    fn cue_text_is_character_specific() {
        let calm = presets::resolve("calm-elegant").expect("存在");
        assert_eq!(cue_text(calm, "ack"), Some("嗯，我在听"));
        assert!(
            cue_text(calm, "thinking")
                .unwrap_or_default()
                .contains("想想")
        );
        assert!(cue_text(calm, "nope").is_none());
    }

    #[test]
    fn provider_unavailable_message_comes_from_the_failure_table() {
        let failure = VoiceFailure::ProviderUnavailable;
        assert_eq!(failure.code(), "provider_unavailable");
        assert!(!failure.user_message().is_ascii(), "文案必须是中文");
    }
}
