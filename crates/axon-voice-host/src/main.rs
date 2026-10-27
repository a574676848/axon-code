//! 宿主进程：严格握手、模型预检、单活跃会话、心跳与关闭确认。
//!
//! 命令行只接受 `--model-root <dir>`；构建号由 `build.rs` 注入。
//! 回执队列容量为 1，队列已满立即退出 12，避免父进程不读时无限排队。
//! 本地推理尚未接入：清单与文件校验通过后仍回 `model_unusable`（原因注明运行时未接入），
//! 不伪造 `warm`；运行时接入在 Stage 4。

use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::exit_code;
use axon_voice_host::model::{ModelFamily, ModelResolution, ModelStore};
use axon_voice_protocol::{
    Control, ErrorDetail, ErrorMsg, Frame, FrameError, Heartbeat, Hello, PROTOCOL_VERSION, Ready,
    Warmup, read_frame, write_frame,
};

/// 心跳间隔。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// 回执队列容量；满即退出，保证控制不被音频拖死。
const OUTBOX_CAPACITY: usize = 1;

#[derive(Debug, Default)]
struct Options {
    model_root: Option<PathBuf>,
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

    let (outbox, inbox) = mpsc::sync_channel::<Control>(OUTBOX_CAPACITY);
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
    let heartbeat = spawn_heartbeat(outbox.clone(), Arc::clone(&stop));
    let outcome = serve(&mut reader, &outbox, store.as_ref());
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

fn spawn_heartbeat(outbox: SyncSender<Control>, stop: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(HEARTBEAT_INTERVAL);
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let beat = Control::Heartbeat(Heartbeat {
                pid: std::process::id(),
                rss_kb: resident_kb(),
                // 推理未接入，心跳始终如实上报 cold。
                model_state: "cold".to_owned(),
                queue_in: 0,
                queue_out: 0,
            });
            match outbox.try_send(beat) {
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
    outbox: &SyncSender<Control>,
    store: Option<&ModelStore>,
) -> Result<u8, u8> {
    let mut session: Option<String> = None;
    loop {
        let frame = match read_frame(reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(exit_code::OK as u8),
            Err(error) => return Err(map_frame_error(&error)),
        };
        let control = match frame {
            Frame::Control(control) => control,
            Frame::AudioIn(_) => {
                if session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
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
                let reply = warmup_reply(store, &message);
                send(outbox, reply);
            }
            Control::SessionStart(message) => {
                if session.is_some() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                session = Some(message.session_id);
            }
            Control::SessionEnd(message) => {
                if session.as_deref() != Some(message.session_id.as_str()) {
                    return Err(exit_code::SEQUENCE as u8);
                }
                session = None;
            }
            Control::Cancel(_) => {}
            Control::Speak(_) => {
                if session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                send(outbox, simple_error("tts_failed", "speaking", true));
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

fn warmup_reply(store: Option<&ModelStore>, message: &Warmup) -> Control {
    let Some(family) = ModelFamily::parse(&message.family) else {
        return unusable(None, "cue 模型在 Stage 4 接入");
    };
    let Some(store) = store else {
        return missing(None, "使用 --model-root 指定模型目录");
    };
    match store.resolve_verified(family, &message.model, &message.manifest_sha256) {
        ModelResolution::Ready { .. } => unusable(
            Some(&message.family),
            "运行时尚未接入，无法完成预热（Stage 4 接入）",
        ),
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

fn send(outbox: &SyncSender<Control>, control: Control) {
    match outbox.try_send(control) {
        Ok(()) => {}
        // writer 阻塞在 stdout 时不能等它，直接按控制通道溢出退出。
        Err(TrySendError::Full(_)) => std::process::exit(exit_code::FRAME),
        Err(TrySendError::Disconnected(_)) => std::process::exit(exit_code::SEQUENCE),
    }
}

fn writer_loop(inbox: Receiver<Control>) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    while let Ok(control) = inbox.recv() {
        if write_frame(&mut writer, &control).is_err() {
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
