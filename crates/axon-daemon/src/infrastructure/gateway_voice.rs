//! `/api/v1/voice` 的 WebSocket 入口：鉴权、升级、会话循环、超时与背压判决。
//!
//! 分工：本模块只管连接与判决；帧解析在 `gateway_voice_frames`，回合状态在
//! `gateway_voice_turn`，宿主工作线程与有界上行队列在 `gateway_voice_worker`，
//! 指标在 `gateway_voice_metrics`。

use super::GatewayHttpState;
use super::gateway_http_response::{bearer, unauthorized};
use super::gateway_voice_frames::{CancelReason, Classified, StartRequest, classify};
use super::gateway_voice_metrics::VoiceMetrics;
use super::gateway_voice_ready::ready_json;
use super::gateway_voice_turn::{
    TurnPhase, VoiceTurn, final_json, submit_rejected_json, turn_json,
};
use super::gateway_voice_worker::{
    MAX_AUDIO_FRAME_BYTES, PushOutcome, VoiceOut, VoiceWorker, backpressure_json, error_json,
    state_json,
};
use crate::application::ExternalEventEnvelope;
use crate::infrastructure::gateway_auth::query_token;
use crate::infrastructure::gateway_kernel::GatewayKernelError;
use axon_domain::{AudienceContext, AudienceKind, DisclosurePolicy};
use axon_session::session_lane_id;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc as tokio_mpsc;

/// 语音与文本共用 `desktop` 渠道（INV-13：不新增渠道）。
const VOICE_CHANNEL: &str = "desktop";
/// 语音事件的存活上限：超过这个时长的转写不再进入 turn。
const VOICE_EVENT_TTL_MS: u64 = 60_000;

/// 会话超时口径；生产用 [`VoiceTimeouts::production`]，测试可注入更短的值。
#[derive(Debug, Clone, Copy)]
pub struct VoiceTimeouts {
    /// 连接后无有效帧即关闭。
    pub idle: Duration,
    /// 单次会话最长时长。
    pub session: Duration,
    /// `start` 到宿主 `warm`。
    pub warmup: Duration,
}

impl VoiceTimeouts {
    /// 合同值：30 s 空闲、120 s 会话、10 s 预热（PROTOCOL §5）。
    pub const fn production() -> Self {
        Self {
            idle: Duration::from_secs(30),
            session: Duration::from_secs(120),
            warmup: Duration::from_secs(10),
        }
    }
}

impl Default for VoiceTimeouts {
    fn default() -> Self {
        Self::production()
    }
}

/// 连续超限多久后断连（PROTOCOL §4.2 的 `backpressure` 条件）。
const BACKPRESSURE_DISCONNECT_AFTER: Duration = Duration::from_secs(10);

/// 超时判定：返回触发关闭的阶段；`idle` 为距上一帧的时长，`session` 为会话时长。
fn timeout_stage(
    idle: Duration,
    session: Option<Duration>,
    timeouts: &VoiceTimeouts,
) -> Option<&'static str> {
    if idle >= timeouts.idle {
        return Some("connect");
    }
    if session.is_some_and(|elapsed| elapsed >= timeouts.session) {
        return Some("listening");
    }
    None
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
    let timeouts = state.voice_timeouts;
    let metrics = Arc::clone(&state.voice_metrics);
    upgrade.on_upgrade(move |socket| session(socket, state, timeouts, metrics))
}

async fn session(
    socket: WebSocket,
    state: Arc<GatewayHttpState>,
    timeouts: VoiceTimeouts,
    metrics: Arc<VoiceMetrics>,
) {
    if serve_session(socket, &state, &timeouts, &metrics).await {
        metrics.session_finished();
    }
}

/// 会话主体；返回是否真的开始过会话（决定指标里的活跃数）。
async fn serve_session(
    mut socket: WebSocket,
    state: &Arc<GatewayHttpState>,
    timeouts: &VoiceTimeouts,
    metrics: &Arc<VoiceMetrics>,
) -> bool {
    send_json(&mut socket, ready_json()).await;
    let mut started = false;
    let mut request: Option<StartRequest> = None;
    let mut connection_session_id = String::new();
    let session_started = Instant::now();
    let mut worker: Option<Arc<VoiceWorker>> = None;
    let (out_tx, mut out_rx) = tokio_mpsc::unbounded_channel::<VoiceOut>();
    let mut voice = VoiceTurn::new();

    loop {
        let session = started.then(|| session_started.elapsed());
        if let Some(stage) = timeout_stage(Duration::ZERO, session, timeouts) {
            send_json(&mut socket, error_json("session_idle", stage)).await;
            close(&mut socket).await;
            break;
        }
        // 会话总时长与空闲时长取较小者：会话到点时即使一直有帧也要收线。
        let wait = match session {
            Some(elapsed) => timeouts.session.saturating_sub(elapsed).min(timeouts.idle),
            None => timeouts.idle,
        };
        if let Some(handle) = worker.as_ref()
            && handle
                .overload_for()
                .is_some_and(|elapsed| elapsed >= BACKPRESSURE_DISCONNECT_AFTER)
        {
            let (frames, bytes) = handle.dropped_totals();
            send_json(&mut socket, backpressure_json(frames, bytes)).await;
            metrics.record_error("backpressure");
            handle.shutdown();
            close(&mut socket).await;
            break;
        }
        tokio::select! {
            message = tokio::time::timeout(wait, socket.recv()) => {
                let Ok(message) = message else {
                    // 等不到帧：先看是不是会话总时长到点，否则按空闲超时收线。
                    let stage = if started && session_started.elapsed() >= timeouts.session {
                        "listening"
                    } else {
                        "connect"
                    };
                    send_json(&mut socket, error_json("session_idle", stage)).await;
                    close(&mut socket).await;
                    break;
                };
                let Some(Ok(message)) = message else { break };
                match message {
                    Message::Text(text) => match classify(started, &text) {
                        Ok(Classified::Start(request_wire)) => {
                            started = true;
                            metrics.session_started();
                            connection_session_id = new_session_id();
                            let session_id = connection_session_id.clone();
                            request = Some(request_wire);
                            match super::gateway_voice_worker::spawn_worker(
                                &session_id,
                                out_tx.clone(),
                                timeouts.warmup,
                                Arc::clone(metrics),
                            ) {
                                Ok(handle) => worker = Some(handle),
                                Err((error, ready)) => {
                                    send_json(&mut socket, error).await;
                                    if !ready {
                                        close(&mut socket).await;
                                        return true;
                                    }
                                }
                            }
                        }
                        Ok(Classified::Stop(utterance_seq)) => {
                            if let Some(handle) = worker.as_ref() {
                                handle.stop(utterance_seq);
                            }
                        }
                        Ok(Classified::Cancel(CancelReason::ClientClose)) => {
                            if let Some(handle) = worker.as_ref() {
                                handle.shutdown();
                            }
                            close(&mut socket).await;
                            break;
                        }
                        Ok(Classified::Cancel(CancelReason::UserCancel)) => {
                            if let Some(handle) = worker.as_ref() {
                                handle.cancel();
                            }
                        }
                        Ok(Classified::Ignored) => {}
                        Err(()) => {
                            reject(&mut socket, "protocol_sequence").await;
                            break;
                        }
                    },
                    Message::Binary(payload) => {
                        if !started || payload.first() != Some(&0x01) {
                            reject(&mut socket, "protocol_sequence").await;
                            break;
                        }
                        if payload.len() > MAX_AUDIO_FRAME_BYTES {
                            reject(&mut socket, "frame_oversize").await;
                            break;
                        }
                        if let Some(handle) = worker.as_ref()
                            && handle.push_audio(payload[1..].to_vec()) == PushOutcome::Queued
                        {
                            continue;
                        }
                        if let Some(handle) = worker.as_ref()
                            && handle.should_report_backpressure()
                        {
                            let (frames, bytes) = handle.dropped_totals();
                            send_json(&mut socket, backpressure_json(frames, bytes)).await;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
                }
            }
            Some(out) = out_rx.recv() => match out {
                VoiceOut::Text(text) => send_json(&mut socket, text).await,
                VoiceOut::Final {
                    utterance_seq,
                    text,
                    start_ms,
                    end_ms,
                    finalize_ms,
                } => {
                    send_json(
                        &mut socket,
                        final_json(utterance_seq, &text, start_ms, end_ms, finalize_ms),
                    )
                    .await;
                    if let Some(request) = request.as_ref() {
                        let phase_events = handle_final(
                            &mut socket,
                            state,
                            &mut voice,
                            metrics,
                            request,
                            &connection_session_id,
                            utterance_seq,
                            &text,
                        )
                        .await;
                        for event in phase_events {
                            send_json(&mut socket, event).await;
                        }
                    }
                }
            },
            else => break,
        }
    }
    if let Some(handle) = worker {
        handle.shutdown();
    }
    started
}

/// 提交定稿并为成功的一轮分配 epoch；返回还需要下发的状态帧。
async fn handle_final(
    socket: &mut WebSocket,
    state: &Arc<GatewayHttpState>,
    voice: &mut VoiceTurn,
    metrics: &Arc<VoiceMetrics>,
    request: &StartRequest,
    connection_session_id: &str,
    utterance_seq: u32,
    text: &str,
) -> Vec<String> {
    let content = text.trim();
    if content.is_empty() {
        voice.rejected();
        return vec![
            submit_rejected_json("转写文本为空"),
            state_json(voice.phase().as_str()),
        ];
    }
    send_json(socket, state_json(TurnPhase::Thinking.as_str())).await;
    let envelope = voice_envelope(request, connection_session_id, utterance_seq, content);
    let context = state.context.clone();
    let submitted =
        tokio::task::spawn_blocking(move || context.submit_external_event(envelope)).await;
    match submitted {
        Ok(Ok(receipt)) => {
            let interaction_id = receipt.interaction_id.as_str().to_owned();
            let assigned = voice.submitted(&interaction_id);
            let lane_id = lane_id_for(request);
            vec![turn_json(&interaction_id, &assigned, &lane_id, &lane_id)]
        }
        Ok(Err(error)) => {
            voice.rejected();
            metrics.record_error("submit_rejected");
            vec![
                submit_rejected_json(reject_reason(&error)),
                state_json(voice.phase().as_str()),
            ]
        }
        Err(_) => {
            voice.rejected();
            metrics.record_error("submit_rejected");
            vec![
                submit_rejected_json("提交任务未能完成"),
                state_json(voice.phase().as_str()),
            ]
        }
    }
}

/// 语音转写提交到既有外部事件端口；渠道、车道键与文本入口一致（INV-1/INV-13）。
fn voice_envelope(
    request: &StartRequest,
    connection_session_id: &str,
    utterance_seq: u32,
    content: &str,
) -> ExternalEventEnvelope {
    let now_ms = system_now_ms();
    ExternalEventEnvelope {
        source_channel: VOICE_CHANNEL.to_owned(),
        event_id: format!("voice-{connection_session_id}-{utterance_seq}"),
        workspace_id: request.workspace_id.clone(),
        observed_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(VOICE_EVENT_TTL_MS),
        content: content.to_owned(),
        title: None,
        correlation_key: None,
        reply_channel: VOICE_CHANNEL.to_owned(),
        reply_destination: request.reply_destination.clone(),
        attachments: Vec::new(),
    }
}

/// 与文本入口同一条车道：`(workspace, 私有信任域, desktop, destination)`。
fn lane_id_for(request: &StartRequest) -> String {
    let audience = AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly);
    session_lane_id(
        &request.workspace_id,
        &audience,
        VOICE_CHANNEL,
        &request.reply_destination,
    )
}

/// 网关拒绝原因的中文稳定说法；不把内部错误原文透给客户端。
fn reject_reason(error: &GatewayKernelError) -> &'static str {
    match error {
        GatewayKernelError::Ingress(_) => "事件被网关拒绝",
        GatewayKernelError::Preparation(_) => "事件预处理失败",
        GatewayKernelError::InvalidLifecycle(_) => "网关正在停止",
        GatewayKernelError::ConnectionLimitReached(_) => "网关繁忙",
        GatewayKernelError::Adapter { .. }
        | GatewayKernelError::DuplicateAdapter(_)
        | GatewayKernelError::EmptyAdapterId => "网关内部错误",
    }
}

fn new_session_id() -> String {
    format!(
        "ws-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    )
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

async fn reject(socket: &mut WebSocket, code: &str) {
    send_json(socket, error_json(code, "connect")).await;
    close(socket).await;
}

async fn send_json(socket: &mut WebSocket, payload: String) {
    let _ = socket.send(Message::Text(payload.into())).await;
}

async fn close(socket: &mut WebSocket) {
    let _ = socket.send(Message::Close(None)).await;
}

#[cfg(test)]
mod tests {
    use super::{VoiceTimeouts, timeout_stage};

    #[test]
    fn timeout_stage_covers_idle_and_session() {
        let timeouts = VoiceTimeouts::production();
        assert_eq!(
            timeout_stage(Duration::from_secs(30), None, &timeouts),
            Some("connect")
        );
        assert_eq!(
            timeout_stage(Duration::ZERO, Some(Duration::from_secs(120)), &timeouts),
            Some("listening")
        );
        assert_eq!(
            timeout_stage(
                Duration::from_secs(1),
                Some(Duration::from_secs(1)),
                &timeouts
            ),
            None
        );
    }

    #[test]
    fn production_timeouts_match_the_contract() {
        let timeouts = VoiceTimeouts::production();
        assert_eq!(timeouts.idle, std::time::Duration::from_secs(30));
        assert_eq!(timeouts.session, std::time::Duration::from_secs(120));
        assert_eq!(timeouts.warmup, std::time::Duration::from_secs(10));
    }

    #[test]
    fn injected_timeouts_are_honoured() {
        let timeouts = VoiceTimeouts {
            idle: Duration::from_millis(50),
            session: Duration::from_millis(80),
            warmup: Duration::from_millis(20),
        };
        assert_eq!(
            timeout_stage(Duration::from_millis(50), None, &timeouts),
            Some("connect")
        );
        assert_eq!(
            timeout_stage(Duration::ZERO, Some(Duration::from_millis(80)), &timeouts),
            Some("listening")
        );
    }

    use std::time::Duration;
}
