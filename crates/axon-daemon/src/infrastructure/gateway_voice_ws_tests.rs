//! `/api/v1/voice` 的 WS 控制面契约测试：真实 socket、真实升级握手、真实帧。
//!
//! 这里只断言对外契约（鉴权、`ready` 字段、阶段错误帧），不断言内部线程结构；
//! 提供方尚未接入，`start` 的错误码因此是确定的。

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

/// `/api/v1/voice/status` 是排障唯一入口：构建版本与预设清单必须直接可读。
#[test]
fn voice_status_reports_build_version_and_presets() {
    let server = TestServer::start(RecordingIngress::idle());
    let (status, body) = request(&server, &get("/api/v1/voice/status", Some(TEST_AUTH_TOKEN)));
    assert_eq!(status, 200, "{body}");

    let value: serde_json::Value = serde_json::from_str(&body).expect("status 是 JSON");
    assert!(
        value["buildVersion"]
            .as_str()
            .is_some_and(|version| !version.is_empty()),
        "status 必须给出构建版本：{body}"
    );
    let presets = value["presetIds"].as_array().expect("presetIds 是数组");
    assert_eq!(presets.len(), 4, "预设清单长度应与 PRESETS 一致");
    assert!(
        presets.iter().any(|id| id == "calm-elegant"),
        "默认音色必须在清单内"
    );
    assert_eq!(value["defaultPreset"], "calm-elegant");
    assert_eq!(
        value["runtimeAvailable"], false,
        "提供方尚未接入时必须如实报不可用"
    );
    assert_eq!(value["degraded"], "voice_provider_unavailable");
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
fn voice_ws_requires_token_and_reports_provider_unavailable() {
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

        // 没有配置任何云端提供方：`start` 必须回 arming 阶段的 provider_unavailable。
        let first = next_ws_text(&mut socket).await;
        let value: serde_json::Value = serde_json::from_str(&first).expect("首帧是 JSON");
        assert_eq!(value["type"], "error", "{first}");
        assert_eq!(value["code"], "provider_unavailable", "{first}");
        assert_eq!(value["stage"], "arming", "{first}");
        assert!(
            value["message"].as_str().is_some_and(|text| !text.is_empty()),
            "错误帧必须带合同文案"
        );
    });
}
