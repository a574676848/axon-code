//! `/api/v1/voice` 的 WebSocket 入口。
//!
//! 本阶段打通控制面：鉴权、升级、`ready`、严格客户端帧、`start` 时拉起宿主并预热、
//! 合同错误帧与协议错误断连。音频与转写链路在 Stage 6 接入，不在本模块伪造结果。

use super::GatewayHttpState;
use super::gateway_http_response::{bearer, unauthorized};
use crate::infrastructure::gateway_auth::query_token;
use axon_voice::expression::render_with_expression;
use axon_voice::fusion::{PersonaVoiceInput, plan as voice_plan};
use axon_voice::{HumorLevel, PersonaMode};
use axon_voice::{Verbosity, VoiceFailure, presets};
use axon_voice_bridge::{BridgeError, VoiceBridge};
use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::inference::{Inference, encode_wav};
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::Response;
use chrono::Timelike;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

/// 预热超时，与 PROTOCOL §5 对齐。
const WARMUP_TIMEOUT: Duration = Duration::from_secs(45);
/// 默认请求的识别模型名，与合同示例一致。
const DEFAULT_ASR_MODEL: &str = "sherpa-streaming-zh-en";
/// 协议版本。
const PROTOCOL_VERSION: u8 = 1;

/// 取消原因，客户端只允许这两个。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelReason {
    /// 用户取消。
    UserCancel,
    /// 客户端关闭。
    ClientClose,
}

/// 客户端帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientFrame {
    /// 开始会话。
    Start {
        /// 客户端会话 id。
        client_session_id: String,
        /// 工作区 id。
        workspace_id: String,
        /// 回复目的地。
        reply_destination: String,
        /// 是否朗读文本轮。
        tts: bool,
    },
    /// 结束当前句。
    Stop {
        /// 句序号。
        utterance_seq: u32,
    },
    /// 取消。
    Cancel {
        /// 原因。
        reason: CancelReason,
    },
    /// 已识别但当前不处理的合法帧。
    Ignored,
}

/// `start` 帧。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartWire {
    protocol_version: u8,
    client_session_id: String,
    workspace_id: String,
    reply_destination: String,
    mode: String,
    lang: String,
    tts: bool,
    #[serde(default)]
    device_id: Option<String>,
}

/// `stop` 帧。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopWire {
    utterance_seq: u32,
}

/// `cancel` 帧。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancelWire {
    reason: String,
}

/// WS 入口。
pub(super) async fn voice_ws(
    State(state): State<Arc<GatewayHttpState>>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !state.context.authorize_request(
        bearer(&headers),
        query_token(
            uri.path_and_query()
                .map(|value| value.as_str())
                .unwrap_or("/"),
        ),
    ) {
        return unauthorized();
    }
    upgrade.on_upgrade(session)
}

/// 文本帧分类结果。
enum Classified {
    /// 开始会话。
    Start,
    /// 结束当前句。
    Stop,
    /// 取消。
    Cancel(CancelReason),
    /// 已知但暂不处理。
    Ignored,
}

fn classify(started: bool, text: &str) -> Result<Classified, ()> {
    match parse_client_frame(text)? {
        ClientFrame::Start { .. } => {
            if started {
                Err(())
            } else {
                Ok(Classified::Start)
            }
        }
        ClientFrame::Stop { .. } => {
            if started {
                Ok(Classified::Stop)
            } else {
                Err(())
            }
        }
        ClientFrame::Cancel { reason } => {
            if started {
                Ok(Classified::Cancel(reason))
            } else {
                Err(())
            }
        }
        ClientFrame::Ignored => Ok(Classified::Ignored),
    }
}

struct ClientState {
    model_root: PathBuf,
    host_exe: Option<PathBuf>,
}

fn client_state() -> ClientState {
    ClientState {
        model_root: model_root(),
        host_exe: host_exe(),
    }
}

/// 会话开始后要处理的命令。'
enum Command {
    /// 上行音频。
    Audio(Vec<u8>),
    /// 结束当前句。
    Stop,
    /// 取消当前轮。
    Cancel,
    /// 收线。
    Shutdown,
}

struct WorkerConfig {
    host: HostConfig,
    manifest_sha: String,
    session_id: String,
}

async fn session(mut socket: WebSocket) {
    send_json(&mut socket, ready_json()).await;
    let mut started = false;
    let mut worker: Option<std_mpsc::Sender<Command>> = None;
    let (out_tx, mut out_rx) = tokio_mpsc::unbounded_channel::<String>();

    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(Ok(message)) = message else { break };
                match message {
                    Message::Text(text) => match classify(started, &text) {
                        Ok(Classified::Start) => {
                            started = true;
                            let session_id = format!("ws-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
                            match spawn_worker(&session_id, out_tx.clone()) {
                                Ok(sender) => worker = Some(sender),
                                Err((error, ready)) => {
                                    send_json(&mut socket, error).await;
                                    if !ready {
                                        close(&mut socket).await;
                                        return;
                                    }
                                }
                            }
                        }
                        Ok(Classified::Stop) => send_command(&worker, Command::Stop),
                        Ok(Classified::Cancel(CancelReason::ClientClose)) => {
                            send_command(&worker, Command::Shutdown);
                            close(&mut socket).await;
                            return;
                        }
                        Ok(Classified::Cancel(CancelReason::UserCancel)) => {
                            send_command(&worker, Command::Cancel);
                        }
                        Ok(Classified::Ignored) => {}
                        Err(()) => {
                            reject(&mut socket, "protocol_sequence").await;
                            return;
                        }
                    },
                    Message::Binary(payload) => {
                        if !started || payload.first() != Some(&0x01) {
                            reject(&mut socket, "protocol_sequence").await;
                            return;
                        }
                        if payload.len() > 8 * 1024 + 1 {
                            reject(&mut socket, "frame_oversize").await;
                            return;
                        }
                        send_command(&worker, Command::Audio(payload[1..].to_vec()));
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            Some(text) = out_rx.recv() => {
                send_json(&mut socket, text).await;
            }
            else => break,
        }
    }
    if let Some(sender) = worker {
        let _ = sender.send(Command::Shutdown);
    }
}

async fn reject(socket: &mut WebSocket, code: &str) {
    send_json(socket, error_json(code, "connect")).await;
    close(socket).await;
}

fn send_command(worker: &Option<std_mpsc::Sender<Command>>, command: Command) {
    if let Some(sender) = worker {
        let _ = sender.send(command);
    }
}

fn spawn_worker(
    session_id: &str,
    out: tokio_mpsc::UnboundedSender<String>,
) -> Result<std_mpsc::Sender<Command>, (String, bool)> {
    let state = client_state();
    if !state.model_root.is_dir() {
        return Err((error_json("model_missing", "arming"), false));
    }
    let Some(exe) = state.host_exe.clone() else {
        return Err((error_json("worker_unavailable", "arming"), false));
    };
    let mut host = HostConfig::new(exe, BUILD_COMMIT.to_owned());
    host.args.push("--model-root".to_owned());
    host.args.push(state.model_root.display().to_string());
    let config = WorkerConfig {
        host,
        manifest_sha: manifest_digest(&state.model_root),
        session_id: session_id.to_owned(),
    };
    let (tx, rx) = std_mpsc::channel::<Command>();
    std::thread::spawn(move || run_worker(config, rx, out));
    Ok(tx)
}

fn run_worker(
    config: WorkerConfig,
    rx: std_mpsc::Receiver<Command>,
    out: tokio_mpsc::UnboundedSender<String>,
) {
    let client = match HostClient::spawn(&config.host) {
        Ok(client) => client,
        Err(_) => {
            let _ = out.send(error_json("worker_unavailable", "arming"));
            return;
        }
    };
    let mut bridge = VoiceBridge::new(client);
    if let Err(error) = bridge.warmup(
        "asr",
        DEFAULT_ASR_MODEL,
        &config.manifest_sha,
        WARMUP_TIMEOUT,
    ) {
        let code = match error {
            BridgeError::Host(error) => error.code,
            _ => "worker_unavailable".to_owned(),
        };
        let _ = out.send(error_json(&code, "arming"));
        bridge.shutdown();
        return;
    }
    if bridge
        .start(&config.session_id, "full_duplex", "zh", &[])
        .is_err()
    {
        let _ = out.send(error_json("protocol_sequence", "arming"));
        bridge.shutdown();
        return;
    }
    let _ = out.send(state_json("listening"));

    loop {
        while let Ok(command) = rx.try_recv() {
            let outcome = match command {
                Command::Audio(bytes) => bridge.push_audio(&bytes),
                Command::Stop => bridge.end_session("client_close"),
                Command::Cancel => bridge.cancel("user_cancel"),
                Command::Shutdown => {
                    bridge.shutdown();
                    return;
                }
            };
            if outcome.is_err() {
                let _ = out.send(error_json("protocol_sequence", "listening"));
                bridge.shutdown();
                return;
            }
        }
        match bridge.poll(Duration::from_millis(20)) {
            Ok(Some(event)) => {
                if let Some(text) = event_json(event) {
                    let _ = out.send(text);
                }
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
    bridge.shutdown();
}

fn event_json(event: axon_voice_bridge::VoiceEvent) -> Option<String> {
    use axon_voice_bridge::VoiceEvent;
    match event {
        VoiceEvent::Partial(partial) => Some(
            serde_json::json!({
                "type": "partial",
                "utteranceSeq": partial.utterance_seq,
                "text": partial.text,
            })
            .to_string(),
        ),
        VoiceEvent::Final(final_message) => Some(
            serde_json::json!({
                "type": "final",
                "utteranceSeq": final_message.utterance_seq,
                "text": final_message.text,
                "startMs": final_message.start_ms,
                "endMs": final_message.end_ms,
                "finalizeMs": final_message.finalize_ms,
            })
            .to_string(),
        ),
        VoiceEvent::Error(error) => Some(
            serde_json::json!({
                "type": "error",
                "code": error.code,
                "stage": error.stage,
                "retryable": error.retryable,
                "message": message_for(&error.code),
            })
            .to_string(),
        ),
        VoiceEvent::CueStatus(_)
        | VoiceEvent::Heartbeat(_)
        | VoiceEvent::Warm(_)
        | VoiceEvent::Audio { .. }
        | VoiceEvent::CueAudio { .. }
        | VoiceEvent::Closed
        | VoiceEvent::Ignored(_) => None,
    }
}

fn state_json(phase: &str) -> String {
    serde_json::json!({ "type": "state", "phase": phase }).to_string()
}

fn message_for(code: &str) -> String {
    VoiceFailure::from_code(code)
        .map(|failure| failure.user_message())
        .unwrap_or_else(|| "出现内部错误，请重试".to_owned())
}

fn host_exe() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let dir = current.parent()?;
    let name = if cfg!(windows) {
        "axon-voice-host.exe"
    } else {
        "axon-voice-host"
    };
    // 生产是版本目录同级；开发树里测试二进制在 target/debug/deps，宿主在上一级。
    for base in [dir.to_path_buf(), dir.parent()?.to_path_buf()] {
        let candidate = base.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn model_root() -> PathBuf {
    if let Ok(root) = std::env::var("AXON_RUNTIME_ROOT") {
        return PathBuf::from(root).join("voice").join("models");
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".axon").join("voice").join("models")
}

fn manifest_digest(root: &Path) -> String {
    let path = root
        .join("asr")
        .join(DEFAULT_ASR_MODEL)
        .join("manifest.json");
    match std::fs::read(path) {
        Ok(bytes) => hex_digest(&bytes),
        Err(_) => "0".repeat(64),
    }
}

fn ready_json() -> String {
    serde_json::json!({
        "type": "ready",
        "asrModel": "unavailable",
        "ttsModel": "none",
        "sampleRate": 16_000,
        "workspaceId": "",
        "replyDestination": "",
        "sessionId": "",
        "epoch": 0,
        "phase": "idle",
        "tts": false,
        "degraded": {
            "asr": null,
            "tts": "none",
            "detail": "语音运行时尚未接入"
        },
        "cues": false
    })
    .to_string()
}

fn error_json(code: &str, stage: &str) -> String {
    let failure = VoiceFailure::from_code(code);
    let (message, retryable) = match &failure {
        Some(failure) => (failure.user_message(), failure.retryable()),
        None => ("出现内部错误，请重试".to_owned(), true),
    };
    let family = matches!(code, "model_missing" | "model_unusable").then_some("asr");
    serde_json::json!({
        "type": "error",
        "code": code,
        "stage": stage,
        "retryable": retryable,
        "message": message,
        "detail": { "family": family },
    })
    .to_string()
}

async fn send_json(socket: &mut WebSocket, payload: String) {
    let _ = socket.send(Message::Text(payload.into())).await;
}

async fn close(socket: &mut WebSocket) {
    let _ = socket.send(Message::Close(None)).await;
}

/// 解析客户端控制帧；未知字段、未知类型与非法取值都拒绝。
pub(crate) fn parse_client_frame(text: &str) -> Result<ClientFrame, ()> {
    let mut value: serde_json::Value = serde_json::from_str(text).map_err(|_| ())?;
    let kind = value
        .as_object()
        .and_then(|object| object.get("type"))
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .ok_or(())?;
    let object = value.as_object_mut().ok_or(())?;
    object.remove("type");
    match kind.as_str() {
        "start" => {
            let wire: StartWire = serde_json::from_value(value).map_err(|_| ())?;
            if wire.protocol_version != PROTOCOL_VERSION
                || wire.client_session_id.is_empty()
                || wire.client_session_id.len() > 64
                || wire.workspace_id.is_empty()
                || wire.reply_destination.is_empty()
                || !matches!(wire.mode.as_str(), "push_to_talk" | "full_duplex")
                || !matches!(wire.lang.as_str(), "zh" | "en" | "auto")
            {
                return Err(());
            }
            let _ = wire.device_id;
            Ok(ClientFrame::Start {
                client_session_id: wire.client_session_id,
                workspace_id: wire.workspace_id,
                reply_destination: wire.reply_destination,
                tts: wire.tts,
            })
        }
        "stop" => {
            let wire: StopWire = serde_json::from_value(value).map_err(|_| ())?;
            Ok(ClientFrame::Stop {
                utterance_seq: wire.utterance_seq,
            })
        }
        "cancel" => {
            let wire: CancelWire = serde_json::from_value(value).map_err(|_| ())?;
            let reason = match wire.reason.as_str() {
                "user_cancel" => CancelReason::UserCancel,
                "client_close" => CancelReason::ClientClose,
                _ => return Err(()),
            };
            Ok(ClientFrame::Cancel { reason })
        }
        "setTts" | "setPreset" | "cancelPreview" | "cueEnded" | "cueAck" | "preview"
        | "playbackDrained" => Ok(ClientFrame::Ignored),
        _ => Err(()),
    }
}

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
            let (spoken, style) = render_with_expression(&text, Verbosity::Concise);
            if spoken.trim().is_empty() {
                return Err("empty_speech");
            }
            let hour = chrono::Local::now().hour() as u8;
            let planned = voice_plan(persona, preset_override, hour);
            let mut profile = planned.profile;
            profile.rate = (profile.rate * style.rate).clamp(0.85, 1.15);
            let samples = runtime
                .synthesize_with(&spoken, profile.speaker, profile.rate)
                .map_err(|_| "tts_failed")?;
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
    let preset = value
        .get("presetId")
        .and_then(|value| value.as_str())
        .filter(|id| !id.is_empty())
        .and_then(presets::resolve)
        .unwrap_or_else(presets::default_preset);
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
        let samples = runtime
            .synthesize_with(&text, profile.speaker, profile.rate)
            .map_err(|_| "tts_failed")?;
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
        .map(|value| value.min(100) as u8)?;
    let formality = value
        .get("formality")
        .and_then(|value| value.as_u64())
        .map(|value| value.min(100) as u8)?;
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

#[cfg(test)]
mod tests {
    use super::{
        CancelReason, Classified, ClientFrame, classify, cue_text, error_json, parse_client_frame,
        parse_tts_body, ready_json,
    };
    use axon_voice::presets;

    fn start_json() -> &'static str {
        r#"{"type":"start","protocolVersion":1,"clientSessionId":"c-1","workspaceId":"w-1","replyDestination":"desktop","mode":"full_duplex","lang":"zh","tts":false}"#
    }

    #[test]
    fn start_frame_is_accepted() {
        match parse_client_frame(start_json()) {
            Ok(ClientFrame::Start {
                client_session_id, ..
            }) => assert_eq!(client_session_id, "c-1"),
            other => panic!("期望 start，得到 {other:?}"),
        }
    }

    #[test]
    fn start_rejects_unknown_field() {
        let json = start_json().replace("\"tts\":false", "\"tts\":false,\"extra\":1");
        assert_eq!(parse_client_frame(&json), Err(()));
    }

    #[test]
    fn start_rejects_bad_version_and_mode() {
        assert_eq!(
            parse_client_frame(
                &start_json().replace("\"protocolVersion\":1", "\"protocolVersion\":2")
            ),
            Err(())
        );
        assert_eq!(
            parse_client_frame(&start_json().replace("\"full_duplex\"", "\"half_duplex\"")),
            Err(())
        );
    }

    #[test]
    fn cancel_reason_matches_contract() {
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"user_cancel"}"#),
            Ok(ClientFrame::Cancel {
                reason: CancelReason::UserCancel
            })
        );
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"client_close"}"#),
            Ok(ClientFrame::Cancel {
                reason: CancelReason::ClientClose
            })
        );
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"barge"}"#),
            Err(())
        );
    }

    #[test]
    fn stop_requires_sequence() {
        assert_eq!(
            parse_client_frame(r#"{"type":"stop","utteranceSeq":3}"#),
            Ok(ClientFrame::Stop { utterance_seq: 3 })
        );
        assert_eq!(parse_client_frame(r#"{"type":"stop"}"#), Err(()));
    }

    #[test]
    fn ready_has_contract_fields() {
        let value: serde_json::Value = serde_json::from_str(&ready_json()).expect("合法 JSON");
        for field in [
            "asrModel",
            "ttsModel",
            "sampleRate",
            "workspaceId",
            "replyDestination",
            "sessionId",
            "epoch",
            "phase",
            "tts",
            "degraded",
            "cues",
        ] {
            assert!(value.get(field).is_some(), "ready 缺字段 {field}");
        }
        assert_eq!(value["sampleRate"], 16_000);
    }

    #[test]
    fn error_frame_uses_contract_message() {
        let value: serde_json::Value =
            serde_json::from_str(&error_json("model_missing", "arming")).expect("合法 JSON");
        assert_eq!(value["code"], "model_missing");
        assert_eq!(value["message"], "本地语音模型未安装，点此查看安装指引");
        assert_eq!(value["detail"]["family"], "asr");
        assert_eq!(value["retryable"], true);
    }
    #[test]
    fn classify_enforces_session_state() {
        assert!(matches!(
            classify(false, start_json()),
            Ok(Classified::Start)
        ));
        assert!(classify(true, start_json()).is_err(), "重复 start 应拒绝");
        assert!(classify(false, r#"{"type":"stop","utteranceSeq":1}"#).is_err());
        assert!(
            classify(false, r#"{"type":"cancel","reason":"user_cancel"}"#).is_err(),
            "未 start 不能 cancel"
        );
        assert!(matches!(
            classify(true, r#"{"type":"stop","utteranceSeq":1}"#),
            Ok(Classified::Stop)
        ));
        assert!(matches!(
            classify(true, r#"{"type":"cancel","reason":"user_cancel"}"#),
            Ok(Classified::Cancel(CancelReason::UserCancel))
        ));
        assert!(matches!(
            classify(true, r#"{"type":"cancel","reason":"client_close"}"#),
            Ok(Classified::Cancel(CancelReason::ClientClose))
        ));
        assert!(matches!(
            classify(true, r#"{"type":"setTts","enabled":true}"#),
            Ok(Classified::Ignored)
        ));
    }

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
}
