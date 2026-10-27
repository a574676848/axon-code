//! `/api/v1/voice` 的 WS 控制面契约测试：真实 socket、真实升级握手、真实帧。
//!
//! 这里只断言对外契约（鉴权、`ready` 字段、阶段错误帧、转写事件），不断言内部
//! 线程结构；宿主缺失时的错误码来自运行环境，按合同允许的取值集合断言。

use super::test_support::{RecordingIngress, TEST_AUTH_TOKEN, TestServer, request};
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

        let error: serde_json::Value =
            serde_json::from_str(&next_ws_text(&mut socket).await).expect("error 是 JSON");
        assert_eq!(error["type"], "error");
        assert_eq!(error["stage"], "arming");
        let code = error["code"].as_str().unwrap_or_default();
        assert!(
            matches!(code, "worker_unavailable" | "model_missing"),
            "测试目录无宿主时应报宿主或模型缺失，实际 {code}"
        );
        assert!(
            error["message"].as_str().is_some_and(|text| !text.is_empty()),
            "错误帧必须带合同文案"
        );
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
        loop {
            assert!(std::time::Instant::now() < deadline, "等待 final 超时");
            let text = next_ws_text(&mut socket).await;
            let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            match value["type"].as_str() {
                Some("final") => {
                    let transcript = value["text"].as_str().unwrap_or_default();
                    println!("ws transcript: {transcript}");
                    assert!(!transcript.is_empty(), "转写不应为空");
                    return;
                }
                Some("error") => panic!("合成/识别失败：{text}"),
                _ => {}
            }
        }
    });
}
