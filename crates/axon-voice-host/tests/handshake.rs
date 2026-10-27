//! 宿主二进制的端到端契约：握手、错误码、关闭。

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Duration;

use axon_voice_host::BUILD_COMMIT;
use axon_voice_host::supervisor::{HostClient, HostConfig, SupervisorError};
use axon_voice_protocol::{
    Control, ErrorMsg, Frame, MAX_FRAME_BYTES, PROTOCOL_VERSION, Ready, SessionStart, Warmup,
    read_frame, write_bytes, write_frame,
};

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_axon-voice-host")
}

struct RawHost {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl RawHost {
    fn spawn(build_commit: &str) -> Self {
        let mut child = Command::new(exe())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("宿主应能启动");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        let mut host = Self {
            child,
            stdin,
            stdout,
        };
        write_frame(
            &mut host.stdin,
            &Control::Hello(axon_voice_protocol::Hello {
                protocol: PROTOCOL_VERSION,
                build_commit: build_commit.to_owned(),
                role: "daemon".to_owned(),
                pid: std::process::id(),
            }),
        )
        .expect("写 hello");
        host
    }

    fn read(&mut self) -> Frame {
        read_frame(&mut self.stdout)
            .expect("读帧")
            .expect("应有一帧")
    }

    fn code(self) -> i32 {
        let mut child = self.child;
        drop(self.stdin);
        let status = child.wait().expect("等待退出");
        status.code().unwrap_or(-1)
    }
}

#[test]
fn handshake_session_and_close() {
    let mut config = HostConfig::new(exe(), BUILD_COMMIT);
    config.handshake_timeout = Duration::from_secs(5);
    let mut client = HostClient::spawn(&config).expect("握手应成功");

    client
        .send(&Control::Warmup(Warmup {
            family: "asr".to_owned(),
            model: "fixture".to_owned(),
            manifest_sha256: "0".repeat(64),
        }))
        .expect("写 warmup");
    match client.recv_timeout(Duration::from_secs(2)).expect("响应") {
        Frame::Control(Control::Error(ErrorMsg { code, stage, .. })) => {
            assert_eq!(code, "model_missing");
            assert_eq!(stage, "arming");
        }
        other => panic!("期望 model_missing，得到 {other:?}"),
    }

    client
        .send(&Control::SessionStart(SessionStart {
            session_id: "s-1".to_owned(),
            mode: "full_duplex".to_owned(),
            lang: "zh".to_owned(),
            hot_words: vec![],
            sample_rate: 16_000,
        }))
        .expect("写 sessionStart");
    client
        .send(&Control::SessionEnd(axon_voice_protocol::SessionEnd {
            session_id: "s-1".to_owned(),
            reason: "client_close".to_owned(),
        }))
        .expect("写 sessionEnd");

    // 第二个会话验证多会话复用。
    client
        .send(&Control::SessionStart(SessionStart {
            session_id: "s-2".to_owned(),
            mode: "push_to_talk".to_owned(),
            lang: "en".to_owned(),
            hot_words: vec!["lane".to_owned()],
            sample_rate: 16_000,
        }))
        .expect("写第二个 sessionStart");
    client
        .send(&Control::SessionEnd(axon_voice_protocol::SessionEnd {
            session_id: "s-2".to_owned(),
            reason: "idle".to_owned(),
        }))
        .expect("写第二个 sessionEnd");

    let status = client.close().expect("关闭应成功");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn mismatched_build_commit_exits_10() {
    let host = RawHost::spawn("not-the-same-build");
    // 版本不符时宿主不写 ready，直接退出。
    assert_eq!(host.code(), 10);
}

#[test]
fn out_of_order_control_exits_11() {
    let mut host = RawHost::spawn(BUILD_COMMIT);
    assert!(matches!(
        host.read(),
        Frame::Control(Control::Ready(Ready { .. }))
    ));
    write_frame(
        &mut host.stdin,
        &Control::SessionEnd(axon_voice_protocol::SessionEnd {
            session_id: "s-1".to_owned(),
            reason: "client_close".to_owned(),
        }),
    )
    .expect("写 sessionEnd");
    assert_eq!(host.code(), 11);
}

#[test]
fn oversized_control_frame_exits_12() {
    let mut host = RawHost::spawn(BUILD_COMMIT);
    assert!(matches!(
        host.read(),
        Frame::Control(Control::Ready(Ready { .. }))
    ));
    let mut header = vec![0x10u8];
    header.extend_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
    write_bytes(&mut host.stdin, &header).expect("写超限头");
    host.stdin.flush().expect("flush");
    assert_eq!(host.code(), 12);
}

#[test]
fn missing_binary_is_reported_by_supervisor() {
    let config = HostConfig::new("definitely-missing-voice-host", BUILD_COMMIT);
    assert!(matches!(
        HostClient::spawn(&config),
        Err(SupervisorError::Spawn(_))
    ));
}

#[test]
fn audio_before_session_is_rejected() {
    let mut host = RawHost::spawn(BUILD_COMMIT);
    assert!(matches!(
        host.read(),
        Frame::Control(Control::Ready(Ready { .. }))
    ));
    let pcm = [0u8, 0, 1, 0];
    let mut frame = vec![0x03u8];
    frame.extend_from_slice(&(pcm.len() as u32).to_be_bytes());
    frame.extend_from_slice(&pcm);
    write_bytes(&mut host.stdin, &frame).expect("写音频");
    assert_eq!(host.code(), 11);
}

#[test]
fn closed_process_stops_reading() {
    let mut host = RawHost::spawn(BUILD_COMMIT);
    let _ = host.read();
    write_frame(&mut host.stdin, &Control::Close).expect("写 close");
    match host.read() {
        Frame::Control(Control::Closed) => {}
        other => panic!("期望 closed，得到 {other:?}"),
    }
    drop(host.stdout);
    let status = host.child.wait().expect("等待退出");
    assert_eq!(status.code(), Some(0));
}
#[test]
fn invalid_cancel_reason_exits_11() {
    let mut host = RawHost::spawn(BUILD_COMMIT);
    assert!(matches!(
        host.read(),
        Frame::Control(Control::Ready(Ready { .. }))
    ));
    write_frame(
        &mut host.stdin,
        &Control::Cancel(axon_voice_protocol::Cancel {
            reason: "not-a-reason".to_owned(),
        }),
    )
    .expect("写 cancel");
    assert_eq!(host.code(), 11);
}

#[test]
fn speak_without_session_exits_11() {
    let mut host = RawHost::spawn(BUILD_COMMIT);
    assert!(matches!(
        host.read(),
        Frame::Control(Control::Ready(Ready { .. }))
    ));
    write_frame(
        &mut host.stdin,
        &Control::Speak(axon_voice_protocol::Speak {
            turn_id: "t".to_owned(),
            epoch: 1,
            seq: 1,
            text: "好".to_owned(),
        }),
    )
    .expect("写 speak");
    assert_eq!(host.code(), 11);
}
