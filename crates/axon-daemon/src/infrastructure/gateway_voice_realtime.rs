//! 端到端 realtime 车道：一条 WebSocket 直连百炼，音频上行、转写与音频下行。
//!
//! 与级联车道的差别只收在这一层：没有预热、没有回合提交、没有朗读稿下发，
//! 识别、生成与合成都在提供方完成。对上层仍沿用同一套 [`Command`] / [`VoiceOut`]，
//! 因此会话循环不需要为车道分叉，只有一个例外：realtime 的音频按回复轮次自己
//! 编号，不能再用级联那套「提交成功才分配 epoch」的过滤。
//!
//! 音频不做服务端节流：协议不会按实时节奏推音频，而桌面端播放器按
//! `AudioContext.currentTime` 连续排程，一次收齐也能正确播出；这里只保证分片
//! 大小与序号单调，把「什么时间播」留给播放器。

use super::gateway_voice_metrics::VoiceMetrics;
use super::gateway_voice_out_frames::{error_json, state_json};
use super::gateway_voice_provider::{Lane, RealtimeEndpoint, realtime_endpoint};
use super::gateway_voice_realtime_protocol::{
    APPEND_CHUNK_BYTES, CANCEL_JSON, CLEAR_JSON, COMMIT_JSON, FINISH_JSON, Resampler24kTo16k,
    ServerEvent, audio_append_json, merge_transcript, parse_server_event, playback_chunks,
};
use super::gateway_voice_worker::{Command, CommandQueue, VoiceOut, VoiceWorker};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderName};
use tokio_tungstenite::tungstenite::protocol::Message;

/// 上行音频队列容量（帧）：与级联车道一致，约 2 s 缓冲。
const AUDIO_QUEUE_CAPACITY: usize = 8;
/// 工作循环轮询步长。
const WORKER_TICK: Duration = Duration::from_millis(20);
/// 百炼的额外请求头：关闭数据巡检，端到端语音不上送内容审核。
const DATA_INSPECTION_HEADER: HeaderName = HeaderName::from_static("x-dashscope-datainspection");
/// 埋点阶段：连接与配置。
const STAGE_ARMING: &str = "arming";
/// 埋点阶段：会话进行中。
const STAGE_WORKER: &str = "worker";

/// 拉起 realtime 会话；返回与级联同形的句柄，会话循环因此不需要区分车道。
///
/// # Errors
///
/// 返回 `(错误帧, 是否已开始会话)`。凭据缺失时如实报 `provider_unavailable`。
#[cfg_attr(test, allow(dead_code))]
pub(super) fn spawn_realtime(
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) -> Result<Arc<VoiceWorker>, (String, bool)> {
    let endpoint = realtime_endpoint().map_err(|code| (error_json(code, STAGE_ARMING), false))?;
    Ok(spawn_realtime_with(endpoint, out, warmup, metrics))
}

/// 用给定端点拉起会话；测试直接注入本地端点，不依赖进程环境变量。
pub(super) fn spawn_realtime_with(
    endpoint: RealtimeEndpoint,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) -> Arc<VoiceWorker> {
    let queue = Arc::new(CommandQueue::new(AUDIO_QUEUE_CAPACITY));
    let worker = Arc::new(VoiceWorker::new(
        Arc::clone(&queue),
        Arc::clone(&metrics),
        Lane::Realtime,
    ));
    tokio::spawn(run_realtime(endpoint, queue, out, warmup, metrics));
    worker
}

/// 一次 realtime 会话的运行主体。
async fn run_realtime(
    endpoint: RealtimeEndpoint,
    queue: Arc<CommandQueue>,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) {
    let request = match build_request(&endpoint) {
        Ok(request) => request,
        Err(code) => {
            fail(&out, &metrics, code, STAGE_ARMING);
            return;
        }
    };
    ensure_rustls_crypto_provider();
    let connected = tokio::time::timeout(warmup, tokio_tungstenite::connect_async(request)).await;
    let (socket, _) = match connected {
        Ok(Ok(pair)) => pair,
        Ok(Err(_)) => {
            fail(&out, &metrics, "worker_unavailable", STAGE_ARMING);
            return;
        }
        Err(_) => {
            fail(&out, &metrics, "worker_unavailable", STAGE_ARMING);
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();

    // 握手后第一件事是下发配置；服务端确认之前送音频会被丢弃。
    let update = endpoint.session.session_update_json();
    if sink.send(Message::Text(update.into())).await.is_err() {
        fail(&out, &metrics, "worker_unavailable", STAGE_ARMING);
        return;
    }
    if !await_session_ready(&mut stream, &out, &metrics, warmup).await {
        let _ = sink.close().await;
        return;
    }
    if out.send(VoiceOut::Text(state_json("listening"))).is_err() {
        return;
    }

    let mut resampler = Resampler24kTo16k::new();
    let mut utterance_seq: u32 = 0;
    let mut epoch: u32 = 0;
    let mut seq: u32 = 0;
    let mut last_partial = String::new();

    loop {
        for command in queue.drain() {
            let sent = match command {
                Command::Audio(bytes) => send_pcm(&mut sink, &bytes).await,
                Command::Stop(_) => send_text(&mut sink, COMMIT_JSON).await,
                Command::Cancel(_) => {
                    if send_text(&mut sink, CANCEL_JSON).await {
                        send_text(&mut sink, CLEAR_JSON).await
                    } else {
                        false
                    }
                }
                // realtime 由服务端自己合成，朗读稿没有落点；丢弃不是错误。
                Command::Speak { .. } => true,
                Command::Shutdown => {
                    let _ = send_text(&mut sink, FINISH_JSON).await;
                    let _ = sink.close().await;
                    return;
                }
            };
            if !sent {
                fail(&out, &metrics, "worker_crashed", STAGE_WORKER);
                return;
            }
        }
        metrics.set_queue_depth(queue.depth());

        let next = tokio::select! {
            message = stream.next() => message,
            () = tokio::time::sleep(WORKER_TICK) => continue,
        };
        let text = match next {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(_)) => continue,
            Some(Err(_)) => {
                fail(&out, &metrics, "worker_crashed", STAGE_WORKER);
                return;
            }
        };

        match parse_server_event(&text) {
            ServerEvent::SpeechStarted => {
                let _ = out.send(VoiceOut::Text(state_json("listening")));
            }
            ServerEvent::SpeechStopped => {
                last_partial.clear();
                let _ = out.send(VoiceOut::Text(state_json("thinking")));
            }
            ServerEvent::InputTranscriptDelta { text, stash } => {
                let merged = merge_transcript(&text, &stash);
                if merged.is_empty() || merged == last_partial {
                    continue;
                }
                last_partial.clone_from(&merged);
                let _ = out.send(VoiceOut::Text(partial_json(utterance_seq, &merged)));
            }
            ServerEvent::InputTranscriptCompleted { transcript } => {
                let final_text = transcript.trim();
                if !final_text.is_empty() {
                    let _ = out.send(VoiceOut::Text(partial_json(utterance_seq, final_text)));
                }
                last_partial.clear();
                utterance_seq = utterance_seq.saturating_add(1);
            }
            ServerEvent::ResponseCreated => {
                epoch = epoch.saturating_add(1);
                seq = 0;
                let _ = out.send(VoiceOut::Text(state_json("speaking")));
            }
            ServerEvent::OutputTranscriptDelta { text } => {
                if text.is_empty() {
                    continue;
                }
                let _ = out.send(VoiceOut::Text(assistant_json(epoch, &text)));
            }
            ServerEvent::AudioDelta { pcm } => {
                if epoch == 0 {
                    epoch = 1;
                }
                let pcm16 = resampler.push(&pcm);
                for chunk in playback_chunks(&pcm16) {
                    seq = seq.saturating_add(1);
                    if out
                        .send(VoiceOut::Audio {
                            epoch,
                            seq,
                            pcm: chunk.to_vec(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
            ServerEvent::ResponseDone => {
                let _ = out.send(VoiceOut::Text(state_json("listening")));
            }
            ServerEvent::Error { code, message } => {
                metrics.record_error(&code);
                let _ = out.send(VoiceOut::Text(provider_error_json(&code, &message)));
            }
            ServerEvent::SessionCreated
            | ServerEvent::SessionUpdated
            | ServerEvent::OutputTranscriptDone { .. }
            | ServerEvent::Ignored => {}
        }
    }

    let _ = sink.close().await;
}

/// 等服务端确认配置；期间只处理错误与关闭，其余事件忽略。
async fn await_session_ready<S>(
    stream: &mut S,
    out: &tokio_mpsc::UnboundedSender<VoiceOut>,
    metrics: &Arc<VoiceMetrics>,
    warmup: Duration,
) -> bool
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = Instant::now() + warmup;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            fail(out, metrics, "model_unusable", STAGE_ARMING);
            return false;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => match parse_server_event(&text) {
                ServerEvent::SessionUpdated => return true,
                ServerEvent::Error { code, message } => {
                    metrics.record_error(&code);
                    let _ = out.send(VoiceOut::Text(provider_error_json(&code, &message)));
                    return false;
                }
                _ => {}
            },
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => {
                fail(out, metrics, "worker_unavailable", STAGE_ARMING);
                return false;
            }
            Err(_) => {
                fail(out, metrics, "model_unusable", STAGE_ARMING);
                return false;
            }
        }
    }
}

/// 把一帧上行音频切成 20 ms 分片再发；协议的 `append` 只认原始 PCM 的 base64。
async fn send_pcm<S>(sink: &mut S, bytes: &[u8]) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    for chunk in bytes.chunks(APPEND_CHUNK_BYTES) {
        let json = audio_append_json(chunk);
        if sink.send(Message::Text(json.into())).await.is_err() {
            return false;
        }
    }
    true
}

async fn send_text<S>(sink: &mut S, payload: &str) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    sink.send(Message::Text(payload.to_owned().into()))
        .await
        .is_ok()
}

/// 上报一次失败：指标、错误帧，然后让会话循环收线。
fn fail(
    out: &tokio_mpsc::UnboundedSender<VoiceOut>,
    metrics: &Arc<VoiceMetrics>,
    code: &str,
    stage: &str,
) {
    metrics.record_error(code);
    let _ = out.send(VoiceOut::Text(error_json(code, stage)));
}

/// `partial` 帧：用户这句话的当前最佳文本。
fn partial_json(utterance_seq: u32, text: &str) -> String {
    serde_json::json!({
        "type": "partial",
        "utteranceSeq": utterance_seq,
        "text": text,
    })
    .to_string()
}

/// 助手文字帧：realtime 由服务端直接产文，没有 `turn` 与订阅。
fn assistant_json(epoch: u32, text: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "epoch": epoch,
        "text": text,
    })
    .to_string()
}

/// 提供方错误的对外形状：保留服务端说明，前端按未知码用这条文案兜底。
fn provider_error_json(code: &str, message: &str) -> String {
    serde_json::json!({
        "type": "error",
        "code": code,
        "stage": STAGE_WORKER,
        "retryable": false,
        "message": message,
        "detail": { "family": serde_json::Value::Null },
    })
    .to_string()
}

/// 构造握手请求：Bearer 凭据 + 关闭数据巡检。
fn build_request(
    endpoint: &RealtimeEndpoint,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, &'static str> {
    let mut request = endpoint
        .url
        .as_str()
        .into_client_request()
        .map_err(|_| "provider_unavailable")?;
    let headers = request.headers_mut();
    let bearer = format!("Bearer {}", endpoint.api_key);
    let value = HeaderValue::from_str(&bearer).map_err(|_| "provider_unavailable")?;
    headers.insert(AUTHORIZATION, value);
    headers.insert(DATA_INSPECTION_HEADER, HeaderValue::from_static("disable"));
    Ok(request)
}

#[cfg(test)]
#[path = "gateway_voice_realtime_tests.rs"]
mod tests;
