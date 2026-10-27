//! `/api/v1/voice` 的 WS 控制面契约测试：真实 socket、真实升级握手、真实帧。
//!
//! 这里只断言对外契约（鉴权、`ready` 字段、阶段错误帧、转写事件），不断言内部
//! 线程结构；宿主缺失时的错误码来自运行环境，按合同允许的取值集合断言。

use super::VoiceTimeouts;
use super::test_support::{RecordingIngress, TEST_AUTH_TOKEN, TestServer, get, request};
use std::time::Duration;

/// 取下一帧文本；跳过二进制与控制帧，超过轮数即判失败。
async fn next_ws_text<S>(socket: &mut S) -> String
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures_util::StreamExt;
    for _ in 0..8 {
        let next = tokio::time::timeout(Duration::from_secs(90), socket.next())
            .await
            .unwrap_or_else(|_| panic!("等待文本帧超时"));
        match next {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                return text.to_string();
            }
            Some(Ok(_)) => continue,
            other => panic!("未收到文本帧：{other:?}"),
        }
    }
    panic!("文本帧数量超预期");
}

/// 空闲超时必须是真实断连：注入 300 ms，等服务端发 `session_idle` 并关闭连接。
#[test]
fn voice_ws_idle_timeout_really_disconnects() {
    let timeouts = VoiceTimeouts {
        idle: Duration::from_millis(300),
        session: Duration::from_secs(30),
        warmup: Duration::from_secs(5),
        drained: Duration::from_secs(10),
    };
    let server = TestServer::start_with_timeouts(RecordingIngress::idle(), timeouts);
    let address = server.address;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时");
    runtime.block_on(async move {
        let url = format!("ws://{address}/api/v1/voice?api_token={TEST_AUTH_TOKEN}");
        let (mut socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("WS 握手应成功");
        let ready: serde_json::Value =
            serde_json::from_str(&next_ws_text(&mut socket).await).expect("ready 是 JSON");
        assert_eq!(ready["type"], "ready");

        use futures_util::StreamExt;
        // 一个字都不发：到点必须收到 session_idle(connect) 并被关闭。
        let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("空闲超时必须触发")
            .expect("应收到一帧")
            .expect("帧应可读");
        let text = match frame {
            tokio_tungstenite::tungstenite::Message::Text(text) => text.to_string(),
            other => panic!("期望文本错误帧，得到 {other:?}"),
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("错误帧是 JSON");
        assert_eq!(value["type"], "error");
        assert_eq!(value["code"], "session_idle");
        assert_eq!(value["stage"], "connect");

        // 紧跟一帧 Close（或流结束），证明是服务端主动断连。
        let closing = tokio::time::timeout(Duration::from_secs(5), socket.next()).await;
        match closing {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))) | Ok(None) => {}
            other => panic!("期望服务端关闭连接，得到 {other:?}"),
        }
    });
}

/// 会话总时长到点必须真实断连：注入 3 s，起会话后什么都不发也要被收线。
#[test]
#[ignore = "需要本地运行时与模型"]
fn voice_ws_session_timeout_really_disconnects() {
    let timeouts = VoiceTimeouts {
        idle: Duration::from_secs(30),
        session: Duration::from_secs(3),
        warmup: Duration::from_secs(10),
        drained: Duration::from_secs(10),
    };
    let server = TestServer::start_with_timeouts(RecordingIngress::idle(), timeouts);
    let address = server.address;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时");
    runtime.block_on(async move {
        use futures_util::SinkExt;
        let url = format!("ws://{address}/api/v1/voice?api_token={TEST_AUTH_TOKEN}");
        let (mut socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("WS 握手应成功");
        let _ready = next_ws_text(&mut socket).await;

        let start = r#"{"type":"start","protocolVersion":1,"clientSessionId":"c-session","workspaceId":"w-1","replyDestination":"desktop","mode":"full_duplex","lang":"zh","tts":false}"#;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(start.into()))
            .await
            .expect("发送 start");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            assert!(std::time::Instant::now() < deadline, "会话超时必须触发");
            let text = next_ws_text(&mut socket).await;
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            match value["type"].as_str() {
                Some("error") => {
                    assert_eq!(value["code"], "session_idle", "{text}");
                    assert_eq!(value["stage"], "listening", "{text}");
                    return;
                }
                Some("state") => continue,
                _ => continue,
            }
        }
    });
}

/// `/api/v1/voice/status` 是排障唯一入口：构建号与预设清单必须直接可读。
#[test]
fn voice_status_reports_build_commit_and_presets() {
    let server = TestServer::start(RecordingIngress::idle());
    let (status, body) = request(&server, &get("/api/v1/voice/status", Some(TEST_AUTH_TOKEN)));
    assert_eq!(status, 200, "{body}");

    let value: serde_json::Value = serde_json::from_str(&body).expect("status 是 JSON");
    assert!(
        value["buildCommit"]
            .as_str()
            .is_some_and(|commit| !commit.is_empty()),
        "status 必须给出构建号：{body}"
    );
    let presets = value["presetIds"].as_array().expect("presetIds 是数组");
    assert_eq!(presets.len(), 4, "预设清单长度应与 PRESETS 一致");
    assert!(
        presets.iter().any(|id| id == "calm-elegant"),
        "默认音色必须在清单内"
    );
    assert_eq!(value["defaultPreset"], "calm-elegant");
    assert!(
        value["runtimeAvailable"].is_boolean(),
        "运行时可可用性必须显式可见，不静默"
    );
    assert_eq!(value["warmupTimeoutSec"], 10, "预热上限必须回到合同值");
    assert_eq!(value["idleTimeoutSec"], 30);
    assert_eq!(value["sessionTimeoutSec"], 120);
    for field in [
        "sessionsStarted",
        "sessionsActive",
        "queueDepth",
        "droppedFrames",
        "rtfP50",
        "lastError",
    ] {
        assert!(
            value["metrics"].get(field).is_some(),
            "指标缺字段 {field}：{body}"
        );
    }
}

#[test]
fn voice_ws_requires_token_and_reports_arming_error() {
    let server = TestServer::start(RecordingIngress::idle());
    // 非升级请求会被升级提取器直接拒绝，不进入鉴权分支。
    let (status, _) = request(
        &server,
        "GET /api/v1/voice HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n",
    );
    assert!(
        matches!(status, 400 | 401),
        "非升级请求应被拒绝，实际 {status}"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时");
    let address = server.address;
    runtime.block_on(async move {
        let bad = format!("ws://{address}/api/v1/voice?api_token=wrong-token");
        assert!(
            tokio_tungstenite::connect_async(bad).await.is_err(),
            "错误 token 必须拒绝升级"
        );
        let url = format!("ws://{address}/api/v1/voice?api_token={TEST_AUTH_TOKEN}");
        let (mut socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("WS 握手应成功");

        let ready: serde_json::Value =
            serde_json::from_str(&next_ws_text(&mut socket).await).expect("ready 是 JSON");
        assert_eq!(ready["type"], "ready");
        assert_eq!(ready["sampleRate"], 16_000);

        use futures_util::SinkExt;
        let start = r#"{"type":"start","protocolVersion":1,"clientSessionId":"c-1","workspaceId":"w-1","replyDestination":"desktop","mode":"full_duplex","lang":"zh","tts":false}"#;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(start.into()))
            .await
            .expect("发送 start");

        // 机器上装了语音运行时（例如本机已安装 Axon）时，`start` 会真的拉起宿主并回
        // `state(listening)`；只有缺运行时/宿主时才回 arming 错误。两种都合法，
        // 但都必须符合合同：要么是 arming 错误帧，要么是 listening 状态帧。
        let first = next_ws_text(&mut socket).await;
        let value: serde_json::Value = serde_json::from_str(&first).expect("首帧是 JSON");
        match value["type"].as_str() {
            Some("error") => {
                assert_eq!(value["stage"], "arming");
                let code = value["code"].as_str().unwrap_or_default();
                assert!(
                    matches!(code, "worker_unavailable" | "model_missing"),
                    "缺少宿主或模型时应报对应错误，实际 {code}"
                );
                assert!(
                    value["message"].as_str().is_some_and(|text| !text.is_empty()),
                    "错误帧必须带合同文案"
                );
            }
            Some("state") => {
                assert_eq!(value["phase"], "listening", "本机有运行时时应直接进入聆听");
            }
            other => panic!("start 之后首帧非法：{other:?} / {first}"),
        }
    });
}

#[test]
#[ignore = "需要本地运行时与模型，用 AXON_RUNTIME_ROOT 与 AXON_VOICE_SAMPLE_WAV 指定"]
fn voice_ws_transcribes_real_audio() {
    use futures_util::SinkExt;

    let sample_path = std::env::var("AXON_VOICE_SAMPLE_WAV").expect("缺少 AXON_VOICE_SAMPLE_WAV");
    let wav = std::fs::read(&sample_path).expect("读取样例 WAV");
    assert!(wav.len() > 44, "样例 WAV 过短");
    let pcm = wav[44..].to_vec();

    let server = TestServer::start(RecordingIngress::idle());
    let address = server.address;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("测试运行时");
    runtime.block_on(async move {
        let url = format!("ws://{address}/api/v1/voice?api_token={TEST_AUTH_TOKEN}");
        let (mut socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("WS 握手应成功");
        let ready: serde_json::Value =
            serde_json::from_str(&next_ws_text(&mut socket).await).expect("ready 是 JSON");
        assert_eq!(ready["type"], "ready");

        let start = r#"{"type":"start","protocolVersion":1,"clientSessionId":"c-real","workspaceId":"w-1","replyDestination":"desktop","mode":"full_duplex","lang":"zh","tts":false}"#;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(start.into()))
            .await
            .expect("发送 start");

        // 等宿主预热并进入 listening。
        let mut listening = false;
        for _ in 0..120 {
            let text = next_ws_text(&mut socket).await;
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            if value["type"] == "state" && value["phase"] == "listening" {
                listening = true;
                break;
            }
            if value["type"] == "error" {
                panic!("预热失败：{text}");
            }
        }
        assert!(listening, "未进入 listening");

        for chunk in pcm.chunks(6_400) {
            let mut frame = Vec::with_capacity(chunk.len() + 1);
            frame.push(0x01u8);
            frame.extend_from_slice(chunk);
            socket
                .send(tokio_tungstenite::tungstenite::Message::Binary(frame.into()))
                .await
                .expect("发送音频");
        }
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"type":"stop","utteranceSeq":1}"#.into(),
            ))
            .await
            .expect("发送 stop");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let mut partials: Vec<String> = Vec::new();
        let mut finals: Vec<String> = Vec::new();
        while finals.is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "等待 final 超时；已收到 partial {partials:?}"
            );
            let text = next_ws_text(&mut socket).await;
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            match value["type"].as_str() {
                Some("partial") => {
                    partials.push(value["text"].as_str().unwrap_or_default().to_owned());
                }
                Some("final") => {
                    finals.push(value["text"].as_str().unwrap_or_default().to_owned());
                }
                Some("error") => {
                    // 本用例按最快速度灌音频，队列溢出属设计内行为：只允许 backpressure，
                    // 其它错误码仍按失败处理（INV-7：失败必须可见，但不能被测试误判成致命）。
                    let code = value["code"].as_str().unwrap_or_default();
                    assert_eq!(
                        code, "backpressure",
                        "识别主链路只允许背压错误：{text}"
                    );
                }
                _ => {}
            }
        }
        println!("ws partials: {partials:?}");
        println!("ws finals: {finals:?}");
        assert!(
            partials.iter().any(|text| !text.is_empty()),
            "WP-09 要求真实 partial：{partials:?}"
        );
        assert!(
            finals.iter().any(|text| !text.is_empty()),
            "定稿不应为空：{finals:?}"
        );
    });
}
