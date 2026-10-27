//! `/api/v1/voice` 的 WebSocket 入口。
//!
//! 本阶段打通控制面：鉴权、升级、`ready`、严格客户端帧、`start` 时拉起宿主并预热、
//! 合同错误帧与协议错误断连。音频与转写链路在 Stage 6 接入，不在本模块伪造结果。

use super::GatewayHttpState;
use super::gateway_http_response::{bearer, unauthorized};
use crate::infrastructure::gateway_auth::query_token;
use axon_voice::VoiceFailure;
use axon_voice_bridge::{BridgeError, VoiceBridge};
use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// 预热超时，与 PROTOCOL §5 对齐。
const WARMUP_TIMEOUT: Duration = Duration::from_secs(10);
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

async fn session(mut socket: WebSocket) {
    send_json(&mut socket, ready_json()).await;
    let mut started = false;
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(text) => match parse_client_frame(&text) {
                Ok(ClientFrame::Start { .. }) => {
                    if started {
                        reject(&mut socket, "protocol_sequence").await;
                        return;
                    }
                    started = true;
                    let error = warmup_error(client_state()).await;
                    send_json(&mut socket, error).await;
                }
                Ok(ClientFrame::Stop { .. }) => {
                    if !started {
                        reject(&mut socket, "protocol_sequence").await;
                        return;
                    }
                }
                Ok(ClientFrame::Cancel { reason }) => {
                    if !started {
                        reject(&mut socket, "protocol_sequence").await;
                        return;
                    }
                    if reason == CancelReason::ClientClose {
                        close(&mut socket).await;
                        return;
                    }
                }
                Ok(ClientFrame::Ignored) => {}
                Err(()) => {
                    reject(&mut socket, "protocol_unknown_field").await;
                    return;
                }
            },
            Message::Binary(payload) => {
                // 音频帧必须是 0x01 开头且已 start；本阶段无推理，收下后不产生转写。
                if !started || payload.first() != Some(&0x01) {
                    reject(&mut socket, "protocol_sequence").await;
                    return;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

async fn reject(socket: &mut WebSocket, code: &str) {
    send_json(socket, error_json(code, "connect")).await;
    close(socket).await;
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

async fn warmup_error(state: ClientState) -> String {
    if !state.model_root.is_dir() {
        return error_json("model_missing", "arming");
    }
    let Some(exe) = state.host_exe.clone() else {
        return error_json("worker_unavailable", "arming");
    };
    let mut config = HostConfig::new(exe, BUILD_COMMIT.to_owned());
    config.args.push("--model-root".to_owned());
    config.args.push(state.model_root.display().to_string());
    let manifest_sha = manifest_digest(&state.model_root);
    let outcome = std::thread::spawn(move || -> Result<(), BridgeError> {
        let client = HostClient::spawn(&config).map_err(|error| {
            BridgeError::Transport(axon_voice_bridge::TransportError::Write(error.to_string()))
        })?;
        let mut bridge = VoiceBridge::new(client);
        let result = bridge.warmup("asr", DEFAULT_ASR_MODEL, &manifest_sha, WARMUP_TIMEOUT);
        bridge.shutdown();
        result
    })
    .join();
    match outcome {
        Ok(Ok(())) => error_json("internal", "arming"),
        Ok(Err(BridgeError::Host(error))) => error_json(&error.code, &error.stage),
        Ok(Err(_)) | Err(_) => error_json("worker_unavailable", "arming"),
    }
}

fn host_exe() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let dir = current.parent()?;
    let name = if cfg!(windows) {
        "axon-voice-host.exe"
    } else {
        "axon-voice-host"
    };
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
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

#[cfg(test)]
mod tests {
    use super::{CancelReason, ClientFrame, error_json, parse_client_frame, ready_json};

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
}
