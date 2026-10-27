//! 宿主进程：握手、模型校验、本地推理、会话与关闭。
//!
//! 命令行只接受 `--model-root <dir>`；构建号由 `build.rs` 注入。
//! 识别走常驻的 sherpa-onnx 在线识别服务（模型只加载一次，中间结果实时上抛），
//! 合成仍走一次性命令行。回执队列按流式事件的节奏给到 64 帧，队列满即退出 12，
//! 避免父进程不读时无限排队。

use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use axon_voice::Verbosity;
use axon_voice::render::render_for_speech;
use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::exit_code;
use axon_voice_host::inference::Inference;
use axon_voice_host::model::{ModelFamily, ModelResolution, ModelStore};
use axon_voice_host::streaming::{AsrEvent, StreamingAsr};
use axon_voice_protocol::{
    Control, ErrorDetail, ErrorMsg, Final, Frame, FrameError, Heartbeat, Hello, PROTOCOL_VERSION,
    Partial, Ready, Warm, Warmup, encode_audio_out, read_frame, write_frame,
};

/// 心跳间隔。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// 回执队列容量。
///
/// 流式识别会把 partial 当事件持续上抛，不再是「一个请求一个回执」的节奏；
/// 容量给到 64 帧（JSON 都很小）足以吸收父进程一次轮询周期的抖动。队列满即退出 12，
/// 保持「父进程不读就不能假装正常」的失败语义。
const OUTBOX_CAPACITY: usize = 64;
/// 单会话音频上限（120 s，与 daemon 的会话时长一致）。
const MAX_SESSION_SAMPLES: usize = 16_000 * 120;
/// 每个下行音频帧的样本数（200 ms）。
const AUDIO_CHUNK_SAMPLES: usize = 3_200;
/// 入站帧队列容量；满了说明主循环卡住，按溢出处理。
const INBOX_CAPACITY: usize = 32;
/// 主循环空转休眠。
const IDLE_TICK: Duration = Duration::from_millis(4);
/// 识别服务轮询预算。
const ASR_POLL: Duration = Duration::from_millis(4);

#[derive(Debug, Default)]
struct Options {
    model_root: Option<PathBuf>,
}

/// 出站消息：控制帧或原始音频帧。
enum Outgoing {
    Control(Control),
    Audio(Vec<u8>),
}

struct SessionState {
    id: String,
    /// 已推入的采样数；用于单会话上限，不再整段落盘。
    samples_pushed: usize,
    /// 是否已就「没有识别服务」告警过一次；避免每帧刷错误。
    degraded_notified: bool,
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(options) => options,
        Err(code) => return ExitCode::from(code),
    };
    match run(options) {
        Ok(code) => ExitCode::from(code),
        Err(code) => ExitCode::from(code),
    }
}

fn parse_args() -> Result<Options, u8> {
    let mut args = std::env::args().skip(1);
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--model-root" => {
                let Some(value) = args.next() else {
                    eprintln!("--model-root 缺少目录参数");
                    return Err(exit_code::MODEL as u8);
                };
                if options.model_root.is_some() {
                    eprintln!("--model-root 只能出现一次");
                    return Err(exit_code::HANDSHAKE as u8);
                }
                options.model_root = Some(PathBuf::from(value));
            }
            other => {
                eprintln!("不支持的参数：{other}");
                return Err(exit_code::HANDSHAKE as u8);
            }
        }
    }
    Ok(options)
}

fn run(options: Options) -> Result<u8, u8> {
    let store = match options.model_root {
        Some(root) => {
            let store = ModelStore::new(root);
            if store.preflight().is_err() {
                return Err(exit_code::MODEL as u8);
            }
            Some(store)
        }
        None => None,
    };

    let (outbox, inbox) = mpsc::sync_channel::<Outgoing>(OUTBOX_CAPACITY);
    let writer = thread::spawn(move || writer_loop(inbox));

    // 读句柄要移交给收帧线程，因此用不持锁的 `Stdin` 而不是 `StdinLock`。
    let mut reader = BufReader::new(io::stdin());
    let hello = match read_frame(&mut reader) {
        Ok(Some(Frame::Control(Control::Hello(hello)))) => hello,
        Ok(_) => return Err(exit_code::SEQUENCE as u8),
        Err(error) => return Err(map_frame_error(&error)),
    };
    if !valid_hello(&hello) {
        return Err(exit_code::HANDSHAKE as u8);
    }
    send(&outbox, ready());

    let stop = Arc::new(AtomicBool::new(false));
    let warm = Arc::new(AtomicBool::new(false));
    let queue_in = Arc::new(AtomicUsize::new(0));
    let heartbeat = spawn_heartbeat(
        outbox.clone(),
        Arc::clone(&stop),
        Arc::clone(&warm),
        Arc::clone(&queue_in),
    );
    // 识别的中间结果要随音频实时上抛，主循环不能阻塞在 stdin 上，
    // 因此入站帧由独立线程读入有界队列。
    let (frames_tx, frames_rx) = mpsc::sync_channel::<Inbound>(INBOX_CAPACITY);
    let depth = Arc::clone(&queue_in);
    thread::spawn(move || read_frames(&mut reader, &frames_tx, &depth));
    let outcome = serve(&frames_rx, &outbox, store.as_ref(), &warm, &queue_in);
    stop.store(true, Ordering::SeqCst);
    if let Err(code) = outcome {
        // writer 可能正阻塞在 stdout，不能 join；由进程退出兜底。
        std::process::exit(i32::from(code));
    }
    drop(outbox);
    let _ = writer.join();
    heartbeat.join().ok();
    Ok(outcome.unwrap_or(exit_code::OK as u8))
}

fn valid_hello(hello: &Hello) -> bool {
    hello.protocol == PROTOCOL_VERSION
        && hello.build_commit == BUILD_COMMIT
        && hello.role == "daemon"
}

fn ready() -> Control {
    Control::Ready(Ready {
        protocol: PROTOCOL_VERSION,
        build_commit: BUILD_COMMIT.to_owned(),
        host_version: env!("CARGO_PKG_VERSION").to_owned(),
    })
}

fn spawn_heartbeat(
    outbox: SyncSender<Outgoing>,
    stop: Arc<AtomicBool>,
    warm: Arc<AtomicBool>,
    queue_in: Arc<AtomicUsize>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(HEARTBEAT_INTERVAL);
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let beat = Control::Heartbeat(Heartbeat {
                pid: std::process::id(),
                rss_kb: resident_kb(),
                model_state: if warm.load(Ordering::SeqCst) {
                    "warm".to_owned()
                } else {
                    "cold".to_owned()
                },
                queue_in: queue_in.load(Ordering::SeqCst) as u32,
                queue_out: 0,
            });
            match outbox.try_send(Outgoing::Control(beat)) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => std::process::exit(exit_code::FRAME),
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
    })
}

/// 常驻内存；平台未提供读取方式时返回 0。
fn resident_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            let pages = statm
                .split_whitespace()
                .nth(1)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            return pages.saturating_mul(4);
        }
    }
    0
}

/// 读线程交来的入站帧；`Err` 是用退出码表达的协议或 IO 失败。
type Inbound = Result<Frame, u8>;

/// 控制帧处理结果。
enum Flow {
    /// 继续服务。
    Continue,
    /// 按退出码收线。
    Exit(u8),
}

/// 宿主运行态：会话、运行时与常驻识别服务。
struct HostState {
    session: Option<SessionState>,
    runtime: Option<Inference>,
    asr: Option<StreamingAsr>,
}

impl HostState {
    fn new() -> Self {
        Self {
            session: None,
            runtime: None,
            asr: None,
        }
    }

    /// 处理一帧。
    fn handle(
        &mut self,
        frame: Frame,
        outbox: &SyncSender<Outgoing>,
        store: Option<&ModelStore>,
        warm: &Arc<AtomicBool>,
    ) -> Result<Flow, u8> {
        let control = match frame {
            Frame::AudioIn(payload) => return self.on_audio(&payload, outbox),
            Frame::AudioOut { .. } | Frame::CueAudio { .. } => {
                return Err(exit_code::SEQUENCE as u8);
            }
            Frame::Control(control) => control,
        };
        match control {
            Control::Hello(_) | Control::Ready(_) => Err(exit_code::SEQUENCE as u8),
            Control::Warmup(message) => {
                if self.session.is_some() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                let reply = self.warm_up(store, &message);
                if matches!(reply, Control::Warm(_)) {
                    warm.store(true, Ordering::SeqCst);
                }
                send(outbox, reply);
                Ok(Flow::Continue)
            }
            Control::SessionStart(message) => {
                if self.session.is_some() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                // 没预热成功时不在会话首帧就回错：daemon 已在 warmup 阶段拿到
                // `model_missing`/`model_unusable`；这里等真的有音频进来再告警一次。
                self.session = Some(SessionState {
                    id: message.session_id,
                    samples_pushed: 0,
                    degraded_notified: false,
                });
                Ok(Flow::Continue)
            }
            Control::UtteranceEnd(_) => {
                self.flush(outbox, "listening");
                Ok(Flow::Continue)
            }
            Control::SessionEnd(message) => {
                let Some(session) = self.session.as_ref() else {
                    return Err(exit_code::SEQUENCE as u8);
                };
                if session.id != message.session_id {
                    return Err(exit_code::SEQUENCE as u8);
                }
                self.flush(outbox, "listening");
                self.session = None;
                if let Some(asr) = self.asr.as_mut()
                    && asr.reset().is_err()
                {
                    self.asr = None;
                    send(outbox, simple_error("worker_crashed", "shutdown", true));
                }
                Ok(Flow::Continue)
            }
            Control::Cancel(_) => {
                if let Some(asr) = self.asr.as_mut() {
                    let _ = asr.reset();
                }
                Ok(Flow::Continue)
            }
            Control::Speak(message) => {
                if self.session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                self.speak(&message, outbox);
                Ok(Flow::Continue)
            }
            Control::CueSpeak(_) => {
                if self.session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                send(outbox, simple_error("cue_unavailable", "speaking", false));
                Ok(Flow::Continue)
            }
            Control::Close => {
                send(outbox, Control::Closed);
                Ok(Flow::Exit(exit_code::OK as u8))
            }
            Control::Warm(_)
            | Control::Partial(_)
            | Control::Final(_)
            | Control::CueStatus(_)
            | Control::Heartbeat(_)
            | Control::Error(_)
            | Control::Closed => Err(exit_code::SEQUENCE as u8),
        }
    }

    fn on_audio(&mut self, payload: &[u8], outbox: &SyncSender<Outgoing>) -> Result<Flow, u8> {
        let over_limit = {
            let Some(session) = self.session.as_mut() else {
                return Err(exit_code::SEQUENCE as u8);
            };
            session.samples_pushed += payload.len() / 2;
            session.samples_pushed > MAX_SESSION_SAMPLES
        };
        if over_limit {
            return Err(exit_code::FRAME as u8);
        }
        let Some(asr) = self.asr.as_mut() else {
            let notify = self
                .session
                .as_mut()
                .is_some_and(|session| !session.degraded_notified);
            if notify {
                if let Some(session) = self.session.as_mut() {
                    session.degraded_notified = true;
                }
                send(
                    outbox,
                    simple_error("worker_unavailable", "listening", true),
                );
            }
            return Ok(Flow::Continue);
        };
        if asr.push_audio(payload).is_err() {
            self.asr = None;
            send(outbox, simple_error("worker_crashed", "listening", true));
        }
        Ok(Flow::Continue)
    }

    /// 轮询识别事件；返回是否产生了新帧。
    fn poll_asr(&mut self, outbox: &SyncSender<Outgoing>) -> bool {
        let outcome = match self.asr.as_mut() {
            Some(asr) => asr.poll(ASR_POLL),
            None => return false,
        };
        match outcome {
            Ok(events) => {
                let produced = !events.is_empty();
                for event in events {
                    emit_event(outbox, event);
                }
                produced
            }
            Err(_) => {
                self.asr = None;
                send(outbox, simple_error("worker_crashed", "listening", true));
                true
            }
        }
    }

    /// 让识别服务把当前句定稿并上抛 final。
    fn flush(&mut self, outbox: &SyncSender<Outgoing>, stage: &str) {
        let outcome = match self.asr.as_mut() {
            Some(asr) => asr.finish_utterance(),
            None => return,
        };
        match outcome {
            Ok(events) => {
                for event in events {
                    emit_event(outbox, event);
                }
            }
            Err(_) => {
                self.asr = None;
                send(outbox, simple_error("worker_crashed", stage, true));
            }
        }
    }

    fn warm_up(&mut self, store: Option<&ModelStore>, message: &Warmup) -> Control {
        let Some(family) = ModelFamily::parse(&message.family) else {
            return unusable(None, "cue 模型预热尚未接入");
        };
        let Some(store) = store else {
            return missing(None, "使用 --model-root 指定模型目录");
        };
        match store.resolve_verified(family, &message.model, &message.manifest_sha256) {
            ModelResolution::Ready { .. } => {
                if self.runtime.is_none() {
                    self.runtime = Inference::discover(store.root());
                }
                let Some(runtime) = self.runtime.as_ref() else {
                    return unusable(Some(&message.family), "语音运行时未安装");
                };
                if self.asr.is_none() {
                    self.asr = match StreamingAsr::start(runtime) {
                        Ok(asr) => Some(asr),
                        Err(reason) => return unusable(Some(&message.family), &reason),
                    };
                }
                let Some(asr) = self.asr.as_mut() else {
                    return unusable(Some(&message.family), "识别服务未就绪");
                };
                match asr.measure_rtf() {
                    Ok(rtf) => Control::Warm(Warm {
                        family: message.family.clone(),
                        model: message.model.clone(),
                        rtf_p50: rtf,
                        rss_kb: resident_kb(),
                    }),
                    Err(reason) => unusable(Some(&message.family), &reason),
                }
            }
            ModelResolution::Missing => missing(
                Some(&message.family),
                &format!(
                    "安装到 {}",
                    store.model_dir(family, &message.model).display()
                ),
            ),
            ModelResolution::Unusable { reason } => unusable(Some(&message.family), &reason),
        }
    }

    fn speak(&mut self, message: &axon_voice_protocol::Speak, outbox: &SyncSender<Outgoing>) {
        let Some(runtime) = self.runtime.as_ref() else {
            send(outbox, simple_error("tts_failed", "speaking", true));
            return;
        };
        // 防御性二次渲染：正常路径由 daemon 产出朗读稿，宿主不再直读正文。
        let spoken = render_for_speech(&message.text, Verbosity::Concise);
        if spoken.trim().is_empty() {
            send(outbox, simple_error("tts_failed", "speaking", true));
            return;
        }
        match runtime.synthesize(&spoken) {
            Ok(samples) => send_audio(outbox, message.epoch, &samples),
            Err(_) => send(outbox, simple_error("tts_failed", "speaking", true)),
        }
    }
}

/// 从 stdin 持续读帧并投递到有界队列。
fn read_frames(reader: &mut impl io::BufRead, frames: &SyncSender<Inbound>, depth: &AtomicUsize) {
    loop {
        let inbound = match read_frame(reader) {
            Ok(Some(frame)) => Ok(frame),
            Ok(None) => return,
            Err(error) => {
                let _ = frames.send(Err(map_frame_error(&error)));
                return;
            }
        };
        depth.fetch_add(1, Ordering::SeqCst);
        if frames.send(inbound).is_err() {
            depth.fetch_sub(1, Ordering::SeqCst);
            return;
        }
    }
}

fn emit_event(outbox: &SyncSender<Outgoing>, event: AsrEvent) {
    let control = match event {
        AsrEvent::Partial {
            utterance_seq,
            text,
        } => Control::Partial(Partial {
            utterance_seq,
            text,
        }),
        AsrEvent::Final {
            utterance_seq,
            text,
            start_ms,
            end_ms,
            finalize_ms,
        } => Control::Final(Final {
            utterance_seq,
            text,
            start_ms,
            end_ms,
            finalize_ms,
        }),
    };
    send(outbox, control);
}

/// 主循环：先清空入站帧，再轮询识别服务，都没有活时短暂休眠。
fn serve(
    frames: &Receiver<Inbound>,
    outbox: &SyncSender<Outgoing>,
    store: Option<&ModelStore>,
    warm: &Arc<AtomicBool>,
    depth: &AtomicUsize,
) -> Result<u8, u8> {
    let mut state = HostState::new();
    loop {
        let mut busy = false;
        loop {
            let inbound = match frames.try_recv() {
                Ok(inbound) => inbound,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(exit_code::OK as u8),
            };
            busy = true;
            depth.fetch_sub(1, Ordering::SeqCst);
            let frame = inbound?;
            match state.handle(frame, outbox, store, warm)? {
                Flow::Continue => {}
                Flow::Exit(code) => return Ok(code),
            }
        }
        if state.poll_asr(outbox) {
            busy = true;
        }
        if !busy {
            thread::sleep(IDLE_TICK);
        }
    }
}

fn send_audio(outbox: &SyncSender<Outgoing>, epoch: u32, samples: &[i16]) {
    let mut seq = 1u32;
    for chunk in samples.chunks(AUDIO_CHUNK_SAMPLES) {
        let mut bytes = Vec::with_capacity(chunk.len() * 2);
        for sample in chunk {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        match encode_audio_out(epoch, seq, &bytes) {
            Ok(frame) => {
                let _ = outbox.try_send(Outgoing::Audio(frame));
                seq += 1;
            }
            Err(_) => break,
        }
    }
}

fn missing(family: Option<&str>, hint: &str) -> Control {
    Control::Error(ErrorMsg {
        code: "model_missing".to_owned(),
        stage: "arming".to_owned(),
        retryable: true,
        detail: Some(ErrorDetail {
            family: family.map(str::to_owned),
            reason: Some(hint.to_owned()),
            ..ErrorDetail::default()
        }),
    })
}

fn unusable(family: Option<&str>, reason: &str) -> Control {
    Control::Error(ErrorMsg {
        code: "model_unusable".to_owned(),
        stage: "arming".to_owned(),
        retryable: false,
        detail: Some(ErrorDetail {
            family: family.map(str::to_owned),
            reason: Some(reason.to_owned()),
            ..ErrorDetail::default()
        }),
    })
}

fn simple_error(code: &str, stage: &str, retryable: bool) -> Control {
    Control::Error(ErrorMsg {
        code: code.to_owned(),
        stage: stage.to_owned(),
        retryable,
        detail: None,
    })
}

fn send(outbox: &SyncSender<Outgoing>, control: Control) {
    match outbox.try_send(Outgoing::Control(control)) {
        Ok(()) => {}
        // writer 阻塞在 stdout 时不能等它，直接按控制通道溢出退出。
        Err(TrySendError::Full(_)) => std::process::exit(exit_code::FRAME),
        Err(TrySendError::Disconnected(_)) => std::process::exit(exit_code::SEQUENCE),
    }
}

fn writer_loop(inbox: Receiver<Outgoing>) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    while let Ok(control) = inbox.recv() {
        let result = match control {
            Outgoing::Control(control) => write_frame(&mut writer, &control),
            Outgoing::Audio(frame) => {
                use std::io::Write;
                writer
                    .write_all(&frame)
                    .and_then(|()| writer.flush())
                    .map_err(FrameError::Io)
            }
        };
        if result.is_err() {
            break;
        }
    }
}

fn map_frame_error(error: &FrameError) -> u8 {
    match error {
        FrameError::TooLarge { .. } => exit_code::FRAME as u8,
        FrameError::Io(_)
        | FrameError::OddAudioLength
        | FrameError::Truncated
        | FrameError::UnknownKind(_)
        | FrameError::InvalidJson(_)
        | FrameError::UnknownType(_)
        | FrameError::UnknownField(_)
        | FrameError::InvalidValue { .. }
        | FrameError::Serialize(_) => exit_code::SEQUENCE as u8,
    }
}
