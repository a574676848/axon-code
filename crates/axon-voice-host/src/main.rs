//! 宿主进程：握手、模型校验、本地推理、会话与关闭。
//!
//! 命令行只接受 `--model-root <dir>`；构建号由 `build.rs` 注入。
//! 识别与合成通过运行态根下的 sherpa-onnx 命令行完成，不依赖 FFI。
//! 回执队列容量为 1，队列已满立即退出 12，避免父进程不读时无限排队。

use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::exit_code;
use axon_voice_host::inference::Inference;
use axon_voice_host::model::{ModelFamily, ModelResolution, ModelStore};
use axon_voice_protocol::{
    Control, ErrorDetail, ErrorMsg, Final, Frame, FrameError, Heartbeat, Hello, PROTOCOL_VERSION,
    Ready, Warm, Warmup, encode_audio_out, read_frame, write_frame,
};

/// 心跳间隔。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// 回执队列容量；满即退出，保证控制不被音频拖死。
const OUTBOX_CAPACITY: usize = 1;
/// 单会话音频上限（60 s）。
const MAX_SESSION_SAMPLES: usize = 16_000 * 60;
/// 每个下行音频帧的样本数（200 ms）。
const AUDIO_CHUNK_SAMPLES: usize = 3_200;

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
    samples: Vec<i16>,
    started: Instant,
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

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
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
    let heartbeat = spawn_heartbeat(outbox.clone(), Arc::clone(&stop), Arc::clone(&warm));
    let outcome = serve(&mut reader, &outbox, store.as_ref(), &warm);
    stop.store(true, Ordering::SeqCst);
    if let Err(code) = outcome {
        // writer 可能正阻塞在 stdout，不能 join；由进程退出兜底。
        std::process::exit(i32::from(code));
    }
    drop(outbox);
    let _ = writer.join();
    heartbeat.join().ok();
    Ok(exit_code::OK as u8)
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
                queue_in: 0,
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

fn serve(
    reader: &mut impl io::BufRead,
    outbox: &SyncSender<Outgoing>,
    store: Option<&ModelStore>,
    warm: &Arc<AtomicBool>,
) -> Result<u8, u8> {
    let mut session: Option<SessionState> = None;
    let mut runtime: Option<Inference> = None;
    loop {
        let frame = match read_frame(reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(exit_code::OK as u8),
            Err(error) => return Err(map_frame_error(&error)),
        };
        let control = match frame {
            Frame::Control(control) => control,
            Frame::AudioIn(payload) => {
                let Some(state) = session.as_mut() else {
                    return Err(exit_code::SEQUENCE as u8);
                };
                for chunk in payload.as_chunks::<2>().0 {
                    state.samples.push(i16::from_le_bytes(*chunk));
                }
                if state.samples.len() > MAX_SESSION_SAMPLES {
                    return Err(exit_code::FRAME as u8);
                }
                continue;
            }
            Frame::AudioOut { .. } | Frame::CueAudio { .. } => {
                return Err(exit_code::SEQUENCE as u8);
            }
        };
        match control {
            Control::Hello(_) | Control::Ready(_) => return Err(exit_code::SEQUENCE as u8),
            Control::Warmup(message) => {
                if session.is_some() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                let reply = warmup_reply(store, &message, &mut runtime);
                if matches!(reply, Control::Warm(_)) {
                    warm.store(true, Ordering::SeqCst);
                }
                send(outbox, reply);
            }
            Control::SessionStart(message) => {
                if session.is_some() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                session = Some(SessionState {
                    id: message.session_id,
                    samples: Vec::new(),
                    started: Instant::now(),
                });
            }
            Control::SessionEnd(message) => {
                let Some(state) = session.take() else {
                    return Err(exit_code::SEQUENCE as u8);
                };
                if state.id != message.session_id {
                    return Err(exit_code::SEQUENCE as u8);
                }
                let end_ms = state.started.elapsed().as_millis() as u64;
                if let (Some(runtime), false) = (&runtime, state.samples.is_empty()) {
                    match runtime.transcribe(&state.samples) {
                        Ok(text) if !text.is_empty() => {
                            let finalize_ms = state.started.elapsed().as_millis() as u64;
                            send(
                                outbox,
                                Control::Final(Final {
                                    utterance_seq: 1,
                                    text,
                                    start_ms: 0,
                                    end_ms,
                                    finalize_ms,
                                }),
                            );
                        }
                        Ok(_) => {}
                        Err(error) => send(outbox, unusable(Some("asr"), &error)),
                    }
                }
            }
            Control::Cancel(_) => {
                if let Some(state) = session.as_mut() {
                    state.samples.clear();
                }
            }
            Control::Speak(message) => {
                if session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                let Some(runtime) = runtime.as_ref() else {
                    send(outbox, simple_error("tts_failed", "speaking", true));
                    continue;
                };
                match runtime.synthesize(&message.text) {
                    Ok(samples) => send_audio(outbox, message.epoch, &samples),
                    Err(_) => send(outbox, simple_error("tts_failed", "speaking", true)),
                }
            }
            Control::CueSpeak(_) => {
                if session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                send(outbox, simple_error("cue_unavailable", "speaking", false));
            }
            Control::Close => {
                send(outbox, Control::Closed);
                return Ok(exit_code::OK as u8);
            }
            Control::Warm(_)
            | Control::Partial(_)
            | Control::Final(_)
            | Control::CueStatus(_)
            | Control::Heartbeat(_)
            | Control::Error(_)
            | Control::Closed => return Err(exit_code::SEQUENCE as u8),
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

fn warmup_reply(
    store: Option<&ModelStore>,
    message: &Warmup,
    runtime: &mut Option<Inference>,
) -> Control {
    let Some(family) = ModelFamily::parse(&message.family) else {
        return unusable(None, "cue 模型预热尚未接入");
    };
    let Some(store) = store else {
        return missing(None, "使用 --model-root 指定模型目录");
    };
    match store.resolve_verified(family, &message.model, &message.manifest_sha256) {
        ModelResolution::Ready { .. } => {
            if runtime.is_none() {
                *runtime = Inference::discover(store.root());
            }
            let Some(runtime) = runtime.as_ref() else {
                return unusable(Some(&message.family), "语音运行时未安装");
            };
            match runtime.warm_probe() {
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
