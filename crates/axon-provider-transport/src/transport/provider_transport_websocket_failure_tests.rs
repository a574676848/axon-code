use super::*;
use axon_agent_runtime::RunCancelToken;
use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const FAILURE_TEST_TIMEOUT_MS: u64 = 2_000;
const FAILURE_TEST_SESSION: &str = "websocket-failure-session";

#[tokio::test]
async fn websocket_close_before_first_frame_returns_capability() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await.unwrap().unwrap();
        socket
            .close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Again,
                reason: "no available account".into(),
            }))
            .await
            .unwrap();
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = format!("ws://{address}/v1/responses");
    let cancel_token = RunCancelToken::default();
    let outcome = send_websocket_async(
        ProviderWebSocketTransportInput {
            request_plan: ProviderStreamingHttpRequestPlan {
                method: "POST",
                url,
                headers: Vec::new(),
                auth_header: None,
                body_text: request_body(),
                server_conversation_state_allowed: true,
                remote_request_body_allowed: false,
                reasons: Vec::new(),
            },
            timeout_ms: FAILURE_TEST_TIMEOUT_MS,
            websocket_url: Some(websocket_url),
            logical_session: Some(FAILURE_TEST_SESSION.to_owned()),
        },
        &cancel_token,
    )
    .await
    .unwrap();
    server.await.unwrap();

    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        panic!("首帧前关闭的 WebSocket 必须返回 capability 错误")
    };
    assert_eq!(
        error.failure_kind,
        ProviderStreamingTransportFailureKind::Capability
    );
    assert!(error.message.contains("code=1013"));
    assert!(error.message.contains("no available account"));
}

#[tokio::test]
async fn prewarm_close_before_first_frame_activates_endpoint_http_fallback() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await.unwrap().unwrap();
        socket.close(None).await.unwrap();
    });
    let url = format!("http://{address}/v1/responses");
    let prewarm = ProviderWebSocketTransportInput {
        request_plan: ProviderStreamingHttpRequestPlan {
            method: "POST",
            url: url.clone(),
            headers: Vec::new(),
            auth_header: None,
            body_text: request_body(),
            server_conversation_state_allowed: true,
            remote_request_body_allowed: false,
            reasons: Vec::new(),
        },
        timeout_ms: FAILURE_TEST_TIMEOUT_MS,
        websocket_url: Some(format!("ws://{address}/v1/responses")),
        logical_session: None,
    };

    let error = prewarm_websocket(prewarm)
        .await
        .expect_err("预热首帧前关闭必须失败");
    server.await.unwrap();
    assert!(format!("{error:?}").contains("首个响应帧前关闭"));

    let session = crate::AgentRunProviderTransportSession::new(
        "prewarm-capability",
        "prewarm-close-session",
        &url,
        axon_agent_runtime::ProviderStreamingWireTransport::WebSocket,
    );
    assert_eq!(
        session.effective_transport(),
        axon_agent_runtime::ProviderStreamingWireTransport::Http
    );
}

#[tokio::test]
async fn websocket_close_before_completed_returns_incomplete() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(
                serde_json::json!({
                    "type": "response.output_text.delta",
                    "delta": "partial"
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = format!("ws://{address}/v1/responses");
    let cancel_token = RunCancelToken::default();
    let outcome = send_websocket_async(
        ProviderWebSocketTransportInput {
            request_plan: ProviderStreamingHttpRequestPlan {
                method: "POST",
                url,
                headers: Vec::new(),
                auth_header: None,
                body_text: request_body(),
                server_conversation_state_allowed: true,
                remote_request_body_allowed: false,
                reasons: Vec::new(),
            },
            timeout_ms: FAILURE_TEST_TIMEOUT_MS,
            websocket_url: Some(websocket_url),
            logical_session: Some(FAILURE_TEST_SESSION.to_owned()),
        },
        &cancel_token,
    )
    .await
    .unwrap();
    server.await.unwrap();

    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        panic!("缺少 response.completed 的 WebSocket 流必须失败")
    };
    assert_eq!(
        error.failure_kind,
        ProviderStreamingTransportFailureKind::Incomplete
    );
}

#[test]
fn failed_or_incomplete_response_never_advances_chain() {
    let request = serde_json::json!({"input": [], "model": "test"});
    for event in [
        serde_json::json!({
            "type": "response.failed",
            "response": {"error": {"code": "server_error", "message": "failed"}}
        }),
        serde_json::json!({
            "type": "response.incomplete",
            "response": {"incomplete_details": {"reason": "max_output_tokens"}}
        }),
    ] {
        let mut state = completed_chain_state(&request);
        let mut response = WebSocketResponseState::default();
        observe_response_event(&mut response, &event.to_string());

        assert!(response.failure.is_some());
        update_response_chain(&mut state, Some(request.clone()), &response, true);
        assert!(state.chain.is_none());
    }
}

#[test]
fn disabled_server_state_does_not_create_response_chain() {
    let request = serde_json::json!({"input": [], "model": "test"});
    let mut state = WebSocketSessionState::default();
    let mut response = WebSocketResponseState::default();
    observe_response_event(
        &mut response,
        r#"{"type":"response.completed","response":{"id":"resp-disabled","output":[]}}"#,
    );

    update_response_chain(&mut state, Some(request), &response, false);

    assert!(state.chain.is_none());
}

fn completed_chain_state(request: &serde_json::Value) -> WebSocketSessionState {
    let mut state = WebSocketSessionState::default();
    let mut response = WebSocketResponseState::default();
    observe_response_event(
        &mut response,
        r#"{"type":"response.completed","response":{"id":"resp-1","output":[]}}"#,
    );
    update_response_chain(&mut state, Some(request.clone()), &response, true);
    assert!(state.chain.is_some());
    state
}

#[tokio::test]
async fn websocket_handshake_426_preserves_capability_status() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 2_048];
        let _ = stream.read(&mut request).await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 426 Upgrade Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = format!("ws://{address}/v1/responses");
    let cancel_token = RunCancelToken::default();
    let error = send_websocket_async(
        ProviderWebSocketTransportInput {
            request_plan: ProviderStreamingHttpRequestPlan {
                method: "POST",
                url,
                headers: Vec::new(),
                auth_header: None,
                body_text: request_body(),
                server_conversation_state_allowed: true,
                remote_request_body_allowed: false,
                reasons: Vec::new(),
            },
            timeout_ms: FAILURE_TEST_TIMEOUT_MS,
            websocket_url: Some(websocket_url),
            logical_session: Some("websocket-426-session".to_owned()),
        },
        &cancel_token,
    )
    .await
    .expect_err("426 握手必须返回带状态码的 provider 错误");
    assert!(matches!(
        error,
        AxonError::Provider(axon_domain::ProviderError::Transport {
            http_status: Some(426),
            ..
        })
    ));
    server.await.unwrap();
}

fn request_body() -> String {
    serde_json::json!({
        "model": "test",
        "input": [{"type": "message", "role": "user", "content": "ping"}],
        "stream": true,
        "store": false
    })
    .to_string()
}
