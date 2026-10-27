//! `/api/v1/voice` 的 WebSocket 入口。
//!
//! Stage 5 只打通「鉴权、升级、握手、预热错误上报」：连接建立后先回 `ready`，
//! 客户端 `start` 时拉起宿主并预热；当前无推理运行时会回 `model_unusable`，
//! 音频与转写链路在 Stage 6 接入，不在本模块伪造结果。

use super::GatewayHttpState;
use super::gateway_http_response::{bearer, unauthorized};
use crate::infrastructure::gateway_auth::query_token;
use axon_voice_bridge::{BridgeError, VoiceBridge};
use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 预热超时。
const WARMUP_TIMEOUT: Duration = Duration::from_secs(5);
/// 默认请求的识别模型名，与合同示例一致。
const DEFAULT_ASR_MODEL: &str = "sherpa-streaming-zh-en";

/// 客户端帧（Stage 5 只接受控制帧，音频在 Stage 6 接入）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientFrame {
    /// 开始会话。
    Start,
    /// 停止当前轮。
    Stop,
    /// 取消。
    Cancel {
        /// 原因。
        reason: String,
    },
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
    upgrade.on_upgrade(move |socket| session(socket, state))
}

async fn session(mut socket: WebSocket, _state: Arc<GatewayHttpState>) {
    send_json(&mut socket, ready_json()).await;
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Text(text) => match parse_client_frame(&text) {
                Ok(ClientFrame::Start) => {
                    let error = warmup_error().await;
                    send_json(&mut socket, error).await;
                }
                Ok(ClientFrame::Stop) | Ok(ClientFrame::Cancel { .. }) => {}
                Err(()) => {
                    send_json(
                        &mut socket,
                        error_json("protocol_unknown_field", "connect", false),
                    )
                    .await;
                }
            },
            Message::Close(_) => break,
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
        }
    }
}

async fn warmup_error() -> String {
    let Some(exe) = host_exe() else {
        return error_json("worker_unavailable", "arming", true);
    };
    let config = HostConfig::new(exe, BUILD_COMMIT.to_owned());
    let root = model_root();
    let manifest_sha = manifest_digest(&root);
    let outcome = tokio::task::spawn_blocking(move || -> Result<(), BridgeError> {
        let client = HostClient::spawn(&config).map_err(|error| {
            BridgeError::Transport(axon_voice_bridge::TransportError::Write(error.to_string()))
        })?;
        let mut bridge = VoiceBridge::new(client);
        let result = bridge.warmup("asr", DEFAULT_ASR_MODEL, &manifest_sha, WARMUP_TIMEOUT);
        bridge.shutdown();
        result
    })
    .await;
    match outcome {
        Ok(Ok(())) => error_json("internal", "arming", true),
        Ok(Err(BridgeError::Host(error))) => error_json(&error.code, &error.stage, error.retryable),
        Ok(Err(_)) | Err(_) => error_json("worker_unavailable", "arming", true),
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

fn manifest_digest(root: &std::path::Path) -> String {
    let path = root
        .join("asr")
        .join(DEFAULT_ASR_MODEL)
        .join("manifest.json");
    match std::fs::read(path) {
        Ok(bytes) => hex_digest(&bytes),
        Err(_) => "0".repeat(64),
    }
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

fn ready_json() -> String {
    r#"{"type":"ready","phase":"idle","epoch":0}"#.to_owned()
}

fn error_json(code: &str, stage: &str, retryable: bool) -> String {
    let root = model_root();
    let hint = if code == "model_missing" {
        format!(
            "，安装到 {}",
            root.join("asr").join(DEFAULT_ASR_MODEL).display()
        )
    } else {
        String::new()
    };
    let message = format!("{code}（{stage}）{hint}");
    serde_json::json!({
        "type": "error",
        "code": code,
        "stage": stage,
        "retryable": retryable,
        "message": message,
    })
    .to_string()
}

async fn send_json(socket: &mut WebSocket, payload: String) {
    let _ = socket.send(Message::Text(payload.into())).await;
}

/// 解析客户端控制帧；未知字段与未知类型都拒绝。
pub(crate) fn parse_client_frame(text: &str) -> Result<ClientFrame, ()> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    let kind = object
        .get("type")
        .and_then(|value| value.as_str())
        .ok_or(())?;
    match kind {
        "start" => {
            for key in object.keys() {
                if !matches!(key.as_str(), "type" | "mode" | "lang" | "hotWords") {
                    return Err(());
                }
            }
            Ok(ClientFrame::Start)
        }
        "stop" => Ok(ClientFrame::Stop),
        "cancel" => {
            let reason = object
                .get("reason")
                .and_then(|value| value.as_str())
                .ok_or(())?;
            if !matches!(reason, "barge" | "user_cancel" | "turn_end") {
                return Err(());
            }
            Ok(ClientFrame::Cancel {
                reason: reason.to_owned(),
            })
        }
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientFrame, error_json, parse_client_frame};

    #[test]
    fn start_frame_is_accepted() {
        assert_eq!(
            parse_client_frame(r#"{"type":"start","mode":"full_duplex","lang":"zh"}"#),
            Ok(ClientFrame::Start)
        );
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert_eq!(
            parse_client_frame(r#"{"type":"start","mode":"full_duplex","extra":1}"#),
            Err(())
        );
    }

    #[test]
    fn cancel_reason_is_validated() {
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"barge"}"#),
            Ok(ClientFrame::Cancel {
                reason: "barge".to_owned()
            })
        );
        assert_eq!(
            parse_client_frame(r#"{"type":"cancel","reason":"nope"}"#),
            Err(())
        );
    }

    #[test]
    fn error_frame_has_contract_fields() {
        let json = error_json("model_unusable", "arming", false);
        let value: serde_json::Value = serde_json::from_str(&json).expect("合法 JSON");
        assert_eq!(value["type"], "error");
        assert_eq!(value["code"], "model_unusable");
        assert_eq!(value["retryable"], false);
    }
}
