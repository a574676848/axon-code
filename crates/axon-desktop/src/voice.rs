//! 桌面端语音：音色试听与麦克风上行。
//!
//! 试听与朗读共用网关的语音接口，但试听是一次短请求，失败分类更细：
//! 504 是超时、409 是运行时或音色不可用、400 是请求体不合法，三者给用户的
//! 下一步动作完全不同，因此在宿主侧先分好类，界面只负责展示结论。
//!
//! 麦克风上行由宿主建立一条独立的语音 WebSocket 连接：浏览器无法在握手头里
//! 携带网关凭据，音频也只有宿主能按协议加上 `0x01` 帧头。界面只负责采集、
//! 合帧与按 100 ms 推给宿主，连接生命周期与协议解析都在这里。
//!
//! 试听音频只在内存里交给前端播放，不落盘；试听不写任何配置，也不影响对话。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tauri::Emitter;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::gateway;

/// 前端监听的语音事件名。
pub const VOICE_EVENT: &str = "axon://voice";
/// 建连超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// 等待服务端首个 `ready` 的超时。
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// 上行音频帧头：与 PROTOCOL 的 `0x01` 一致。
const AUDIO_FRAME_KIND: u8 = 0x01;
/// 下行对话音频帧头：与 PROTOCOL 的 `0x02`（`[0x02][u32be epoch][u32be seq][s16le]`）一致。
const AUDIO_OUT_FRAME_KIND: u8 = 0x02;
/// 下行音频帧的最小头部长度（1 字节类型 + 4 字节 epoch + 4 字节 seq）。
const AUDIO_OUT_HEADER_LEN: usize = 9;

/// 试听回执：base64 音频、MIME 与网关实际采用的音色。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicePreviewPayload {
    /// base64 编码的 WAV 字节。
    pub audio_base64: String,
    /// MIME 类型，固定 `audio/wav`。
    pub mime_type: String,
    /// 网关响应头返回的实际音色；请求跟随人格时也由它给出结论。
    pub preset_id: String,
}

/// 试听指定音色；`preset_id` 为空表示跟随人格。
///
/// 合成在阻塞池里执行：试听最长可等 10 s，占住主线程会让整个窗口卡住。
///
/// # Errors
///
/// 参数为空、网关不可达、语音运行时不可用或合成超时。
#[tauri::command]
pub async fn voice_preview(preset_id: Option<String>) -> Result<VoicePreviewPayload, String> {
    tauri::async_runtime::spawn_blocking(move || preview_blocking(preset_id))
        .await
        .map_err(|error| format!("试听任务失败：{error}"))?
}

/// 试听的同步实现；调试桥与命令通道共用这一份。
pub(crate) fn preview_blocking(preset_id: Option<String>) -> Result<VoicePreviewPayload, String> {
    use base64::Engine as _;

    let requested = preset_id.filter(|id| !id.is_empty());
    let persona = crate::ipc::persona_payload();
    let audio =
        gateway::post_voice_preview(requested.as_deref(), persona.as_ref()).map_err(|error| {
            match error {
                gateway::VoicePreviewError::Timeout => {
                    "preview_timeout: 试听超时，请重试或换一个音色".to_owned()
                }
                gateway::VoicePreviewError::Unavailable(reason) => {
                    format!("voice_unavailable: {reason}")
                }
                gateway::VoicePreviewError::Invalid(reason) => format!("preview_invalid: {reason}"),
                gateway::VoicePreviewError::Failed(reason) => reason,
            }
        })?;
    let preset_id = if audio.preset_id.is_empty() {
        requested.unwrap_or_default()
    } else {
        audio.preset_id
    };
    Ok(VoicePreviewPayload {
        audio_base64: base64::engine::general_purpose::STANDARD.encode(audio.bytes),
        mime_type: "audio/wav".to_owned(),
        preset_id,
    })
}

/// 一路语音会话：出站命令通道、连接任务与当前句序号。
struct VoiceSession {
    tx: mpsc::UnboundedSender<Message>,
    task: tauri::async_runtime::JoinHandle<()>,
    utterance_seq: Arc<AtomicU32>,
}

/// 进程内唯一的语音会话槽；同一时刻只允许一路语音。
fn session_slot() -> &'static Mutex<Option<VoiceSession>> {
    static SLOT: OnceLock<Mutex<Option<VoiceSession>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 开始一轮语音会话：建连、发送 `start` 并等首个 `ready`。
///
/// 返回时连接已就绪，前端可以开始推送音频帧；重复调用会先结束上一路。
///
/// # Errors
///
/// 缺少工作区或回复目的地、网关不可达、鉴权失败或就绪超时。
#[tauri::command]
pub async fn voice_start(
    app: tauri::AppHandle,
    workspace_id: String,
    reply_destination: String,
    tts: Option<bool>,
    submit: Option<bool>,
) -> Result<(), String> {
    let workspace_id = workspace_id.trim();
    let reply_destination = reply_destination.trim();
    if workspace_id.is_empty() {
        return Err("当前会话缺少工作区，无法开始语音输入".to_owned());
    }
    if reply_destination.is_empty() {
        return Err("当前会话缺少回复目的地，无法开始语音输入".to_owned());
    }
    stop_current_session();
    let url = gateway::voice_websocket_url()?;
    let client_session_id = format!("desktop-{}", now_nanos());
    let start = start_frame(
        workspace_id,
        reply_destination,
        &client_session_id,
        tts.unwrap_or(false),
        submit.unwrap_or(false),
    );
    let (tx, outbound) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
    let utterance_seq = Arc::new(AtomicU32::new(1));
    let task = tauri::async_runtime::spawn(run_session(
        app,
        url,
        start,
        outbound,
        ready_tx,
        Arc::clone(&utterance_seq),
    ));
    match tokio::time::timeout(READY_TIMEOUT, ready_rx).await {
        Ok(Ok(Ok(()))) => {
            *session_slot()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(VoiceSession {
                tx,
                task,
                utterance_seq,
            });
            Ok(())
        }
        Ok(Ok(Err(message))) => {
            task.abort();
            Err(message)
        }
        Ok(Err(_)) => {
            task.abort();
            Err("语音会话在就绪前结束".to_owned())
        }
        Err(_) => {
            task.abort();
            Err("语音会话就绪超时".to_owned())
        }
    }
}

/// 推入一帧 PCM：原始字节来自 Tauri 的 raw body，宿主在这里补协议帧头。
///
/// # Errors
///
/// 请求不是二进制、帧为空或当前没有语音会话。
#[tauri::command]
pub async fn voice_audio(request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("语音上行只接受二进制帧".to_owned());
    };
    if bytes.is_empty() {
        return Err("语音上行收到空帧".to_owned());
    }
    forward(Message::Binary(audio_frame(bytes).into()))
}

/// 结束当前句：会话保持，服务端回 `final` 后前端把定稿写入正文。
///
/// # Errors
///
/// 语音连接已断开。
#[tauri::command]
pub fn voice_stop() -> Result<(), String> {
    let sequence = session_slot().lock().ok().and_then(|slot| {
        slot.as_ref()
            .map(|session| session.utterance_seq.load(Ordering::Relaxed))
    });
    let Some(sequence) = sequence else {
        return Ok(());
    };
    forward(Message::Text(stop_frame(sequence).into()))
}

/// 取消当前轮或主动收线（`reason` 只允许 `"user_cancel"` 或 `"client_close"`）。
///
/// # Errors
///
/// 原因取值非法。
#[tauri::command]
pub fn voice_cancel(reason: Option<String>) -> Result<(), String> {
    let normalized = match reason.as_deref().map(str::trim) {
        None | Some("") | Some("user_cancel") => "user_cancel",
        Some("client_close") => "client_close",
        Some(other) => return Err(format!("不支持的取消原因：{other}")),
    };
    if normalized == "client_close" {
        stop_current_session();
        return Ok(());
    }
    let _ = forward(Message::Text(cancel_frame(normalized).into()));
    Ok(())
}

/// 上报该轮下行音频已播放完毕或已清空队列（与服务端 `stopAudio` 闭环）。
///
/// # Errors
///
/// 会话已关闭时静默忽略并返回成功，避免收尾竞态报错。
#[tauri::command]
pub fn voice_playback_drained(epoch: u32) -> Result<(), String> {
    if epoch == 0 {
        return Ok(());
    }
    let _ = forward(Message::Text(playback_drained_frame(epoch).into()));
    Ok(())
}

/// 结束上一路会话；没有会话时是幂等的。
fn stop_current_session() {
    let previous = session_slot()
        .lock()
        .map(|mut slot| slot.take())
        .unwrap_or(None);
    if let Some(session) = previous {
        let _ = session
            .tx
            .send(Message::Text(cancel_frame("client_close").into()));
        session.task.abort();
    }
}

/// 把一条出站消息交给连接任务。
fn forward(message: Message) -> Result<(), String> {
    let slot = session_slot()
        .lock()
        .map_err(|_| "语音会话状态不可用".to_owned())?;
    let Some(session) = slot.as_ref() else {
        return Err("语音会话尚未开始".to_owned());
    };
    session
        .tx
        .send(message)
        .map_err(|_| "语音连接已断开".to_owned())
}

/// 连接、发送首帧并持续转发事件，直到连接结束。
async fn run_session(
    app: tauri::AppHandle,
    url: String,
    start: String,
    mut outbound: mpsc::UnboundedReceiver<Message>,
    ready: oneshot::Sender<Result<(), String>>,
    utterance_seq: Arc<AtomicU32>,
) {
    let mut ready = Some(ready);
    let request = match build_request(&url) {
        Ok(request) => request,
        Err(message) => {
            fail_ready(&mut ready, message);
            return;
        }
    };
    let connected =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request)).await;
    let (socket, _) = match connected {
        Ok(Ok(pair)) => pair,
        Ok(Err(error)) => {
            fail_ready(&mut ready, describe_handshake(&error));
            return;
        }
        Err(_) => {
            fail_ready(&mut ready, "语音连接超时".to_owned());
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();
    if let Err(error) = sink.send(Message::Text(start.into())).await {
        fail_ready(&mut ready, format!("发送语音会话首帧失败：{error}"));
        return;
    }
    loop {
        tokio::select! {
            message = outbound.recv() => {
                let Some(message) = message else { break };
                if sink.send(message).await.is_err() {
                    emit(&app, &closed_frame("语音连接已断开"));
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                            continue;
                        };
                        let kind = value
                            .get("type")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        if kind == "ready" {
                            if let Some(sender) = ready.take() {
                                let _ = sender.send(Ok(()));
                            }
                            emit(&app, &value);
                            continue;
                        }
                        if kind == "error" && ready.is_some() {
                            let message = value
                                .get("message")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("语音会话启动失败")
                                .to_owned();
                            fail_ready(&mut ready, message);
                            break;
                        }
                        if kind == "final"
                            && let Some(seq) = value
                                .get("utteranceSeq")
                                .and_then(serde_json::Value::as_u64)
                        {
                            update_utterance(&utterance_seq, seq);
                        }
                        if matches!(
                            kind,
                            "state" | "partial" | "final" | "turn" | "stopAudio" | "error"
                        ) {
                            emit(&app, &value);
                        }
                        if kind == "closed" {
                            emit(&app, &value);
                            break;
                        }
                    }
                    // 下行音频与播放属于后续增量，本切片不把它转成界面事件。
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Some(frame) = decode_downlink_audio(&bytes) {
                            emit(&app, &frame);
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | None => {
                        emit(&app, &closed_frame("语音服务已关闭连接"));
                        break;
                    }
                    Some(Err(error)) => {
                        emit(&app, &closed_frame(&format!("语音连接中断：{error}")));
                        break;
                    }
                }
            }
        }
    }
    let _ = sink.close().await;
}

/// 组装带查询串凭据的握手请求。
fn build_request(
    url: &str,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    url.into_client_request()
        .map_err(|error| format!("构造语音连接请求失败：{error}"))
}

/// 把握手期错误翻译成可读结论；401/404 属于配置与版本问题，重试无益。
fn describe_handshake(error: &tokio_tungstenite::tungstenite::Error) -> String {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        return match response.status().as_u16() {
            401 | 403 => "语音连接拒绝访问：网关凭据缺失或不匹配".to_owned(),
            404 => "语音连接地址不存在，后台服务版本可能过旧".to_owned(),
            status => format!("语音连接握手失败：HTTP {status}"),
        };
    }
    format!("语音连接失败：{error}")
}

/// 把一帧事件下发给桌面窗口。
fn emit(app: &tauri::AppHandle, payload: &serde_json::Value) {
    let _ = app.emit(VOICE_EVENT, payload);
}

/// 就绪前的失败交给等待方；已经就绪后不再重复通知。
fn fail_ready(ready: &mut Option<oneshot::Sender<Result<(), String>>>, message: String) {
    if let Some(sender) = ready.take() {
        let _ = sender.send(Err(message));
    }
}

/// 记录下一句序号：服务端 `final` 的序号决定下一次 `stop` 带什么。
fn update_utterance(current: &Arc<AtomicU32>, final_seq: u64) {
    let next = u32::try_from(final_seq)
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    current.store(next, Ordering::Relaxed);
}

/// `start` 首帧：字段与 PROTOCOL 2.1 一一对应。
fn start_frame(
    workspace_id: &str,
    reply_destination: &str,
    client_session_id: &str,
    tts: bool,
    submit: bool,
) -> String {
    serde_json::json!({
        "type": "start",
        "protocolVersion": 1,
        "clientSessionId": client_session_id,
        "workspaceId": workspace_id,
        "replyDestination": reply_destination,
        "mode": "push_to_talk",
        "lang": "zh",
        "tts": tts,
        /*
         * 定稿的去向由调用方声明：输入坞麦克风是听写（false，只回文本），
         * 语音对话入口传 true，由网关提交成一轮。
         */
        "submit": submit,
    })
    .to_string()
}

/// `stop` 帧：只结束当前句，会话保持。
fn stop_frame(utterance_seq: u32) -> String {
    serde_json::json!({
        "type": "stop",
        "utteranceSeq": utterance_seq,
    })
    .to_string()
}

/// `cancel` 帧：用于打断当前说话轮次或通知服务端收线。
fn cancel_frame(reason: &str) -> String {
    serde_json::json!({
        "type": "cancel",
        "reason": reason,
    })
    .to_string()
}

/// `playbackDrained` 帧：通知服务端该轮音频已播完或清空。
fn playback_drained_frame(epoch: u32) -> String {
    serde_json::json!({
        "type": "playbackDrained",
        "epoch": epoch,
    })
    .to_string()
}

/// 解码服务端下行的 `0x02` 音频帧（`[0x02][u32be epoch][u32be seq][s16le]`）。
fn decode_downlink_audio(bytes: &[u8]) -> Option<serde_json::Value> {
    use base64::Engine as _;

    if bytes.len() <= AUDIO_OUT_HEADER_LEN || bytes.first().copied() != Some(AUDIO_OUT_FRAME_KIND) {
        return None;
    }
    let epoch = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    let seq = u32::from_be_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
    let pcm = &bytes[AUDIO_OUT_HEADER_LEN..];
    if pcm.is_empty() || !pcm.len().is_multiple_of(2) {
        return None;
    }
    Some(serde_json::json!({
        "type": "audio",
        "epoch": epoch,
        "seq": seq,
        "pcmBase64": base64::engine::general_purpose::STANDARD.encode(pcm),
    }))
}

/// 上行音频帧：协议要求首字节为 `0x01`，其后是 s16le PCM。
fn audio_frame(pcm: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(pcm.len() + 1);
    frame.push(AUDIO_FRAME_KIND);
    frame.extend_from_slice(pcm);
    frame
}

/// 连接主动收口时下发给前端的一帧，界面据此把入口复位。
fn closed_frame(reason: &str) -> serde_json::Value {
    serde_json::json!({ "type": "closed", "reason": reason })
}

/// 单调纳秒时间戳，用作客户端会话标识。
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        audio_frame, cancel_frame, decode_downlink_audio, playback_drained_frame, start_frame,
        stop_frame, update_utterance,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn start_frame_carries_the_frozen_protocol_fields() {
        let value: serde_json::Value = serde_json::from_str(&start_frame(
            "workspace-a",
            "desktop",
            "desktop-1",
            true,
            false,
        ))
        .expect("首帧应为合法 JSON");
        assert_eq!(value["type"], "start");
        assert_eq!(value["protocolVersion"], 1);
        assert_eq!(value["workspaceId"], "workspace-a");
        assert_eq!(value["replyDestination"], "desktop");
        assert_eq!(value["mode"], "push_to_talk");
        assert_eq!(value["lang"], "zh");
        assert_eq!(value["tts"], true);
        assert_eq!(value["submit"], false);

        // 对话入口显式声明提交：同一个首帧构造器覆盖两种模式。
        let dialogue: serde_json::Value = serde_json::from_str(&start_frame(
            "workspace-a",
            "desktop",
            "desktop-2",
            true,
            true,
        ))
        .expect("对话模式首帧");
        assert_eq!(dialogue["submit"], true);
    }

    #[test]
    fn stop_frame_carries_the_current_utterance_sequence() {
        let value: serde_json::Value = serde_json::from_str(&stop_frame(3)).expect("stop 帧");
        assert_eq!(value["type"], "stop");
        assert_eq!(value["utteranceSeq"], 3);
    }

    #[test]
    fn cancel_and_drained_frames_match_gateway_wire_contract() {
        let cancel: serde_json::Value =
            serde_json::from_str(&cancel_frame("user_cancel")).expect("cancel 帧");
        assert_eq!(cancel["type"], "cancel");
        assert_eq!(cancel["reason"], "user_cancel");

        let drained: serde_json::Value =
            serde_json::from_str(&playback_drained_frame(2)).expect("playbackDrained 帧");
        assert_eq!(drained["type"], "playbackDrained");
        assert_eq!(drained["epoch"], 2);
    }

    #[test]
    fn downlink_audio_frame_decodes_epoch_seq_and_pcm() {
        let mut raw = vec![0x02];
        raw.extend_from_slice(&3u32.to_be_bytes());
        raw.extend_from_slice(&7u32.to_be_bytes());
        raw.extend_from_slice(&[0x10, 0x20, 0x30, 0x40]);
        let decoded = decode_downlink_audio(&raw).expect("合法 0x02 音频帧应能解码");
        assert_eq!(decoded["type"], "audio");
        assert_eq!(decoded["epoch"], 3);
        assert_eq!(decoded["seq"], 7);
        assert_eq!(decoded["pcmBase64"], "ECAwQA==");

        // 非 0x02、空载荷或奇数长度字节一律拒绝。
        assert!(decode_downlink_audio(&[0x01, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0]).is_none());
        assert!(decode_downlink_audio(&[0x02, 0, 0, 0, 1, 0, 0, 0, 1]).is_none());
        assert!(decode_downlink_audio(&[0x02, 0, 0, 0, 1, 0, 0, 0, 1, 0x10]).is_none());
    }

    #[test]
    fn audio_frame_prefixes_the_protocol_kind() {
        let frame = audio_frame(&[1, 2, 3, 4]);
        assert_eq!(frame[0], 0x01);
        assert_eq!(&frame[1..], &[1, 2, 3, 4]);
        assert_eq!(frame.len(), 5);
    }

    #[test]
    fn final_sequence_advances_and_saturates() {
        let current = Arc::new(AtomicU32::new(1));
        update_utterance(&current, 4);
        assert_eq!(current.load(Ordering::Relaxed), 5);
        update_utterance(&current, u64::MAX);
        assert_eq!(current.load(Ordering::Relaxed), u32::MAX);
    }
}
