//! 语音会话的宿主工作线程：命令队列、背压、桥接事件与对外 JSON。
//!
//! 上行音频进有界队列：满了丢最旧并计数（PROTOCOL §4.2 的 `backpressure`），
//! 控制命令（`stop`/`cancel`/`shutdown`）永不被丢弃，保证会话一定能收尾。

use super::gateway_voice_metrics::{HostSnapshot, VoiceMetrics};
use super::gateway_voice_out_frames::{error_json, message_for, state_json};
use super::gateway_voice_ready::{DEFAULT_ASR_MODEL, model_root};
use axon_voice_bridge::{BridgeError, HostTransport, VoiceBridge};
use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;

/// 上行音频队列容量（帧）：每帧最多 256 ms，8 帧约 2 s 缓冲。
const AUDIO_QUEUE_CAPACITY: usize = 8;
/// 单帧音频上限（8 KiB + 1 字节帧头），与 PROTOCOL §2.2 一致。
pub(super) const MAX_AUDIO_FRAME_BYTES: usize = 8 * 1024 + 1;
/// 工作线程轮询步长。
const WORKER_TICK: Duration = Duration::from_millis(20);
/// 丢帧时最多每秒上报一次 `backpressure`，避免错误帧自己形成洪水。
const BACKPRESSURE_REPORT_INTERVAL: Duration = Duration::from_secs(1);
/// 连续超限的恢复间隔：超过这个时长没有新的丢帧才算恢复。
const OVERLOAD_RECOVERY_GAP: Duration = Duration::from_secs(2);

/// 会话开始后要处理的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// 上行音频。
    Audio(Vec<u8>),
    /// 结束当前句。
    Stop(u32),
    /// 取消当前轮；参数是合同原因值。
    Cancel(String),
    /// 下发一句朗读稿。
    Speak {
        /// 回合标识。
        turn_id: String,
        /// 轮次。
        epoch: u32,
        /// 句序号。
        seq: u32,
        /// 朗读稿。
        text: String,
    },
    /// 收线。
    Shutdown,
}

/// 工作线程送往 WS 会话的输出。
#[derive(Debug)]
pub(super) enum VoiceOut {
    /// 直接下发的 JSON 文本帧。
    Text(String),
    /// 定稿转写：由会话负责提交 durable ingress。
    Final {
        /// 句序号。
        utterance_seq: u32,
        /// 定稿文本。
        text: String,
        /// 本句起点（宿主单调毫秒）。
        start_ms: u64,
        /// 端点判定时刻。
        end_ms: u64,
        /// 定稿产出时刻。
        finalize_ms: u64,
    },
    /// 对话音频：会话原样封装成 WS `0x02` 帧。
    Audio {
        /// 轮次。
        epoch: u32,
        /// 宿主包序号，原样透传。
        seq: u32,
        /// s16le 载荷。
        pcm: Vec<u8>,
    },
}

/// 入队结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PushOutcome {
    /// 已入队。
    Queued,
    /// 队列已满，丢了最旧的一帧。
    Dropped {
        /// 本次丢的帧数（0 或 1）。
        frames: u64,
        /// 本次丢的字节数。
        bytes: u64,
    },
}

/// 有界命令队列。
#[derive(Debug)]
struct CommandQueue {
    state: Mutex<QueueState>,
    capacity: usize,
    dropped_frames: AtomicU64,
    dropped_bytes: AtomicU64,
}

#[derive(Debug, Default)]
struct QueueState {
    commands: VecDeque<Command>,
    /// 首次丢帧的时刻；持续丢帧期间保持，恢复后清空。
    overload_since: Option<Instant>,
    /// 最近一次丢帧时刻。
    last_drop: Option<Instant>,
}

impl CommandQueue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            capacity: capacity.max(1),
            dropped_frames: AtomicU64::new(0),
            dropped_bytes: AtomicU64::new(0),
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 推入一帧音频；满则丢最旧的一帧（不关连接）。
    fn push_audio(&self, bytes: Vec<u8>) -> PushOutcome {
        let mut state = self.guard();
        let mut dropped = PushOutcome::Queued;
        // 容量只约束音频帧：控制命令必须能进来，绝不因为音频挤占而被丢弃。
        loop {
            let queued_audio = state
                .commands
                .iter()
                .filter(|command| matches!(command, Command::Audio(_)))
                .count();
            if queued_audio < self.capacity {
                break;
            }
            let Some(index) = state
                .commands
                .iter()
                .position(|command| matches!(command, Command::Audio(_)))
            else {
                break;
            };
            let Some(Command::Audio(oldest)) = state.commands.remove(index) else {
                continue;
            };
            self.dropped_frames.fetch_add(1, Ordering::Relaxed);
            self.dropped_bytes
                .fetch_add(oldest.len() as u64, Ordering::Relaxed);
            state.overload_since.get_or_insert_with(Instant::now);
            state.last_drop = Some(Instant::now());
            dropped = PushOutcome::Dropped {
                frames: 1,
                bytes: oldest.len() as u64,
            };
        }
        state.commands.push_back(Command::Audio(bytes));
        dropped
    }

    fn push_control(&self, command: Command) {
        let mut state = self.guard();
        state.commands.push_back(command);
    }

    /// 取走当前全部命令。
    ///
    /// 注意：排空不等于恢复。持续超限时每轮都会排空，若在这里清计时，
    /// 「连续 10 s 超限才断连」永远不可达，因此恢复判定按丢帧间隔做。
    fn drain(&self) -> Vec<Command> {
        self.guard().commands.drain(..).collect()
    }

    fn depth(&self) -> u64 {
        self.guard().commands.len() as u64
    }

    /// 连续超限时长；未超限时为 `None`。
    fn overload_for(&self) -> Option<Duration> {
        let mut state = self.guard();
        let last_drop = state.last_drop?;
        if last_drop.elapsed() >= OVERLOAD_RECOVERY_GAP {
            // 已经有一个恢复间隔没有新丢帧，视为恢复。
            state.overload_since = None;
            state.last_drop = None;
            return None;
        }
        state.overload_since.map(|since| since.elapsed())
    }

    fn totals(&self) -> (u64, u64) {
        (
            self.dropped_frames.load(Ordering::Relaxed),
            self.dropped_bytes.load(Ordering::Relaxed),
        )
    }
}

/// 会话持有的工作句柄。
pub(super) struct VoiceWorker {
    queue: Arc<CommandQueue>,
    metrics: Arc<VoiceMetrics>,
    last_backpressure_at: Mutex<Option<Instant>>,
}

impl VoiceWorker {
    /// 推入一帧音频。
    pub(super) fn push_audio(&self, bytes: Vec<u8>) -> PushOutcome {
        let outcome = self.queue.push_audio(bytes);
        self.metrics.set_queue_depth(self.queue.depth());
        if let PushOutcome::Dropped { frames, bytes } = outcome {
            self.metrics.record_drop(frames, bytes);
        }
        outcome
    }

    /// 结束当前句。
    pub(super) fn stop(&self, utterance_seq: u32) {
        self.queue.push_control(Command::Stop(utterance_seq));
    }

    /// 取消当前轮。
    pub(super) fn cancel(&self, reason: &str) {
        self.queue.push_control(Command::Cancel(reason.to_owned()));
    }

    /// 下发一句朗读稿。
    pub(super) fn speak(&self, turn_id: &str, epoch: u32, seq: u32, text: &str) {
        self.queue.push_control(Command::Speak {
            turn_id: turn_id.to_owned(),
            epoch,
            seq,
            text: text.to_owned(),
        });
    }

    /// 收线。
    pub(super) fn shutdown(&self) {
        self.queue.push_control(Command::Shutdown);
    }

    /// 连续超限时长。
    pub(super) fn overload_for(&self) -> Option<Duration> {
        self.queue.overload_for()
    }

    /// 丢帧累计。
    pub(super) fn dropped_totals(&self) -> (u64, u64) {
        self.queue.totals()
    }

    /// 该不该在这一轮上报 `backpressure`：每秒最多一次。
    pub(super) fn should_report_backpressure(&self) -> bool {
        let Ok(mut slot) = self.last_backpressure_at.lock() else {
            return false;
        };
        let now = Instant::now();
        match *slot {
            Some(at) if now.duration_since(at) < BACKPRESSURE_REPORT_INTERVAL => false,
            _ => {
                *slot = Some(now);
                true
            }
        }
    }
}

/// 拉起工作线程；返回可推音频与收线的句柄。
pub(super) fn spawn_worker(
    session_id: &str,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) -> Result<Arc<VoiceWorker>, (String, bool)> {
    let root = model_root();
    if !root.is_dir() {
        return Err((error_json("model_missing", "arming"), false));
    }
    let Some(exe) = host_exe() else {
        return Err((error_json("worker_unavailable", "arming"), false));
    };
    let mut host = HostConfig::new(exe, BUILD_COMMIT.to_owned());
    host.args.push("--model-root".to_owned());
    host.args.push(root.display().to_string());
    Ok(spawn_worker_with(
        session_id.to_owned(),
        manifest_digest(&root),
        out,
        warmup,
        metrics,
        move || {
            HostClient::spawn(&host)
                .map(|client| Box::new(client) as BoxedTransport)
                .map_err(|_| ())
        },
    ))
}

/// 宿主传输的擦除类型：生产是真实子进程，测试是进程内脚本宿主。
pub(super) type BoxedTransport = Box<dyn HostTransport + Send>;

/// 宿主传输工厂；测试用它把脚本宿主接进真实的 WS 会话循环。
#[cfg(test)]
pub(super) type HostLauncher = std::sync::Arc<dyn Fn() -> Result<BoxedTransport, ()> + Send + Sync>;

/// 用给定的宿主启动工作线程。
pub(super) fn spawn_worker_with<F>(
    session_id: String,
    manifest_sha: String,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
    launch: F,
) -> Arc<VoiceWorker>
where
    F: FnOnce() -> Result<BoxedTransport, ()> + Send + 'static,
{
    let queue = Arc::new(CommandQueue::new(AUDIO_QUEUE_CAPACITY));
    let worker = Arc::new(VoiceWorker {
        queue: Arc::clone(&queue),
        metrics: Arc::clone(&metrics),
        last_backpressure_at: Mutex::new(None),
    });
    std::thread::spawn(move || {
        run_worker(
            WorkerConfig {
                manifest_sha,
                session_id,
                launch: Box::new(launch),
            },
            queue,
            out,
            warmup,
            metrics,
        );
    });
    worker
}

impl super::GatewayHttpState {
    /// 按运行态拉起本会话的工作线程；测试可注入进程内脚本宿主。
    pub(super) fn spawn_voice_worker(
        &self,
        session_id: &str,
        out: tokio_mpsc::UnboundedSender<VoiceOut>,
        warmup: Duration,
        metrics: Arc<VoiceMetrics>,
    ) -> Result<Arc<VoiceWorker>, (String, bool)> {
        #[cfg(test)]
        if let Some(launcher) = self.voice_host.as_ref() {
            let transport =
                launcher().map_err(|()| (error_json("worker_unavailable", "arming"), false))?;
            return Ok(spawn_worker_with(
                session_id.to_owned(),
                "0".repeat(64),
                out,
                warmup,
                metrics,
                move || Ok(transport),
            ));
        }
        spawn_worker(session_id, out, warmup, metrics)
    }
}

struct WorkerConfig {
    manifest_sha: String,
    session_id: String,
    launch: Box<dyn FnOnce() -> Result<BoxedTransport, ()> + Send>,
}

fn run_worker(
    config: WorkerConfig,
    queue: Arc<CommandQueue>,
    out: tokio_mpsc::UnboundedSender<VoiceOut>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) {
    let transport = match (config.launch)() {
        Ok(transport) => transport,
        Err(()) => {
            metrics.record_error("worker_unavailable");
            let _ = out.send(VoiceOut::Text(error_json("worker_unavailable", "arming")));
            return;
        }
    };
    let mut bridge = VoiceBridge::new(transport);
    match bridge.warmup("asr", DEFAULT_ASR_MODEL, &config.manifest_sha, warmup) {
        Ok(Some(warm)) => metrics.set_rtf(warm.rtf_p50),
        Ok(None) => {}
        Err(error) => {
            let code = match error {
                BridgeError::Host(error) => error.code,
                _ => "worker_unavailable".to_owned(),
            };
            metrics.record_error(&code);
            let _ = out.send(VoiceOut::Text(error_json(&code, "arming")));
            bridge.shutdown();
            return;
        }
    }
    if bridge
        .start(&config.session_id, "full_duplex", "zh", &[])
        .is_err()
    {
        metrics.record_error("protocol_sequence");
        let _ = out.send(VoiceOut::Text(error_json("protocol_sequence", "arming")));
        bridge.shutdown();
        return;
    }
    let _ = out.send(VoiceOut::Text(state_json("listening")));

    loop {
        for command in queue.drain() {
            let outcome = match command {
                Command::Audio(bytes) => bridge.push_audio(&bytes),
                Command::Stop(utterance_seq) => bridge.end_utterance(utterance_seq),
                Command::Cancel(reason) => bridge.cancel(&reason),
                Command::Speak {
                    turn_id,
                    epoch,
                    seq,
                    text,
                } => bridge.speak(&turn_id, epoch, seq, &text),
                Command::Shutdown => {
                    bridge.shutdown();
                    return;
                }
            };
            if outcome.is_err() {
                metrics.record_error("protocol_sequence");
                let _ = out.send(VoiceOut::Text(error_json("protocol_sequence", "listening")));
                bridge.shutdown();
                return;
            }
        }
        metrics.set_queue_depth(queue.depth());
        match bridge.poll(WORKER_TICK) {
            Ok(Some(event)) => {
                if let Some(snapshot) = host_snapshot(&event) {
                    metrics.record_host(snapshot);
                }
                if let Some(text) = event_out(event) {
                    let _ = out.send(text);
                }
            }
            Ok(None) => {}
            Err(_) => {
                // 桥断了必须让客户端看见，不能静默等到超时。
                metrics.record_error("worker_crashed");
                let _ = out.send(VoiceOut::Text(error_json("worker_crashed", "worker")));
                break;
            }
        }
    }
    bridge.shutdown();
}

/// 心跳只更新指标，不转成客户端帧。
fn host_snapshot(event: &axon_voice_bridge::VoiceEvent) -> Option<HostSnapshot> {
    match event {
        axon_voice_bridge::VoiceEvent::Heartbeat(beat) => Some(HostSnapshot {
            pid: beat.pid,
            rss_kb: beat.rss_kb,
            queue_in: beat.queue_in,
            queue_out: beat.queue_out,
            model_state: match beat.model_state.as_str() {
                "warm" => "warm",
                "warming" => "warming",
                _ => "cold",
            },
        }),
        _ => None,
    }
}

fn event_out(event: axon_voice_bridge::VoiceEvent) -> Option<VoiceOut> {
    use axon_voice_bridge::VoiceEvent;
    match event {
        VoiceEvent::Partial(partial) => Some(VoiceOut::Text(
            serde_json::json!({
                "type": "partial",
                "utteranceSeq": partial.utterance_seq,
                "text": partial.text,
            })
            .to_string(),
        )),
        VoiceEvent::Final(final_message) => Some(VoiceOut::Final {
            utterance_seq: final_message.utterance_seq,
            text: final_message.text,
            start_ms: final_message.start_ms,
            end_ms: final_message.end_ms,
            finalize_ms: final_message.finalize_ms,
        }),
        VoiceEvent::Audio { epoch, seq, audio } => Some(VoiceOut::Audio {
            epoch,
            seq,
            pcm: audio,
        }),
        VoiceEvent::Error(error) => Some(VoiceOut::Text(
            serde_json::json!({
                "type": "error",
                "code": error.code,
                "stage": error.stage,
                "retryable": error.retryable,
                "message": message_for(&error.code),
            })
            .to_string(),
        )),
        VoiceEvent::CueStatus(_)
        | VoiceEvent::Heartbeat(_)
        | VoiceEvent::Warm(_)
        | VoiceEvent::CueAudio { .. }
        | VoiceEvent::Closed
        | VoiceEvent::Ignored(_) => None,
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
    // 生产是版本目录同级；开发树里测试二进制在 target/debug/deps，宿主在上一级。
    for base in [dir.to_path_buf(), dir.parent()?.to_path_buf()] {
        let candidate = base.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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

#[cfg(test)]
#[path = "gateway_voice_worker_tests.rs"]
mod tests;
