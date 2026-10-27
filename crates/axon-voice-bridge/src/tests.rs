//! 桥的状态机测试：用进程内脚本宿主验证编排，不启动真实进程。

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    sent: Arc<Mutex<Vec<Control>>>,
}

impl ScriptedHost {
    fn new(mode: Mode) -> Self {
        let (replies_tx, replies) = mpsc::channel();
        Self {
            mode,
            replies_tx,
            replies,
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn sent_handle(&self) -> Arc<Mutex<Vec<Control>>> {
        Arc::clone(&self.sent)
    }

    fn reply(&self, frame: Frame) {
        let _ = self.replies_tx.send(frame);
    }
}

impl HostTransport for ScriptedHost {
    fn send_control(&mut self, control: &Control) -> Result<(), TransportError> {
        self.sent.lock().expect("锁").push(control.clone());
        match control {
            Control::Warmup(_) => match self.mode {
                Mode::Warm => self.reply(Frame::Control(Control::Warm(Warm {
                    family: "asr".to_owned(),
                    model: "demo".to_owned(),
                    rtf_p50: 0.2,
                    rss_kb: 0,
                }))),
                Mode::Reject => self.reply(Frame::Control(Control::Error(ErrorMsg {
                    code: "model_unusable".to_owned(),
                    stage: "arming".to_owned(),
                    retryable: false,
                    detail: Some(ErrorDetail {
                        family: Some("asr".to_owned()),
                        reason: Some("运行时尚未接入".to_owned()),
                        ..ErrorDetail::default()
                    }),
                }))),
                Mode::Silent => {}
            },
            Control::SessionStart(_) => self.reply(Frame::Control(Control::Heartbeat(Heartbeat {
                pid: 1,
                rss_kb: 0,
                model_state: "cold".to_owned(),
                queue_in: 0,
                queue_out: 0,
            }))),
            Control::SessionEnd(_) => self.reply(Frame::Control(Control::Final(Final {
                utterance_seq: 1,
                text: "你好".to_owned(),
                start_ms: 0,
                end_ms: 10,
                finalize_ms: 20,
            }))),
            Control::Cancel(_) => self.reply(Frame::Control(Control::Error(ErrorMsg {
                code: "tts_failed".to_owned(),
                stage: "speaking".to_owned(),
                retryable: true,
                detail: None,
            }))),
            Control::Close => self.reply(Frame::Control(Control::Closed)),
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
            self.reply(Frame::Control(Control::Partial(Partial {
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

    fn heartbeat_ok(&self, _now: Instant) -> bool {
        true
    }

    fn alive(&mut self) -> bool {
        true
    }

    fn terminate(&mut self) -> Option<i32> {
        None
    }

    fn shutdown(&mut self) {
        self.reply(Frame::Control(Control::Closed));
    }
}

fn bridge(mode: Mode) -> VoiceBridge<ScriptedHost> {
    VoiceBridge::new(ScriptedHost::new(mode))
}

fn bridge_with_sent(mode: Mode) -> (VoiceBridge<ScriptedHost>, Arc<Mutex<Vec<Control>>>) {
    let host = ScriptedHost::new(mode);
    let sent = host.sent_handle();
    (VoiceBridge::new(host), sent)
}

fn drain_events(bridge: &mut VoiceBridge<ScriptedHost>, want: usize) -> Vec<VoiceEvent> {
    let mut events = Vec::new();
    for _ in 0..8 {
        if events.len() >= want {
            break;
        }
        match bridge.poll(Duration::from_secs(1)).expect("应有事件") {
            Some(event) => events.push(event),
            None => break,
        }
    }
    events
}

#[test]
fn warmup_then_session_reaches_final() {
    let mut bridge = bridge(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    assert_eq!(bridge.state(), BridgeState::Ready);

    bridge
        .start("s-1", "full_duplex", "zh", &["车道".to_owned()])
        .expect("应能开始");
    assert_eq!(bridge.state(), BridgeState::InSession);

    bridge.push_audio(&[0u8; 640]).expect("应能推音频");
    let events = drain_events(&mut bridge, 3);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, VoiceEvent::Partial(partial) if partial.text == "你好")),
        "应有 partial：{events:?}"
    );

    bridge.end_session("client_close").expect("应能结束");
    assert_eq!(bridge.state(), BridgeState::Ready);
    let events = drain_events(&mut bridge, 3);
    assert!(
        events.iter().any(
            |event| matches!(event, VoiceEvent::Final(final_message) if final_message.text == "你好")
        ),
        "应有 final：{events:?}"
    );
}

#[test]
fn outbound_controls_carry_fields() {
    let (mut bridge, sent) = bridge_with_sent(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"a".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge
        .start("s-1", "full_duplex", "zh", &["车道".to_owned()])
        .expect("开始");
    bridge.cancel("barge").expect("取消");

    let sent = sent.lock().expect("锁");
    assert!(matches!(
        sent.first(),
        Some(Control::Warmup(warmup)) if warmup.manifest_sha256 == "a".repeat(64)
    ));
    assert!(sent.iter().any(|control| matches!(
        control,
        Control::SessionStart(start)
            if start.session_id == "s-1"
                && start.mode == "full_duplex"
                && start.sample_rate == 16_000
                && start.hot_words == vec!["车道".to_owned()]
    )));
    assert!(sent.iter().any(|control| matches!(
        control,
        Control::Cancel(cancel) if cancel.reason == "barge"
    )));
}

#[test]
fn speak_carries_turn_epoch_and_sequence() {
    let (mut bridge, sent) = bridge_with_sent(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"a".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    assert!(
        matches!(
            bridge.speak("turn-1", 1, 1, "你好"),
            Err(BridgeError::NoSession)
        ),
        "未开始会话不得下发朗读稿"
    );
    bridge.start("s-1", "full_duplex", "zh", &[]).expect("开始");
    bridge
        .speak("interaction-1:1", 1, 1, "第一句。")
        .expect("应能下发朗读稿");
    assert!(
        matches!(
            bridge.speak("turn", 1, 2, "  "),
            Err(BridgeError::Protocol(_))
        ),
        "空朗读稿必须拒绝"
    );

    let sent = sent.lock().expect("锁");
    assert!(sent.iter().any(|control| matches!(
        control,
        Control::Speak(message)
            if message.turn_id == "interaction-1:1"
                && message.epoch == 1
                && message.seq == 1
                && message.text == "第一句。"
    )));
}

#[test]
fn warmup_is_idempotent_once_ready() {
    let (mut bridge, sent) = bridge_with_sent(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("重复预热应直接成功");
    let warmups = sent
        .lock()
        .expect("锁")
        .iter()
        .filter(|control| matches!(control, Control::Warmup(_)))
        .count();
    assert_eq!(warmups, 1, "重复预热不应再次下发");
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
    assert_eq!(
        bridge.warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1)),
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
fn shutdown_closes_and_rejects_later_calls() {
    let mut bridge = bridge(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge.shutdown();
    assert_eq!(bridge.state(), BridgeState::Closed);
    assert_eq!(
        bridge.poll(Duration::from_millis(10)),
        Err(BridgeError::Closed)
    );
    assert_eq!(
        bridge.start("s", "full_duplex", "zh", &[]),
        Err(BridgeError::Closed)
    );
    assert_eq!(bridge.push_audio(&[0, 0]), Err(BridgeError::Closed));
}

#[test]
fn host_error_frame_becomes_event() {
    let mut bridge = bridge(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge.start("s-1", "full_duplex", "zh", &[]).expect("开始");
    let _ = drain_events(&mut bridge, 1);
    bridge.cancel("barge").expect("取消应成功");
    let events = drain_events(&mut bridge, 3);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, VoiceEvent::Error(error) if error.code == "tts_failed")),
        "应出现错误事件：{events:?}"
    );
}

#[test]
fn empty_audio_is_rejected() {
    let mut bridge = bridge(Mode::Warm);
    bridge
        .warmup("asr", "demo", &"0".repeat(64), Duration::from_secs(1))
        .expect("预热应成功");
    bridge.start("s-1", "full_duplex", "zh", &[]).expect("开始");
    assert!(matches!(
        bridge.push_audio(&[]),
        Err(BridgeError::Protocol(_))
    ));
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

#[test]
fn into_inner_returns_transport() {
    let bridge = bridge(Mode::Warm);
    let host = bridge.into_inner();
    assert!(matches!(host.mode, Mode::Warm));
}
