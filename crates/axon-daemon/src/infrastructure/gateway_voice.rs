//! `/api/v1/voice` 的 WebSocket 入口：鉴权、升级、会话循环、超时与背压判决。
//!
//! 分工：本模块只管连接与编排；帧解析在 `gateway_voice_frames`，回合状态在
//! `gateway_voice_turn`，事件订阅在 `gateway_voice_turn_stream`，宿主工作线程与
//! 有界上行队列在 `gateway_voice_worker`，指标在 `gateway_voice_metrics`。

use super::GatewayHttpState;
use super::gateway_http_response::{bearer, unauthorized};
use super::gateway_voice_frames::{CancelReason, Classified, StartRequest, classify};
use super::gateway_voice_metrics::VoiceMetrics;
use super::gateway_voice_out_frames::{backpressure_json, error_json, state_json};
use super::gateway_voice_ready::ready_json;
use super::gateway_voice_turn::{
    DrainedOutcome, SpeakFrame, StopAudio, TurnPhase, VoiceTurn, drain_timeout_json, final_json,
    stop_audio_json, submit_rejected_json, turn_json,
};
use super::gateway_voice_turn_stream::{TurnSignal, spawn_turn_subscription};
use super::gateway_voice_worker::{MAX_AUDIO_FRAME_BYTES, PushOutcome, VoiceOut, VoiceWorker};
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
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// `stopAudio` 到客户端 `playbackDrained` 的回执上限（PROTOCOL §5）。
    pub drained: Duration,
}

impl VoiceTimeouts {
    /// 合同值：30 s 空闲、120 s 会话、10 s 预热、10 s 停播回执（PROTOCOL §5）。
    pub const fn production() -> Self {
        Self {
            idle: Duration::from_secs(30),
            session: Duration::from_secs(120),
            warmup: Duration::from_secs(10),
            drained: Duration::from_secs(10),
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
    let (turn_tx, mut turn_rx) = tokio_mpsc::unbounded_channel::<TurnSignal>();
    let mut voice = VoiceTurn::new(timeouts.drained);
    // 当前轮的事件订阅；换轮或收线时置位停止标志。
    let mut subscription: Option<Arc<AtomicBool>> = None;

    loop {
        let session = started.then(|| session_started.elapsed());
        if let Some(stage) = timeout_stage(Duration::ZERO, session, timeouts) {
            send_json(&mut socket, error_json("session_idle", stage)).await;
            close(&mut socket).await;
            break;
        }
        // 回执超时必须在每轮循环开头判定：客户端持续发帧时不会走到 socket 超时分支。
        if voice.on_drain_timeout(Instant::now()) {
            force_drain_cleanup(&mut socket, metrics, &mut subscription).await;
            continue;
        }
        // 会话总时长、空闲时长与停播回执三者取最小者作为本轮等待上限。
        let mut wait = match session {
            Some(elapsed) => timeouts.session.saturating_sub(elapsed).min(timeouts.idle),
            None => timeouts.idle,
        };
        if let Some(deadline) = voice.drain_deadline() {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
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
                    // 停播回执先于空闲/会话超时判定：到点必须记 internal 并强制清理。
                    if voice.on_drain_timeout(Instant::now()) {
                        force_drain_cleanup(&mut socket, metrics, &mut subscription).await;
                        continue;
                    }
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
                            voice.set_read_aloud(request_wire.tts);
                            connection_session_id = new_session_id();
                            let session_id = connection_session_id.clone();
                            request = Some(request_wire);
                            match state.spawn_voice_worker(
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
                            if let Some(stop) = voice.on_client_cancel(Instant::now()) {
                                send_json(&mut socket, stop_audio_json(&stop)).await;
                                if let Some(handle) = worker.as_ref() {
                                    handle.cancel(stop.reason);
                                }
                            }
                            stop_subscription(&mut subscription);
                            if let Some(handle) = worker.as_ref() {
                                handle.shutdown();
                            }
                            close(&mut socket).await;
                            break;
                        }
                        Ok(Classified::Cancel(CancelReason::UserCancel)) => {
                            if let Some(handle) = worker.as_ref() {
                                handle.cancel("user_cancel");
                            }
                            if let Some(stop) = voice.on_client_cancel(Instant::now()) {
                                send_json(&mut socket, stop_audio_json(&stop)).await;
                                stop_subscription(&mut subscription);
                            }
                        }
                        Ok(Classified::PlaybackDrained(epoch)) => {
                            if voice.on_playback_drained(epoch) == DrainedOutcome::Accepted {
                                metrics.record_audio_flushed();
                                send_json(&mut socket, state_json(TurnPhase::Listening.as_str()))
                                    .await;
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
                        /*
                         * 听写模式只把文本交给客户端：定稿进输入坞，发不发由人决定。
                         * 这时不分配 epoch、不订阅回合，也不下发 `turn`，
                         * 因此不会在会话里凭空长出一轮。
                         */
                        if !request.submit {
                            send_json(&mut socket, state_json(TurnPhase::Listening.as_str()))
                                .await;
                            continue;
                        }
                        let phase_events = handle_final(
                            &mut socket,
                            state,
                            &mut voice,
                            &mut subscription,
                            &turn_tx,
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
                VoiceOut::Audio { epoch, seq, pcm } => {
                    if voice.accepts_audio(epoch) {
                        let _ = socket
                            .send(Message::Binary(audio_frame(epoch, seq, &pcm).into()))
                            .await;
                    }
                }
            },
            Some(signal) = turn_rx.recv() => {
                let phase_before = voice.phase();
                let events = handle_turn_signal(
                    &signal,
                    &mut voice,
                    &mut subscription,
                    worker.as_ref(),
                );
                for event in events {
                    send_json(&mut socket, event).await;
                }
                if phase_before != TurnPhase::Speaking && voice.phase() == TurnPhase::Speaking {
                    send_json(&mut socket, state_json(TurnPhase::Speaking.as_str())).await;
                }
            }
            else => break,
        }
    }
    stop_subscription(&mut subscription);
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
    subscription: &mut Option<Arc<AtomicBool>>,
    turn_tx: &tokio_mpsc::UnboundedSender<TurnSignal>,
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
            state_json(TurnPhase::Listening.as_str()),
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
            let frame = turn_json(&interaction_id, &assigned, &lane_id, &lane_id);
            let stop = Arc::new(AtomicBool::new(false));
            if let Some(previous) = subscription.replace(Arc::clone(&stop)) {
                previous.store(true, Ordering::Relaxed);
            }
            spawn_turn_subscription(
                state.context.clone(),
                request.workspace_id.clone(),
                interaction_id,
                assigned.epoch,
                turn_tx.clone(),
                stop,
            );
            vec![frame]
        }
        Ok(Err(error)) => {
            voice.rejected();
            metrics.record_error("submit_rejected");
            vec![
                submit_rejected_json(reject_reason(&error)),
                state_json(TurnPhase::Listening.as_str()),
            ]
        }
        Err(_) => {
            voice.rejected();
            metrics.record_error("submit_rejected");
            vec![
                submit_rejected_json("提交任务未能完成"),
                state_json(TurnPhase::Listening.as_str()),
            ]
        }
    }
}

/// 处理订阅线程投递的助手增量与终态；返回还需要下发的帧。
fn handle_turn_signal(
    signal: &TurnSignal,
    voice: &mut VoiceTurn,
    subscription: &mut Option<Arc<AtomicBool>>,
    worker: Option<&Arc<VoiceWorker>>,
) -> Vec<String> {
    match signal {
        TurnSignal::Delta { epoch, text } => {
            if *epoch != voice.epoch() {
                return Vec::new();
            }
            let speaks = voice.on_assistant_delta(text);
            send_speaks(worker, &speaks);
            Vec::new()
        }
        TurnSignal::Completed { epoch, response } => {
            if *epoch != voice.epoch() {
                return Vec::new();
            }
            let (speaks, stop) = voice.on_turn_completed(Instant::now(), response.as_deref());
            send_speaks(worker, &speaks);
            finish_stop(stop, worker, subscription)
        }
        TurnSignal::Closed { epoch } => {
            if *epoch != voice.epoch() {
                return Vec::new();
            }
            // 订阅断开后不会再有任何增量：按本轮结束收口，避免相位永久停在说话。
            let (speaks, stop) = voice.on_turn_completed(Instant::now(), None);
            send_speaks(worker, &speaks);
            finish_stop(stop, worker, subscription)
        }
    }
}

fn send_speaks(worker: Option<&Arc<VoiceWorker>>, speaks: &[SpeakFrame]) {
    let Some(worker) = worker else {
        return;
    };
    for speak in speaks {
        worker.speak(&speak.turn_id, speak.epoch, speak.seq, &speak.text);
    }
}

fn finish_stop(
    stop: Option<StopAudio>,
    worker: Option<&Arc<VoiceWorker>>,
    subscription: &mut Option<Arc<AtomicBool>>,
) -> Vec<String> {
    let Some(stop) = stop else {
        return Vec::new();
    };
    if let Some(worker) = worker {
        worker.cancel(stop.reason);
    }
    stop_subscription(subscription);
    vec![stop_audio_json(&stop)]
}

fn stop_subscription(slot: &mut Option<Arc<AtomicBool>>) {
    if let Some(stop) = slot.take() {
        stop.store(true, Ordering::Relaxed);
    }
}

fn audio_frame(epoch: u32, seq: u32, pcm: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(9 + pcm.len());
    frame.push(0x02);
    frame.extend_from_slice(&epoch.to_be_bytes());
    frame.extend_from_slice(&seq.to_be_bytes());
    frame.extend_from_slice(pcm);
    frame
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

/// 停播回执超时：记 `internal` 并强制清理一侧的可见动作。
async fn force_drain_cleanup(
    socket: &mut WebSocket,
    metrics: &Arc<VoiceMetrics>,
    subscription: &mut Option<Arc<AtomicBool>>,
) {
    metrics.record_error("internal");
    metrics.record_drain_timeout();
    send_json(socket, drain_timeout_json()).await;
    send_json(socket, state_json(TurnPhase::Listening.as_str())).await;
    stop_subscription(subscription);
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
        assert_eq!(timeouts.drained, std::time::Duration::from_secs(10));
    }

    #[test]
    fn injected_timeouts_are_honoured() {
        let timeouts = VoiceTimeouts {
            idle: Duration::from_millis(50),
            session: Duration::from_millis(80),
            warmup: Duration::from_millis(20),
            drained: Duration::from_millis(30),
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
