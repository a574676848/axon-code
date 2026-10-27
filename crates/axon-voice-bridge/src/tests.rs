//! 桥的状态机测试：用进程内脚本宿主验证编排，不启动真实进程。

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use axon_voice_protocol::{Control, ErrorDetail, ErrorMsg, Final, Frame, Heartbeat, Partial, Warm};

use super::{BridgeError, BridgeState, HostTransport, TransportError, VoiceBridge, VoiceEvent};

#[derive(Clone, Copy)]
enum Mode {
    Warm,
    Reject,
    Silent,
}

struct ScriptedHost {
    mode: Mode,
    replies_tx: Sender<Frame>,
    replies: Receiver<Frame>,
    _received: Receiver<Control>,
}

impl ScriptedHost {
    fn new(mode: Mode) -> Self {
        let (replies_tx, replies) = mpsc::channel();
        let (received_tx, received) = mpsc::channel();
        let _ = received_tx;
        Self {
            mode,
            replies_tx,
            replies,
            _received: received,
        }
    }
}

impl HostTransport for ScriptedHost {
    fn send_control(&mut self, control: &Control) -> Result<(), TransportError> {
        match control {
            Control::Warmup(_) => match self.mode {
                Mode::Warm => {
                    let _ = self.replies_tx.send(Frame::Control(Control::Warm(Warm {
                        family: "asr".to_owned(),
                        model: "demo".to_owned(),
                        rtf_p50: 0.2,
                        rss_kb: 0,
                    })));
                }
                Mode::Reject => {
                    let _ = self
                        .replies_tx
                        .send(Frame::Control(Control::Error(ErrorMsg {
                            code: "model_unusable".to_owned(),
                            stage: "arming".to_owned(),
                            retryable: false,
                            detail: Some(ErrorDetail {
                                family: Some("asr".to_owned()),
                                reason: Some("运行时尚未接入".to_owned()),
                                ..ErrorDetail::default()
                            }),
                        })));
                }
                Mode::Silent => {}
            },
            Control::SessionStart(_) => {
                let _ = self
                    .replies_tx
                    .send(Frame::Control(Control::Heartbeat(Heartbeat {
                        pid: 1,
                        rss_kb: 0,
                        model_state: "cold".to_owned(),
                        queue_in: 0,
                        queue_out: 0,
                    })));
            }
            Control::SessionEnd(_) => {
                let _ = self.replies_tx.send(Frame::Control(Control::Final(Final {
                    utterance_seq: 1,
                    text: "你好".to_owned(),
                    start_ms: 0,
                    end_ms: 10,
                    finalize_ms: 20,
                })));
            }
            Control::Close => {
                let _ = self.replies_tx.send(Frame::Control(Control::Closed));
            }
            _ => {}
        }
        Ok(())
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let frame = axon_voice_protocol::decode_frame(bytes)
            .map_err(|error| TransportError::Write(error.to_string()))?;
        if let Frame::AudioIn(payload) = frame
            && !payload.is_empty()
        {
            let _ = self
                .replies_tx
                .send(Frame::Control(Control::Partial(Partial {
                    utterance_seq: 1,
                    text: "你好".to_owned(),
                })));
        }
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame, TransportError> {
        self.replies
            .recv_timeout(timeout)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => TransportError::Timeout,
                mpsc::RecvTimeoutError::Disconnected => TransportError::Closed,
            })
    }

    fn close(&mut self) {}
}

fn bridge(mode: Mode) -> VoiceBridge<ScriptedHost> {
    VoiceBridge::new(ScriptedHost::new(mode))
}

#[test]
fn warmup_then_session_reaches_final() {
    let mut bridge = bridge(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    assert_eq!(bridge.state(), BridgeState::Ready);

    bridge
        .start("s-1", "full_duplex", "zh", &[])
        .expect("应能开始");
    assert_eq!(bridge.state(), BridgeState::InSession);

    bridge.push_audio(&[0u8; 640]).expect("应能推音频");
    bridge.push_audio(&[0u8; 640]).expect("应能推音频");
    assert_eq!(next_partial(&mut bridge), "你好");

    bridge.end_session("client_close").expect("应能结束");
    assert_eq!(bridge.state(), BridgeState::Ready);
    assert_eq!(next_final(&mut bridge), "你好");
}

fn next_partial(bridge: &mut VoiceBridge<ScriptedHost>) -> String {
    for _ in 0..4 {
        match bridge.poll(Duration::from_secs(1)).expect("应有事件") {
            Some(VoiceEvent::Partial(partial)) => return partial.text,
            Some(_) => continue,
            None => break,
        }
    }
    panic!("没有 partial");
}

fn next_final(bridge: &mut VoiceBridge<ScriptedHost>) -> String {
    for _ in 0..4 {
        match bridge.poll(Duration::from_secs(1)).expect("应有事件") {
            Some(VoiceEvent::Final(final_message)) => return final_message.text,
            Some(_) => continue,
            None => break,
        }
    }
    panic!("没有 final");
}

#[test]
fn host_error_is_surfaced() {
    let mut bridge = bridge(Mode::Reject);
    match bridge.warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1)) {
        Err(BridgeError::Host(error)) => assert_eq!(error.code, "model_unusable"),
        other => panic!("期望宿主错误，得到 {other:?}"),
    }
    assert_eq!(bridge.state(), BridgeState::Idle);
}

#[test]
fn state_guards_are_enforced() {
    let mut bridge = bridge(Mode::Warm);
    assert_eq!(
        bridge.start("s", "push_to_talk", "zh", &[]),
        Err(BridgeError::NotReady)
    );
    assert_eq!(bridge.push_audio(&[0, 0]), Err(BridgeError::NoSession));
    assert_eq!(bridge.end_session("idle"), Err(BridgeError::NoSession));

    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge.start("s-1", "full_duplex", "zh", &[]).expect("开始");
    assert_eq!(
        bridge.start("s-2", "full_duplex", "zh", &[]),
        Err(BridgeError::Busy)
    );
    bridge.cancel("barge").expect("取消应成功");
}

#[test]
fn timeout_on_silent_host() {
    let mut bridge = bridge(Mode::Silent);
    assert_eq!(
        bridge.warmup("asr", "demo", &"0".repeat(64), Duration::from_millis(50)),
        Err(BridgeError::Transport(TransportError::Timeout))
    );
}

#[test]
fn heartbeat_and_close_are_mapped() {
    let mut bridge = bridge(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge.start("s-1", "full_duplex", "zh", &[]).expect("开始");
    match bridge.poll(Duration::from_secs(1)).expect("应有事件") {
        Some(VoiceEvent::Heartbeat(beat)) => assert_eq!(beat.model_state, "cold"),
        other => panic!("期望心跳，得到 {other:?}"),
    }
    bridge.close();
    assert_eq!(bridge.state(), BridgeState::Closed);
}

#[test]
fn poll_returns_none_on_timeout() {
    let mut bridge = bridge(Mode::Warm);
    assert!(
        bridge
            .poll(Duration::from_millis(20))
            .expect("应超时")
            .is_none()
    );
}
