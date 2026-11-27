//! realtime 车道的会话闭环：真实本地 WS 服务扮演百炼，断言全部落在对外帧上。

use super::super::gateway_voice_metrics::VoiceMetrics;
use super::super::gateway_voice_realtime_protocol::{BailianProvider, SessionConfig};
use super::super::gateway_voice_realtime_provider::RealtimeProvider;
use super::super::gateway_voice_worker::VoiceOut;
use super::spawn_realtime_with;
use futures_util::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::Message;

const FRAME_TIMEOUT: Duration = Duration::from_secs(3);
const COLLECT_TIMEOUT: Duration = Duration::from_secs(5);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时")
}

fn provider(address: SocketAddr) -> BailianProvider {
    BailianProvider::new(
        format!("ws://{address}/api-ws/v1/realtime?model=test-model"),
        "test-key".to_owned(),
        SessionConfig {
            voice: "test-voice".to_owned(),
            instructions: "回答简短".to_owned(),
            ..SessionConfig::default()
        },
    )
}

async fn next_json<S>(socket: &mut S) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = tokio::time::timeout(FRAME_TIMEOUT, socket.next())
            .await
            .expect("等帧超时")
            .expect("连接未断开")
            .expect("帧合法");
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).expect("合法 JSON");
        }
    }
}

#[test]
fn handshake_carries_bearer_and_disables_data_inspection() {
    let provider = BailianProvider::new(
        "wss://example.com/api-ws/v1/realtime?model=m".to_owned(),
        "sk-test".to_owned(),
        SessionConfig::default(),
    );
    let request = provider.handshake_request().expect("请求应可构造");
    assert_eq!(request.headers()["authorization"], "Bearer sk-test");
    assert_eq!(request.headers()["x-dashscope-datainspection"], "disable");
    assert!(
        request.uri().to_string().contains("model=m"),
        "模型必须随地址下发"
    );
}

#[test]
fn realtime_session_relays_transcript_and_audio() {
    let runtime = runtime();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定端口");
        let address = listener.local_addr().expect("本地地址");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("接受连接");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("完成握手");
            let update = next_json(&mut socket).await;
            assert_eq!(update["type"], "session.update");
            assert_eq!(update["session"]["voice"], "test-voice");
            assert_eq!(update["session"]["turn_detection"]["type"], "server_vad");
            socket
                .send(Message::Text(
                    r#"{"type":"session.updated","session":{"id":"s"}}"#.into(),
                ))
                .await
                .expect("回执配置");

            // 上行：一帧 1280 字节应被切成两个 640 字节分片。
            let append = next_json(&mut socket).await;
            assert_eq!(append["type"], "input_audio_buffer.append");
            let append = next_json(&mut socket).await;
            assert_eq!(append["type"], "input_audio_buffer.append");

            for event in [
                r#"{"type":"input_audio_buffer.speech_started"}"#,
                r#"{"type":"input_audio_buffer.speech_stopped"}"#,
                r#"{"type":"conversation.item.input_audio_transcription.delta","text":"今天","stash":"天气"}"#,
                r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"今天天气"}"#,
                r#"{"type":"response.created","response":{"id":"r-1"}}"#,
                r#"{"type":"response.audio_transcript.delta","delta":"晴"}"#,
                r#"{"type":"response.audio.delta","delta":"AAAAAAoAAAAUAAAAHgAAACgAAAAyAAAA"}"#,
                r#"{"type":"response.done","response":{"status":"completed"}}"#,
            ] {
                socket
                    .send(Message::Text(event.into()))
                    .await
                    .expect("推送事件");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // 保持连接，等客户端收齐后自行断开。
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let (out_tx, mut out_rx) = tokio_mpsc::unbounded_channel();
        let metrics = Arc::new(VoiceMetrics::default());
        let worker = spawn_realtime_with(
            provider(address),
            out_tx,
            Duration::from_secs(2),
            Arc::clone(&metrics),
        );

        let mut texts: Vec<serde_json::Value> = Vec::new();
        let mut audios: Vec<(u32, u32, usize)> = Vec::new();
        let mut pushed = false;
        let deadline = Instant::now() + COLLECT_TIMEOUT;
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), out_rx.recv()).await {
                Ok(Some(VoiceOut::Text(text))) => {
                    let value: serde_json::Value = serde_json::from_str(&text).expect("合法 JSON");
                    let listening = value["type"] == "state" && value["phase"] == "listening";
                    texts.push(value);
                    if listening && !pushed {
                        pushed = true;
                        worker.push_audio(vec![0u8; 1_280]);
                    }
                }
                Ok(Some(VoiceOut::Audio { epoch, seq, pcm })) => {
                    audios.push((epoch, seq, pcm.len()));
                }
                Ok(Some(VoiceOut::Final { .. })) => panic!("realtime 不应产出 final 帧"),
                Ok(None) => break,
                Err(_) => {}
            }
            if !audios.is_empty() && texts.iter().any(|value| value["type"] == "assistant") {
                break;
            }
        }
        worker.shutdown();
        server.abort();

        let partials: Vec<&str> = texts
            .iter()
            .filter(|value| value["type"] == "partial")
            .filter_map(|value| value["text"].as_str())
            .collect();
        assert!(
            partials.contains(&"今天天气"),
            "增量与定稿都要落到 partial，实际 {partials:?}"
        );
        let phases: Vec<&str> = texts
            .iter()
            .filter(|value| value["type"] == "state")
            .filter_map(|value| value["phase"].as_str())
            .collect();
        assert!(phases.contains(&"thinking"), "端点判定后进入思考：{phases:?}");
        assert!(phases.contains(&"speaking"), "回复开始后进入说话：{phases:?}");
        assert!(
            texts.iter().any(|value| {
                value["type"] == "assistant" && value["text"] == "晴" && value["epoch"] == 1
            }),
            "助手文字要带轮次下发"
        );
        assert_eq!(audios.len(), 1, "12 个 24 kHz 采样重采样后是一片 16 kHz 音频");
        let (epoch, seq, len) = audios[0];
        assert_eq!((epoch, seq), (1, 1), "首轮首个分片必须从 1 起编号");
        assert_eq!(len, 16, "12 个采样按 3:2 折算成 8 个采样");
    });
}

#[test]
fn provider_error_is_reported_and_session_ends() {
    let runtime = runtime();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定端口");
        let address = listener.local_addr().expect("本地地址");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("接受连接");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("完成握手");
            let _ = next_json(&mut socket).await;
            socket
                .send(Message::Text(
                    r#"{"type":"error","error":{"code":"invalid_parameter","message":"voice 不支持"}}"#
                        .into(),
                ))
                .await
                .expect("推送错误");
            tokio::time::sleep(Duration::from_millis(500)).await;
        });

        let (out_tx, mut out_rx) = tokio_mpsc::unbounded_channel();
        let metrics = Arc::new(VoiceMetrics::default());
        let _worker = spawn_realtime_with(
            provider(address),
            out_tx,
            Duration::from_secs(2),
            Arc::clone(&metrics),
        );
        let out = tokio::time::timeout(FRAME_TIMEOUT, out_rx.recv())
            .await
            .expect("等错误帧超时")
            .expect("通道未关闭");
        let VoiceOut::Text(text) = out else {
            panic!("期望文本帧，得到 {out:?}");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("合法 JSON");
        assert_eq!(value["type"], "error");
        assert_eq!(value["code"], "invalid_parameter");
        assert_eq!(value["message"], "voice 不支持");
        server.abort();
    });
}
