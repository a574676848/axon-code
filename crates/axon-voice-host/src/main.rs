//! 宿主进程：严格握手、单活跃会话、心跳、关闭确认。
//!
//! 不接受命令行参数；构建号由 `build.rs` 注入。回执队列容量为 1，
//! 队列已满即按控制通道溢出退出 12，避免父进程不读时无限排队。

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::exit_code;
use axon_voice_protocol::{
    Control, ErrorDetail, ErrorMsg, Frame, FrameError, Heartbeat, Hello, PROTOCOL_VERSION, Ready,
    Warmup, read_frame, write_frame,
};

/// 心跳间隔。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
/// 回执队列容量；满即退出，保证控制不被音频拖死。
const OUTBOX_CAPACITY: usize = 1;

fn main() -> ExitCode {
    if std::env::args().nth(1).is_some() {
        eprintln!("axon-voice-host 不接受命令行参数");
        return ExitCode::from(exit_code::HANDSHAKE as u8);
    }
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(code) => ExitCode::from(code),
    }
}

fn run() -> Result<u8, u8> {
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
    send(&outbox, ready())?;

    let stop = Arc::new(AtomicBool::new(false));
    let heartbeat = spawn_heartbeat(outbox.clone(), Arc::clone(&stop));
    let outcome = serve(&mut reader, &outbox);
    stop.store(true, Ordering::SeqCst);
    drop(outbox);
    let _ = writer.join();
    heartbeat.join().ok();
    outcome
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

fn serve(reader: &mut impl io::BufRead, outbox: &SyncSender<Control>) -> Result<u8, u8> {
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
            Control::Warmup(message) => send(outbox, model_missing(&message))?,
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
                send(outbox, simple_error("tts_failed", "speaking"))?;
            }
            Control::CueSpeak(_) => {
                if session.is_none() {
                    return Err(exit_code::SEQUENCE as u8);
                }
                send(outbox, simple_error("cue_unavailable", "speaking"))?;
            }
            Control::Close => {
                send(outbox, Control::Closed)?;
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

fn model_missing(message: &Warmup) -> Control {
    Control::Error(ErrorMsg {
        code: "model_missing".to_owned(),
        stage: "arming".to_owned(),
        retryable: true,
        detail: Some(ErrorDetail {
            family: Some(message.family.clone()),
            ..ErrorDetail::default()
        }),
    })
}

fn simple_error(code: &str, stage: &str) -> Control {
    Control::Error(ErrorMsg {
        code: code.to_owned(),
        stage: stage.to_owned(),
        retryable: true,
        detail: None,
    })
}

fn send(outbox: &SyncSender<Control>, control: Control) -> Result<(), u8> {
    match outbox.try_send(control) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(exit_code::FRAME as u8),
        Err(TrySendError::Disconnected(_)) => Err(exit_code::SEQUENCE as u8),
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
