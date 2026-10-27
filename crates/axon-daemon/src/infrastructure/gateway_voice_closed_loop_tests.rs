//! 语音回合闭环：`final → turn → speak → 0x02 → stopAudio → playbackDrained`。
//!
//! 用真实 socket、真实 WS 升级与进程内脚本宿主，验证 daemon 侧编排而不依赖模型
//! 与原生运行时；宿主只记录收到的控制帧并按脚本回帧，断言全部落在对外帧上。

use super::VoiceTimeouts;
use super::gateway_voice_worker::{BoxedTransport, HostLauncher};
use super::test_support::{RecordingIngress, TEST_AUTH_TOKEN, TestServer, get, request};
use crate::infrastructure::gateway_kernel::GatewayEvent;
use axon_voice_bridge::axon_voice_protocol::{
    Control, Final, Frame, Heartbeat, Warm, decode_frame,
};
use axon_voice_bridge::{HostTransport, TransportError};
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

/// 测试连接类型。
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 进程内脚本宿主：收到控制帧就按脚本回帧，并把控制帧留给断言。
struct ScriptedHost {
    controls: Arc<Mutex<Vec<Control>>>,
    reply_tx: std::sync::mpsc::Sender<Frame>,
    replies: std::sync::mpsc::Receiver<Frame>,
}

impl ScriptedHost {
    fn reply(&self, frame: Frame) {
        let _ = self.reply_tx.send(frame);
    }
}

impl HostTransport for ScriptedHost {
    fn send_control(&mut self, control: &Control) -> Result<(), TransportError> {
        self.controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(control.clone());
        match control {
            Control::Warmup(_) => self.reply(Frame::Control(Control::Warm(Warm {
                family: "asr".to_owned(),
                model: "demo".to_owned(),
                rtf_p50: 0.2,
                rss_kb: 0,
            }))),
            Control::SessionStart(_) => self.reply(Frame::Control(Control::Heartbeat(Heartbeat {
                pid: 1,
                rss_kb: 0,
                model_state: "warm".to_owned(),
                queue_in: 0,
                queue_out: 0,
            }))),
            Control::Speak(speak) => self.reply(Frame::AudioOut {
                epoch: speak.epoch,
                seq: speak.seq,
                audio: vec![0x11, u8::try_from(speak.seq).unwrap_or(0), 0x22, 0x33],
            }),
            Control::Close => self.reply(Frame::Control(Control::Closed)),
            _ => {}
        }
        Ok(())
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        let frame =
            decode_frame(bytes).map_err(|error| TransportError::Write(error.to_string()))?;
        if matches!(frame, Frame::AudioIn(_)) {
            self.reply(Frame::Control(Control::Final(Final {
                utterance_seq: 1,
                text: "打开设置。".to_owned(),
                start_ms: 0,
                end_ms: 10,
                finalize_ms: 20,
            })));
        }
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame, TransportError> {
        self.replies
            .recv_timeout(timeout)
            .map_err(|error| match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => TransportError::Timeout,
                std::sync::mpsc::RecvTimeoutError::Disconnected => TransportError::Closed,
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

/// 脚本宿主工厂与它的控制帧记录。
fn scripted_host() -> (HostLauncher, Arc<Mutex<Vec<Control>>>) {
    let controls = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&controls);
    let launcher: HostLauncher = Arc::new(move || {
        let (reply_tx, replies) = std::sync::mpsc::channel();
        Ok(Box::new(ScriptedHost {
            controls: Arc::clone(&captured),
            reply_tx,
            replies,
        }) as BoxedTransport)
    });
    (launcher, controls)
}

fn test_timeouts(drained: Duration) -> VoiceTimeouts {
    VoiceTimeouts {
        idle: Duration::from_secs(30),
        session: Duration::from_secs(30),
        warmup: Duration::from_secs(5),
        drained,
    }
}

fn lane_id() -> String {
    let audience = axon_domain::AudienceContext::new(
        axon_domain::AudienceKind::Private,
        axon_domain::DisclosurePolicy::PrivateOnly,
    );
    axon_session::session_lane_id("w-1", &audience, "desktop", "desktop")
}

fn wire_delta(text: &str) -> GatewayEvent {
    GatewayEvent {
        event_type: "wire_event".to_owned(),
        payload_json: serde_json::json!({
            "interactionId": "interaction-1",
            "schemaVersion": 1,
            "eventId": "event-1",
            "correlationId": "correlation-1",
            "session": {"sessionId": lane_id()},
            "sequence": 1,
            "recordedAtMs": 0,
            "kind": {
                "type": "assistantMessage",
                "payload": {
                    "partId": "part-1",
                    "messageId": "message-1",
                    "content": text,
                    "delta": true,
                }
            }
        })
        .to_string(),
        cursor: None,
        replay: false,
    }
}

fn turn_completed(text: &str) -> GatewayEvent {
    GatewayEvent {
        event_type: "turn_completed".to_owned(),
        payload_json: serde_json::json!({
            "interactionId": "interaction-1",
            "response": text,
        })
        .to_string(),
        cursor: None,
        replay: false,
    }
}

async fn next_message(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(20), socket.next())
        .await
        .expect("等待帧超时")
        .expect("连接不应提前结束")
        .expect("帧应可读")
}

async fn next_json_type(socket: &mut Socket, wanted: &str) -> serde_json::Value {
    for _ in 0..64 {
        if let Message::Text(text) = next_message(socket).await {
            let value: serde_json::Value = serde_json::from_str(&text).expect("文本帧是 JSON");
            if value["type"] == wanted {
                return value;
            }
        }
    }
    panic!("未等到 {wanted} 帧");
}

async fn next_audio(socket: &mut Socket) -> Vec<u8> {
    for _ in 0..64 {
        if let Message::Binary(bytes) = next_message(socket).await {
            return bytes.to_vec();
        }
    }
    panic!("未等到音频帧");
}

async fn send_json(socket: &mut Socket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("发送文本帧");
}

/// 建连、消费 `ready`、发 `start` 并等到 `listening`。
async fn open_session(server: &TestServer, tts: bool) -> Socket {
    let url = format!(
        "ws://{}/api/v1/voice?api_token={TEST_AUTH_TOKEN}",
        server.address
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("WS 握手应成功");
    let ready = next_json_type(&mut socket, "ready").await;
    assert_eq!(ready["sampleRate"], 16_000);
    send_json(
        &mut socket,
        serde_json::json!({
            "type": "start",
            "protocolVersion": 1,
            "clientSessionId": "c-1",
            "workspaceId": "w-1",
            "replyDestination": "desktop",
            "mode": "full_duplex",
            "lang": "zh",
            "tts": tts,
        }),
    )
    .await;
    let listening = next_json_type(&mut socket, "state").await;
    assert_eq!(listening["phase"], "listening");
    socket
}

/// 发送一帧上行音频触发脚本宿主的 `final`，并取回 `turn` 帧。
async fn push_final(socket: &mut Socket) -> serde_json::Value {
    socket
        .send(Message::Binary(vec![0x01, 0, 0, 0, 0].into()))
        .await
        .expect("发送音频帧");
    next_json_type(socket, "turn").await
}

async fn wait_subscriber(egress: &Arc<super::test_support::ScriptedEgress>) {
    for _ in 0..200 {
        if egress.subscriber_count() > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("订阅线程未就绪");
}

async fn wait_speak(controls: &Arc<Mutex<Vec<Control>>>) -> Control {
    for _ in 0..200 {
        if let Some(speak) = controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .find(|control| matches!(control, Control::Speak(_)))
            .cloned()
        {
            return speak;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("宿主未收到 speak");
}

async fn wait_cancel(controls: &Arc<Mutex<Vec<Control>>>, reason: &str) -> bool {
    for _ in 0..200 {
        if controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|control| matches!(control, Control::Cancel(cancel) if cancel.reason == reason))
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时")
}

fn status_metrics(server: &TestServer) -> serde_json::Value {
    let (status, body) = request(server, &get("/api/v1/voice/status", Some(TEST_AUTH_TOKEN)));
    assert_eq!(status, 200, "{body}");
    serde_json::from_str::<serde_json::Value>(&body).expect("status 是 JSON")["metrics"].clone()
}

#[test]
fn turn_closed_loop_reaches_playback_drained() {
    let (launcher, controls) = scripted_host();
    let (server, egress) = TestServer::start_scripted(
        RecordingIngress::idle(),
        test_timeouts(Duration::from_secs(2)),
        launcher,
    );
    let runtime = runtime();
    runtime.block_on(async move {
        let mut socket = open_session(&server, true).await;
        let turn = push_final(&mut socket).await;

        // INV-1：转写以既有端口提交，渠道与车道键与文本入口一致。
        assert_eq!(turn["interactionId"], "interaction-1");
        assert_eq!(turn["epoch"], 1);
        assert_eq!(turn["turnId"], "interaction-1:1");
        assert_eq!(turn["laneId"], lane_id());
        assert_eq!(turn["sessionId"], lane_id());
        let submitted = server.ingress.submitted();
        assert_eq!(submitted.len(), 1, "必须且只能提交一次");
        assert_eq!(submitted[0].source_channel, "desktop");
        assert_eq!(submitted[0].reply_channel, "desktop");
        assert_eq!(submitted[0].reply_destination, "desktop");
        assert_eq!(submitted[0].workspace_id, "w-1");
        assert_eq!(submitted[0].content, "打开设置。");

        // 助手增量渲染成朗读稿，经宿主合成后原样透传成 0x02 帧。
        wait_subscriber(&egress).await;
        egress.push(wire_delta("打开设置，第一句。"));
        match wait_speak(&controls).await {
            Control::Speak(speak) => {
                assert_eq!(speak.turn_id, "interaction-1:1");
                assert_eq!(speak.epoch, 1);
                assert_eq!(speak.seq, 1, "seq 每轮从 1 起");
                assert!(speak.text.contains("第一句"));
            }
            other => panic!("期望 speak，得到 {other:?}"),
        }
        let audio = next_audio(&mut socket).await;
        assert_eq!(audio[0], 0x02, "对话音频只走 0x02");
        assert_eq!(
            u32::from_be_bytes(audio[1..5].try_into().expect("epoch 头")),
            1
        );
        assert_eq!(
            u32::from_be_bytes(audio[5..9].try_into().expect("pcmSeq 头")),
            1,
            "pcmSeq 必须原样透传"
        );
        assert_eq!(&audio[9..], &[0x11, 1, 0x22, 0x33]);

        // 一轮结束：先停播，客户端回执后才迁回聆听。
        egress.push(turn_completed("打开设置，第一句。"));
        let stop = next_json_type(&mut socket, "stopAudio").await;
        assert_eq!(stop["epoch"], 1);
        assert_eq!(stop["reason"], "turn_end");

        send_json(
            &mut socket,
            serde_json::json!({"type": "playbackDrained", "epoch": 1}),
        )
        .await;
        let state = next_json_type(&mut socket, "state").await;
        assert_eq!(state["phase"], "listening");
        assert_eq!(status_metrics(&server)["audioFlushed"], 1);
    });
}

#[test]
fn user_cancel_in_speaking_stops_audio_and_waits_for_receipt() {
    let (launcher, controls) = scripted_host();
    let (server, egress) = TestServer::start_scripted(
        RecordingIngress::idle(),
        test_timeouts(Duration::from_secs(2)),
        launcher,
    );
    let runtime = runtime();
    runtime.block_on(async move {
        let mut socket = open_session(&server, true).await;
        let _turn = push_final(&mut socket).await;
        wait_subscriber(&egress).await;
        egress.push(wire_delta("这一句会被打断。"));
        let _speak = wait_speak(&controls).await;
        let _audio = next_audio(&mut socket).await;

        // F-02：说话期取消必须立刻进入停播闭环。
        send_json(
            &mut socket,
            serde_json::json!({"type": "cancel", "reason": "user_cancel"}),
        )
        .await;
        let stop = next_json_type(&mut socket, "stopAudio").await;
        assert_eq!(stop["epoch"], 1);
        assert_eq!(stop["reason"], "user_cancel");
        assert!(
            wait_cancel(&controls, "user_cancel").await,
            "停播必须同时通知宿主取消合成"
        );

        // 回执之前仍停在说话相位；收到回执才迁回聆听。
        send_json(
            &mut socket,
            serde_json::json!({"type": "playbackDrained", "epoch": 1}),
        )
        .await;
        let state = next_json_type(&mut socket, "state").await;
        assert_eq!(state["phase"], "listening");
        assert_eq!(status_metrics(&server)["audioFlushed"], 1);
    });
}

#[test]
fn missing_playback_drained_times_out_and_forces_cleanup() {
    let (launcher, controls) = scripted_host();
    let (server, egress) = TestServer::start_scripted(
        RecordingIngress::idle(),
        test_timeouts(Duration::from_millis(150)),
        launcher,
    );
    let runtime = runtime();
    runtime.block_on(async move {
        let mut socket = open_session(&server, true).await;
        let _turn = push_final(&mut socket).await;
        wait_subscriber(&egress).await;
        egress.push(wire_delta("这一句之后没有回执。"));
        let _speak = wait_speak(&controls).await;
        let _audio = next_audio(&mut socket).await;
        egress.push(turn_completed("这一句之后没有回执。"));
        let stop = next_json_type(&mut socket, "stopAudio").await;
        assert_eq!(stop["reason"], "turn_end");

        // 不回 playbackDrained：到点必须记 internal 并强制清理。
        let error = next_json_type(&mut socket, "error").await;
        assert_eq!(error["code"], "internal");
        assert_eq!(error["stage"], "playback");
        let state = next_json_type(&mut socket, "state").await;
        assert_eq!(state["phase"], "listening");

        let metrics = status_metrics(&server);
        assert_eq!(metrics["lastError"], "internal");
        assert_eq!(metrics["playbackDrainsTimedOut"], 1);
        assert_eq!(metrics["audioFlushed"], 0);
    });
}

#[test]
fn tts_disabled_submits_without_speaking_but_keeps_lifecycle() {
    let (launcher, controls) = scripted_host();
    let (server, egress) = TestServer::start_scripted(
        RecordingIngress::idle(),
        test_timeouts(Duration::from_secs(2)),
        launcher,
    );
    let runtime = runtime();
    runtime.block_on(async move {
        let mut socket = open_session(&server, false).await;
        let turn = push_final(&mut socket).await;
        assert_eq!(turn["epoch"], 1);
        // 不朗读：订阅照常建立以拿回合终态，但不得向宿主下发 speak。
        wait_subscriber(&egress).await;
        egress.push(wire_delta("这段不该被读出来。"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .any(|control| matches!(control, Control::Speak(_))),
            "关闭朗读后不得下发 speak"
        );
        // 终态与回执闭环照旧：空队列也必须回执。
        egress.push(turn_completed("这段不该被读出来。"));
        let stop = next_json_type(&mut socket, "stopAudio").await;
        assert_eq!(stop["epoch"], 1);
        assert_eq!(stop["reason"], "turn_end");
        send_json(
            &mut socket,
            serde_json::json!({"type": "playbackDrained", "epoch": 1}),
        )
        .await;
        let state = next_json_type(&mut socket, "state").await;
        assert_eq!(state["phase"], "listening");
    });
}
