//! 语音会话的宿主工作线程：命令队列、背压、桥接事件与对外 JSON。
//!
//! 上行音频进有界队列：满了丢最旧并计数（PROTOCOL §4.2 的 `backpressure`），
//! 控制命令（`stop`/`cancel`/`shutdown`）永不被丢弃，保证会话一定能收尾。

use super::gateway_voice_metrics::VoiceMetrics;
use axon_voice::VoiceFailure;
use axon_voice_bridge::{BridgeError, VoiceBridge};
use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::model::sha256::hex_digest;
use axon_voice_host::supervisor::{HostClient, HostConfig};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;

/// 默认请求的识别模型名，与合同示例一致。
const DEFAULT_ASR_MODEL: &str = "sherpa-streaming-zh-en";
/// 上行音频队列容量（帧）：每帧最多 256 ms，8 帧约 2 s 缓冲。
const AUDIO_QUEUE_CAPACITY: usize = 8;
/// 单帧音频上限（8 KiB + 1 字节帧头），与 PROTOCOL §2.2 一致。
pub(super) const MAX_AUDIO_FRAME_BYTES: usize = 8 * 1024 + 1;
/// 工作线程轮询步长。
const WORKER_TICK: Duration = Duration::from_millis(20);
/// 丢帧时最多每秒上报一次 `backpressure`，避免错误帧自己形成洪水。
const BACKPRESSURE_REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// 会话开始后要处理的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// 上行音频。
    Audio(Vec<u8>),
    /// 结束当前句。
    Stop(u32),
    /// 取消当前轮。
    Cancel,
    /// 收线。
    Shutdown,
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
    /// 首次丢帧的时刻；队列重新排空后清空。
    overload_since: Option<Instant>,
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
    fn drain(&self) -> Vec<Command> {
        let mut state = self.guard();
        let drained: Vec<Command> = state.commands.drain(..).collect();
        // 队列排空即视为消费方追上生产方，连续超限计时重置。
        state.overload_since = None;
        drained
    }

    fn depth(&self) -> u64 {
        self.guard().commands.len() as u64
    }

    /// 连续超限时长；未超限时为 `None`。
    fn overload_for(&self) -> Option<Duration> {
        self.guard().overload_since.map(|since| since.elapsed())
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
    pub(super) fn cancel(&self) {
        self.queue.push_control(Command::Cancel);
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
    out: tokio_mpsc::UnboundedSender<String>,
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
    let config = WorkerConfig {
        host,
        manifest_sha: manifest_digest(&root),
        session_id: session_id.to_owned(),
    };
    let queue = Arc::new(CommandQueue::new(AUDIO_QUEUE_CAPACITY));
    let worker = Arc::new(VoiceWorker {
        queue: Arc::clone(&queue),
        metrics: Arc::clone(&metrics),
        last_backpressure_at: Mutex::new(None),
    });
    std::thread::spawn(move || run_worker(config, queue, out, warmup, metrics));
    Ok(worker)
}

struct WorkerConfig {
    host: HostConfig,
    manifest_sha: String,
    session_id: String,
}

fn run_worker(
    config: WorkerConfig,
    queue: Arc<CommandQueue>,
    out: tokio_mpsc::UnboundedSender<String>,
    warmup: Duration,
    metrics: Arc<VoiceMetrics>,
) {
    let client = match HostClient::spawn(&config.host) {
        Ok(client) => client,
        Err(_) => {
            metrics.record_error("worker_unavailable");
            let _ = out.send(error_json("worker_unavailable", "arming"));
            return;
        }
    };
    let mut bridge = VoiceBridge::new(client);
    match bridge.warmup("asr", DEFAULT_ASR_MODEL, &config.manifest_sha, warmup) {
        Ok(Some(warm)) => metrics.set_rtf(warm.rtf_p50),
        Ok(None) => {}
        Err(error) => {
            let code = match error {
                BridgeError::Host(error) => error.code,
                _ => "worker_unavailable".to_owned(),
            };
            metrics.record_error(&code);
            let _ = out.send(error_json(&code, "arming"));
            bridge.shutdown();
            return;
        }
    }
    if bridge
        .start(&config.session_id, "full_duplex", "zh", &[])
        .is_err()
    {
        metrics.record_error("protocol_sequence");
        let _ = out.send(error_json("protocol_sequence", "arming"));
        bridge.shutdown();
        return;
    }
    let _ = out.send(state_json("listening"));

    loop {
        for command in queue.drain() {
            let outcome = match command {
                Command::Audio(bytes) => bridge.push_audio(&bytes),
                Command::Stop(utterance_seq) => bridge.end_utterance(utterance_seq),
                Command::Cancel => bridge.cancel("user_cancel"),
                Command::Shutdown => {
                    bridge.shutdown();
                    metrics.session_finished();
                    return;
                }
            };
            if outcome.is_err() {
                metrics.record_error("protocol_sequence");
                let _ = out.send(error_json("protocol_sequence", "listening"));
                bridge.shutdown();
                metrics.session_finished();
                return;
            }
        }
        metrics.set_queue_depth(queue.depth());
        match bridge.poll(WORKER_TICK) {
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
    metrics.session_finished();
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

/// `backpressure` 错误帧：不关连接，带累计丢帧数。
pub(super) fn backpressure_json(frames: u64, bytes: u64) -> String {
    serde_json::json!({
        "type": "error",
        "code": "backpressure",
        "stage": "listening",
        "retryable": true,
        "message": message_for("backpressure"),
        "detail": { "droppedFrames": frames, "droppedBytes": bytes },
    })
    .to_string()
}

pub(super) fn state_json(phase: &str) -> String {
    serde_json::json!({ "type": "state", "phase": phase }).to_string()
}

pub(super) fn message_for(code: &str) -> String {
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

/// 模型根：优先运行态根，否则回落到用户目录下的安装位置。
pub(super) fn model_root() -> PathBuf {
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

pub(super) fn ready_json() -> String {
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

pub(super) fn error_json(code: &str, stage: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::{
        AUDIO_QUEUE_CAPACITY, Command, CommandQueue, PushOutcome, backpressure_json, error_json,
        ready_json,
    };

    fn audio(tag: u8) -> Vec<u8> {
        vec![tag; 4]
    }

    #[test]
    fn queue_drops_oldest_audio_and_keeps_controls() {
        let queue = CommandQueue::new(4);
        for tag in 0..4 {
            assert_eq!(queue.push_audio(audio(tag)), PushOutcome::Queued);
        }
        queue.push_control(Command::Stop(1));
        // 队列已满（4 条音频 + 1 条控制），第 5 帧音频必须丢掉最旧的一帧。
        assert_eq!(
            queue.push_audio(audio(9)),
            PushOutcome::Dropped {
                frames: 1,
                bytes: 4
            }
        );
        let drained = queue.drain();
        let audios: Vec<u8> = drained
            .iter()
            .filter_map(|command| match command {
                Command::Audio(bytes) => bytes.first().copied(),
                _ => None,
            })
            .collect();
        assert_eq!(audios, vec![1, 2, 3, 9], "应丢第 0 帧，保留最新帧");
        assert!(
            drained
                .iter()
                .any(|command| matches!(command, Command::Stop(1))),
            "控制命令不得丢"
        );
        assert_eq!(queue.totals(), (1, 4));
    }

    #[test]
    fn queue_capacity_is_the_documented_value() {
        assert_eq!(AUDIO_QUEUE_CAPACITY, 8);
    }

    #[test]
    fn overload_window_clears_after_drain() {
        let queue = CommandQueue::new(1);
        queue.push_audio(audio(0));
        let dropped = queue.push_audio(audio(1));
        assert!(matches!(dropped, PushOutcome::Dropped { .. }));
        assert!(queue.overload_for().is_some(), "丢帧后应开始计时");
        let _ = queue.drain();
        assert!(queue.overload_for().is_none(), "排空后不再算超限");
    }

    #[test]
    fn backpressure_frame_keeps_the_connection_alive() {
        let value: serde_json::Value =
            serde_json::from_str(&backpressure_json(3, 9_600)).expect("合法 JSON");
        assert_eq!(value["code"], "backpressure");
        assert_eq!(value["stage"], "listening");
        assert_eq!(value["retryable"], true);
        assert_eq!(value["detail"]["droppedFrames"], 3);
        assert_eq!(value["detail"]["droppedBytes"], 9_600);
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
