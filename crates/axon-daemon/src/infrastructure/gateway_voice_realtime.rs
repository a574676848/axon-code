//! 端到端 realtime 车道：一条 WebSocket 直连提供方，音频上行、转写与音频下行。
//!
//! 与级联车道的差别只收在这一层：没有预热、没有回合提交、没有朗读稿下发，
//! 识别、生成与合成都在提供方完成。对上层仍沿用同一套 [`Command`] / [`VoiceOut`]，
//! 因此会话循环不需要为车道分叉，只有一个例外：realtime 的音频按回复轮次自己
//! 编号，不能再用级联那套「提交成功才分配 epoch」的过滤。
//!
//! **厂商差异只出现在 [`spawn_realtime`] 的构造期分派里**：协议编解码、握手参数、
//! 下行采样率全部收在 [`RealtimeProvider`] 实现内部，会话循环只认规范化事件。
//!
//! 音频不做服务端节流：协议不会按实时节奏推音频，而桌面端播放器按
//! `AudioContext.currentTime` 连续排程，一次收齐也能正确播出；这里只保证分片
//! 大小与序号单调，把「什么时间播」留给播放器。

use super::gateway_voice_metrics::VoiceMetrics;
use super::gateway_voice_out_frames::{error_json, state_json};
use super::gateway_voice_provider::{Lane, RealtimeProtocol, VoiceRuntime};
use super::gateway_voice_realtime_protocol::{BailianProvider, merge_transcript, playback_chunks};
use super::gateway_voice_realtime_provider::{RealtimeControl, RealtimeEvent, RealtimeProvider};
use super::gateway_voice_worker::{Command, CommandQueue, VoiceOut, VoiceWorker};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;

/// 上行音频队列容量（帧）：与级联车道一致，约 2 s 缓冲。
const AUDIO_QUEUE_CAPACITY: usize = 8;
/// 工作循环轮询步长。
const WORKER_TICK: Duration = Duration::from_millis(20);
/// 埋点阶段：连接与配置。
const STAGE_ARMING: &str = "arming";
/// 埋点阶段：会话进行中。
const STAGE_WORKER: &str = "worker";

/// 拉起 realtime 会话；返回与级联同形的句柄，会话循环因此不需要区分车道。
///
/// 这里是**唯一的厂商分派点**：按 `channel.protocol` 实例化具体实现，
/// 之后所有代码都跑在泛型单态化后的路径上，每帧没有 trait 对象分派。
///
/// # Errors
///
/// 返回 `(错误帧, 是否已开始会话)`。凭据缺失或协议未接通时如实报
/// `provider_unavailable`。
#[cfg_attr(test, allow(dead_code))]
pub(super) fn spawn_realtime(
    runtime: &VoiceRuntime,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) -> Result<Arc<VoiceWorker>, (String, bool)> {
    let Some(endpoint) = runtime.realtime() else {
        return Err((error_json("provider_unavailable", STAGE_ARMING), false));
    };
    // 人格只有一份真源：每次建会话从 SOUL 现算，刷新配置或重启都不需要。
    let mut session = endpoint.session.clone();
    session.instructions = runtime.persona_instructions();
    match endpoint.protocol {
        RealtimeProtocol::Bailian => Ok(spawn_realtime_with(
            BailianProvider::new(endpoint.url.clone(), endpoint.api_key.clone(), session),
            out,
            warmup,
            metrics,
        )),
        // 火山 realtime 的协议探针与实现是 M4 的下一步；在此之前如实报不可用，
        // 不拿百炼实现冒充第二提供方。
        RealtimeProtocol::Volc => Err((error_json("provider_unavailable", STAGE_ARMING), false)),
    }
}

/// 用给定提供方拉起会话；测试直接注入本地提供方，不依赖进程环境变量。
pub(super) fn spawn_realtime_with<P>(
    provider: P,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) -> Arc<VoiceWorker>
where
    P: RealtimeProvider + 'static,
{
    let queue = Arc::new(CommandQueue::new(AUDIO_QUEUE_CAPACITY));
    let worker = Arc::new(VoiceWorker::new(
        Arc::clone(&queue),
        Arc::clone(&metrics),
        Lane::Realtime,
    ));
    tokio::spawn(run_realtime(provider, queue, out, warmup, metrics));
    worker
}

/// 一次 realtime 会话的运行主体。
async fn run_realtime<P>(
    mut provider: P,
    queue: Arc<CommandQueue>,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) where
    P: RealtimeProvider,
{
    let request = match provider.handshake_request() {
        Ok(request) => request,
        Err(code) => {
            fail(&out, &metrics, &code, STAGE_ARMING);
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
    let update = provider.session_update();
    if sink.send(Message::Text(update.into())).await.is_err() {
        fail(&out, &metrics, "worker_unavailable", STAGE_ARMING);
        return;
    }
    if !await_session_ready(&mut stream, &provider, &out, &metrics, warmup).await {
        let _ = sink.close().await;
        return;
    }
    if out.send(VoiceOut::Text(state_json("listening"))).is_err() {
        return;
    }

    // 复用同一个消息缓冲：每帧只清空不重建，避免热路径反复分配容器。
    let mut encoded: Vec<Message> = Vec::with_capacity(4);
    let mut utterance_seq: u32 = 0;
    let mut epoch: u32 = 0;
    let mut seq: u32 = 0;
    let mut last_partial = String::new();
    // 「说完 → 出声」的计时起点：`speech_stopped` 落点，首个音频分片消费掉。
    let mut speech_stopped_at: Option<Instant> = None;
    let mut first_audio_sent = false;

    loop {
        for command in queue.drain() {
            let sent = match command {
                Command::Audio(bytes) => {
                    encoded.clear();
                    provider.encode_audio(&bytes, &mut encoded);
                    send_all(&mut sink, &mut encoded).await
                }
                Command::Stop(_) => {
                    send_one(&mut sink, provider.encode_control(RealtimeControl::Commit)).await
                }
                Command::Cancel(_) => {
                    if send_one(&mut sink, provider.encode_control(RealtimeControl::Cancel)).await {
                        send_one(&mut sink, provider.encode_control(RealtimeControl::Clear)).await
                    } else {
                        false
                    }
                }
                // realtime 由服务端自己合成，朗读稿没有落点；丢弃不是错误。
                Command::Speak { .. } => true,
                Command::Shutdown => {
                    let _ =
                        send_one(&mut sink, provider.encode_control(RealtimeControl::Finish)).await;
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

        // 先求值再 match：`decode_event` 借 `provider`，`resample_downlink` 要独占借用。
        // 解码耗时每帧必过，在这里埋点，不受分支里 `continue` 的影响。
        let decode_started = Instant::now();
        let event = provider.decode_event(&text);
        metrics.record_frame_latency(decode_started.elapsed().as_micros() as u64);
        match event {
            RealtimeEvent::SpeechStarted => {
                let _ = out.send(VoiceOut::Text(state_json("listening")));
            }
            RealtimeEvent::SpeechStopped => {
                last_partial.clear();
                speech_stopped_at = Some(Instant::now());
                let _ = out.send(VoiceOut::Text(state_json("thinking")));
            }
            RealtimeEvent::InputTranscriptDelta { text, stash } => {
                let merged = merge_transcript(&text, &stash);
                if merged.is_empty() || merged == last_partial {
                    continue;
                }
                last_partial.clone_from(&merged);
                let _ = out.send(VoiceOut::Text(partial_json(utterance_seq, &merged)));
            }
            RealtimeEvent::InputTranscriptCompleted { transcript } => {
                let final_text = transcript.trim();
                if !final_text.is_empty() {
                    let _ = out.send(VoiceOut::Text(partial_json(utterance_seq, final_text)));
                }
                last_partial.clear();
                utterance_seq = utterance_seq.saturating_add(1);
            }
            RealtimeEvent::ResponseCreated => {
                epoch = epoch.saturating_add(1);
                seq = 0;
                let _ = out.send(VoiceOut::Text(state_json("speaking")));
            }
            RealtimeEvent::OutputTranscriptDelta { text } => {
                if text.is_empty() {
                    continue;
                }
                let _ = out.send(VoiceOut::Text(assistant_json(epoch, &text)));
            }
            RealtimeEvent::AudioDelta { pcm } => {
                if epoch == 0 {
                    epoch = 1;
                }
                let pcm16 = provider.resample_downlink(&pcm);
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
                    if !first_audio_sent {
                        first_audio_sent = true;
                        if let Some(started) = speech_stopped_at.take() {
                            metrics.record_first_audio(started.elapsed().as_micros() as u64);
                        }
                    }
                }
            }
            RealtimeEvent::ResponseDone => {
                let _ = out.send(VoiceOut::Text(state_json("listening")));
            }
            RealtimeEvent::Error { code, message } => {
                metrics.record_error(&code);
                let _ = out.send(VoiceOut::Text(provider_error_json(&code, &message)));
            }
            RealtimeEvent::SessionCreated
            | RealtimeEvent::SessionUpdated
            | RealtimeEvent::OutputTranscriptDone { .. }
            | RealtimeEvent::Ignored => {}
        }
    }

    let _ = sink.close().await;
}

/// 等服务端确认配置；期间只处理错误与关闭，其余事件忽略。
async fn await_session_ready<S, P>(
    stream: &mut S,
    provider: &P,
    out: &tokio_mpsc::UnboundedSender<VoiceOut>,
    metrics: &Arc<VoiceMetrics>,
    warmup: Duration,
) -> bool
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    P: RealtimeProvider,
{
    let deadline = Instant::now() + warmup;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            fail(out, metrics, "model_unusable", STAGE_ARMING);
            return false;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => match provider.decode_event(&text) {
                RealtimeEvent::SessionUpdated => return true,
                RealtimeEvent::Error { code, message } => {
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

/// 逐条发出编码好的消息；失败即视为连接断开。
async fn send_all<S>(sink: &mut S, messages: &mut Vec<Message>) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    for message in messages.drain(..) {
        if sink.send(message).await.is_err() {
            return false;
        }
    }
    true
}

/// 发一条消息；失败即视为连接断开。
async fn send_one<S>(sink: &mut S, message: Message) -> bool
where
    S: futures_util::Sink<Message> + Unpin,
{
    sink.send(message).await.is_ok()
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

#[cfg(test)]
#[path = "gateway_voice_realtime_tests.rs"]
mod tests;
