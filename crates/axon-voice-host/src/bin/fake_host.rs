//! 测试用假宿主：只实现握手与会话骨架，用于验证监督闭环。
//!
//! 仅在 `fake-host` feature 下构建，不进入发布产物。
//! 模式由环境变量 `AXON_FAKE_HOST_MODE` 选择：`silent`（握手后不再发心跳）、
//! `crash`（握手后立即退出 1）、`multi`（正常心跳并接受多会话）。

use std::io::{BufReader, BufWriter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use axon_voice_host::BUILD_COMMIT;
use axon_voice_protocol::{
    Control, Frame, Heartbeat, PROTOCOL_VERSION, Ready, read_frame, write_frame,
};

const HEARTBEAT: Duration = Duration::from_secs(1);

fn main() {
    let mode = std::env::var("AXON_FAKE_HOST_MODE").unwrap_or_else(|_| "multi".to_owned());
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let out = Arc::new(Mutex::new(BufWriter::new(std::io::stdout())));
    if !matches!(
        read_frame(&mut reader),
        Ok(Some(Frame::Control(Control::Hello(_))))
    ) {
        std::process::exit(10);
    }
    let ready = Control::Ready(Ready {
        protocol: PROTOCOL_VERSION,
        build_commit: BUILD_COMMIT.to_owned(),
        host_version: "fake".to_owned(),
    });
    if write(&out, &ready).is_err() {
        return;
    }
    match mode.as_str() {
        "crash" => std::process::exit(1),
        "silent" => {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        _ => {}
    }

    let stop = Arc::new(AtomicBool::new(false));
    let beat_out = Arc::clone(&out);
    let beat_stop = Arc::clone(&stop);
    thread::spawn(move || {
        while !beat_stop.load(Ordering::SeqCst) {
            thread::sleep(HEARTBEAT);
            if beat_stop.load(Ordering::SeqCst) {
                break;
            }
            let beat = Control::Heartbeat(Heartbeat {
                pid: std::process::id(),
                rss_kb: 0,
                model_state: "warm".to_owned(),
                queue_in: 0,
                queue_out: 0,
            });
            if write(&beat_out, &beat).is_err() {
                break;
            }
        }
    });

    let mut session: Option<String> = None;
    loop {
        match read_frame(&mut reader) {
            Ok(Some(Frame::Control(Control::SessionStart(message)))) if session.is_none() => {
                session = Some(message.session_id);
            }
            Ok(Some(Frame::Control(Control::SessionEnd(message))))
                if session.as_deref() == Some(message.session_id.as_str()) =>
            {
                session = None;
            }
            Ok(Some(Frame::Control(Control::Close))) => {
                stop.store(true, Ordering::SeqCst);
                let _ = write(&out, &Control::Closed);
                return;
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => std::process::exit(11),
        }
    }
    stop.store(true, Ordering::SeqCst);
}

fn write(out: &Arc<Mutex<BufWriter<std::io::Stdout>>>, control: &Control) -> Result<(), ()> {
    let mut guard = out.lock().map_err(|_| ())?;
    write_frame(&mut *guard, control).map_err(|_| ())
}
