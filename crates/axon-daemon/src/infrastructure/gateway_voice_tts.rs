//! 朗读合成与音色查询：`/api/v1/voice/tts`、`/cue`、`/presets`。
//!
//! 合成前先过朗读稿渲染与人格融合，音色只覆盖声学参数；详略始终来自人格。

use super::GatewayHttpState;
use super::gateway_http_response::bearer;
use super::gateway_voice::model_root;
use crate::infrastructure::gateway_auth::query_token;
use axon_voice::expression::render_with_expression;
use axon_voice::fusion::{PersonaVoiceInput, plan as voice_plan};
use axon_voice::pitch::shift_pitch;
use axon_voice::presets;
use axon_voice::render::apply_gain;
use axon_voice::{HumorLevel, PersonaMode, Verbosity};
use axon_voice_host::inference::{Inference, encode_wav};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;
use chrono::Timelike;
use std::sync::Arc;

/// 一次合成请求的上限。
const TTS_MAX_CHARS: usize = 4_000;

/// 试听合成超时（PROTOCOL §5 的 preview_timeout）。
const PREVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
    let text = request.text;
    let preset_id = request.preset_id;
    let persona = request.persona;
    let preset_override = match preset_id.as_deref() {
        Some(id) if !id.is_empty() => match presets::resolve(id) {
            Some(preset) => Some(preset),
            None => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error":"unknown_preset"}),
                );
            }
        },
        _ => None,
    };
    if let Some(preset) = preset_override
        && !preset.available()
    {
        return json_response(
            StatusCode::CONFLICT,
            serde_json::json!({"error":"preset_unavailable","presetId":preset.id}),
        );
    }

    let outcome =
        tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, &'static str), &'static str> {
            let root = model_root();
            let runtime = Inference::discover(&root).ok_or("voice_unavailable")?;
            let verbosity = persona.map_or(Verbosity::Concise, |input| input.verbosity);
            let (spoken, style) = render_with_expression(&text, verbosity);
            if spoken.trim().is_empty() {
                return Err("empty_speech");
            }
            let hour = chrono::Local::now().hour() as u8;
            let planned = voice_plan(persona, preset_override, hour);
            let mut profile = planned.profile;
            profile.rate = (profile.rate * style.rate).clamp(0.85, 1.15);
            let mut samples = runtime
                .synthesize_with(&spoken, profile.speaker, profile.rate)
                .map_err(|_| "tts_failed")?;
            apply_gain(&mut samples, profile.energy_db);
            let samples = shift_pitch(&samples, profile.pitch_semitones);
            Ok((encode_wav(&samples), planned.preset.id))
        })
        .await;

    match outcome {
        Ok(Ok((bytes, preset_id))) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "audio/wav")
            .header("x-voice-preset", preset_id)
            .body(Body::from(bytes))
            .unwrap_or_else(|_| {
                json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error":"internal"}),
                )
            }),
        Ok(Err(error)) => json_response(
            StatusCode::CONFLICT,
            serde_json::json!({ "error": error, "message": message_for_tts(error) }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error":"internal"}),
        ),
    }
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
    let persona = value.get("persona").and_then(parse_persona_input);
    let requested = value
        .get("presetId")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .and_then(presets::resolve);
    let hour = chrono::Local::now().hour() as u8;
    let preset = voice_plan(persona, requested, hour).preset;
    if !preset.available() {
        return json_response(
            StatusCode::CONFLICT,
            serde_json::json!({"error":"preset_unavailable","presetId":preset.id}),
        );
    }
    let Some(spoken) = cue_text(preset, kind) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error":"unknown_cue"}),
        );
    };
    let text = spoken.to_owned();
    let outcome = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, &'static str> {
        let root = model_root();
        let runtime = Inference::discover(&root).ok_or("voice_unavailable")?;
        let hour = chrono::Local::now().hour() as u8;
        let profile = preset.profile_for(Verbosity::Concise, hour);
        let mut samples = runtime
            .synthesize_with(&text, profile.speaker, profile.rate)
            .map_err(|_| "tts_failed")?;
        apply_gain(&mut samples, profile.energy_db);
        let samples = shift_pitch(&samples, profile.pitch_semitones);
        Ok(encode_wav(&samples))
    })
    .await;
    match outcome {
        Ok(Ok(bytes)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "audio/wav")
            .header("x-voice-preset", preset.id)
            .body(Body::from(bytes))
            .unwrap_or_else(|_| Response::new(Body::empty())),
        Ok(Err(error)) => json_response(
            StatusCode::CONFLICT,
            serde_json::json!({ "error": error, "message": message_for_tts(error) }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error":"internal"}),
        ),
    }
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

struct TtsRequest {
    text: String,
    preset_id: Option<String>,
    persona: Option<PersonaVoiceInput>,
}

fn parse_tts_body(body: &Bytes) -> Result<TtsRequest, (StatusCode, &'static str)> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_json"))?;
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
    let persona = value.get("persona").and_then(parse_persona_input);
    Ok(TtsRequest {
        text: text.to_owned(),
        preset_id: preset,
        persona,
    })
}

fn parse_persona_input(value: &serde_json::Value) -> Option<PersonaVoiceInput> {
    let mode = match value.get("mode").and_then(|value| value.as_str()) {
        Some("focus") => PersonaMode::Focus,
        Some("incident") => PersonaMode::Incident,
        Some("group") => PersonaMode::Group,
        Some("casual") | None => PersonaMode::Casual,
        Some(_) => return None,
    };
    let warmth = value
        .get("warmth")
        .and_then(|value| value.as_u64())
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value <= 100)?;
    let formality = value
        .get("formality")
        .and_then(|value| value.as_u64())
        .and_then(|value| u8::try_from(value).ok())
        .filter(|value| *value <= 100)?;
    let humor = match value.get("humor").and_then(|value| value.as_str()) {
        Some("off") => HumorLevel::Off,
        Some("subtle") | None => HumorLevel::Subtle,
        Some("playful") => HumorLevel::Playful,
        Some(_) => return None,
    };
    let verbosity = match value.get("verbosity").and_then(|value| value.as_str()) {
        Some("minimal") => Verbosity::Minimal,
        Some("concise") | None => Verbosity::Concise,
        Some("detailed") => Verbosity::Detailed,
        Some(_) => return None,
    };
    Some(PersonaVoiceInput {
        mode,
        warmth,
        formality,
        humor,
        verbosity,
    })
}

fn message_for_tts(error: &str) -> &'static str {
    match error {
        "voice_unavailable" => "语音运行时未就绪，请先完成语音依赖安装",
        "empty_speech" => "这条回复没有可朗读的内容",
        _ => "朗读失败，请稍后重试",
    }
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
    let runtime = model_root();
    let available = Inference::discover(&runtime).is_some();
    json_response(
        StatusCode::OK,
        serde_json::json!({
            "runtimeAvailable": available,
            "defaultPreset": presets::default_preset().id,
            "presets": presets::PRESETS.len(),
            "idleTimeoutSec": 30,
            "sessionTimeoutSec": 120,
            "warmupTimeoutSec": 45,
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
    let persona = value.get("persona").and_then(parse_persona_input);
    let requested = value
        .get("presetId")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .and_then(presets::resolve);
    let hour = chrono::Local::now().hour() as u8;
    let planned = voice_plan(persona, requested, hour);
    let profile = planned.profile;
    let outcome = tokio::time::timeout(
        PREVIEW_TIMEOUT,
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, &'static str> {
            let root = model_root();
            let runtime = Inference::discover(&root).ok_or("voice_unavailable")?;
            let mut samples = runtime
                .synthesize_with("你好，这是当前音色的试听。", profile.speaker, profile.rate)
                .map_err(|_| "tts_failed")?;
            apply_gain(&mut samples, profile.energy_db);
            let samples = shift_pitch(&samples, profile.pitch_semitones);
            Ok(encode_wav(&samples))
        }),
    )
    .await;
    let outcome = match outcome {
        Ok(result) => result,
        Err(_) => {
            return json_response(
                StatusCode::GATEWAY_TIMEOUT,
                serde_json::json!({"error":"preview_timeout"}),
            );
        }
    };
    match outcome {
        Ok(Ok(bytes)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "audio/wav")
            .header("x-voice-preset", planned.preset.id)
            .body(Body::from(bytes))
            .unwrap_or_else(|_| Response::new(Body::empty())),
        Ok(Err(error)) => json_response(
            StatusCode::CONFLICT,
            serde_json::json!({ "error": error, "message": message_for_tts(error) }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error":"internal"}),
        ),
    }
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
        assert_eq!(request.text, "你好");
        assert_eq!(request.preset_id.as_deref(), Some("calm-elegant"));
    }

    #[test]
    fn tts_body_rejects_oversize() {
        use axum::body::Bytes;
        let long = "好".repeat(4_001);
        let body = format!("{{\"text\":\"{long}\"}}");
        assert!(parse_tts_body(&Bytes::from(body)).is_err());
    }

    #[test]
    fn tts_body_reads_persona() {
        use axum::body::Bytes;
        let body = Bytes::from_static(
            br#"{"text":"\\u4f60\\u597d","persona":{"mode":"group","warmth":80,"formality":70,"humor":"off","verbosity":"minimal"}}"#,
        );
        let request = parse_tts_body(&body).expect("合法请求");
        let persona = request.persona.expect("应解析人格");
        assert_eq!(persona.warmth, 80);
        assert_eq!(persona.formality, 70);
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
    #[ignore = "需要本地运行时与模型"]
    fn persona_changes_audio() {
        use axon_voice::fusion::{PersonaVoiceInput, plan};
        use axon_voice::{HumorLevel, PersonaMode};
        use axon_voice_host::inference::Inference;
        use std::path::Path;
        let root = std::env::var("AXON_RUNTIME_ROOT").expect("需要 AXON_RUNTIME_ROOT");
        let models = Path::new(&root).join("voice").join("models");
        let runtime = Inference::discover(&models).expect("运行时应可用");
        let calm = plan(
            Some(PersonaVoiceInput {
                mode: PersonaMode::Casual,
                warmth: 45,
                formality: 50,
                humor: HumorLevel::Off,
                verbosity: Verbosity::Concise,
            }),
            None,
            14,
        );
        let group = plan(
            Some(PersonaVoiceInput {
                mode: PersonaMode::Group,
                warmth: 45,
                formality: 70,
                humor: HumorLevel::Off,
                verbosity: Verbosity::Concise,
            }),
            None,
            14,
        );
        assert_ne!(calm.preset.id, group.preset.id);
        let a = runtime
            .synthesize_with("你好，我是 Axon。", calm.profile.speaker, calm.profile.rate)
            .expect("合成 A");
        let b = runtime
            .synthesize_with(
                "你好，我是 Axon。",
                group.profile.speaker,
                group.profile.rate,
            )
            .expect("合成 B");
        println!(
            "persona audio: calm={} samples preset={} group={} samples preset={}",
            a.len(),
            calm.preset.id,
            b.len(),
            group.preset.id
        );
        assert!(!a.is_empty() && !b.is_empty());
        assert_ne!(a, b);
    }

    #[test]
    fn tts_body_rejects_out_of_range_persona() {
        let body = Bytes::from_static(
            br#"{"text":"\\u4f60\\u597d","persona":{"mode":"casual","warmth":150,"formality":50}}"#,
        );
        let request = parse_tts_body(&body).expect("合法请求");
        assert!(request.persona.is_none(), "越界温度不应被当成合法人格");
    }
}
