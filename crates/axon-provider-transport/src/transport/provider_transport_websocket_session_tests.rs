use super::*;

#[tokio::test]
async fn websocket_transport_returns_cancelled_before_connect() {
    let input = websocket_input(
        "http://127.0.0.1:9/v1/responses",
        Some("ws://127.0.0.1:9/v1/responses".to_owned()),
        Some("cancelled"),
        response_body("cancelled", serde_json::json!([]), serde_json::json!([])),
    );
    let cancel_token = RunCancelToken::default();
    cancel_token.cancel();
    let outcome = send_websocket_async(input, &cancel_token).await.unwrap();

    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        panic!("expected cancelled transport error");
    };
    assert!(error.message.contains("已取消"));
}

#[tokio::test]
async fn websocket_generated_request_requires_logical_session() {
    let input = websocket_input(
        "http://127.0.0.1:9/v1/responses",
        Some("ws://127.0.0.1:9/v1/responses".to_owned()),
        None,
        response_body("shared-cache", serde_json::json!([]), serde_json::json!([])),
    );

    let error = send_websocket_async(input, &RunCancelToken::default())
        .await
        .unwrap_err();

    assert!(format!("{error:?}").contains("缺少逻辑会话"));
}

#[tokio::test]
async fn websocket_stream_frame_callback_observes_intermediate_events() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = next_request(&mut socket).await;
        socket
            .send(Message::Text(
                serde_json::json!({
                    "type":"response.output_text.delta",
                    "delta":"partial"
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        send_completed(&mut socket, "resp-callback", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let input = websocket_input(
        &url,
        Some(format!("ws://{address}/v1/responses")),
        Some("callback-session"),
        response_body(
            "callback-session",
            serde_json::json!([user_message("hello")]),
            serde_json::json!([]),
        ),
    );
    let frames = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = Arc::clone(&frames);
    let outcome = send_websocket_async_with_frame_and_progress_callback(
        input,
        &RunCancelToken::default(),
        move |frame| {
            observed.lock().unwrap().push(frame.to_owned());
        },
        || {},
    )
    .await
    .unwrap();
    server.await.unwrap();

    assert!(matches!(
        outcome,
        ProviderStreamingTransportOutcome::StreamCompleted { .. }
    ));
    let frames = frames.lock().unwrap();
    assert_eq!(frames.len(), 2);
    assert!(frames[0].contains("response.output_text.delta"));
    assert!(frames[1].contains("response.completed"));
}

#[tokio::test]
async fn websocket_disabled_server_state_sends_full_input_for_each_request() {
    const SESSION: &str = "websocket-stateless-session";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let first = next_request(&mut socket).await;
        assert!(first.get("previous_response_id").is_none());
        assert_eq!(first["input"].as_array().map(Vec::len), Some(1));
        send_completed(&mut socket, "resp-stateless-1", Vec::new()).await;

        let second = next_request(&mut socket).await;
        assert!(second.get("previous_response_id").is_none());
        assert_eq!(second["input"].as_array().map(Vec::len), Some(2));
        send_completed(&mut socket, "resp-stateless-2", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first_body = response_body(
        SESSION,
        serde_json::json!([user_message("first")]),
        serde_json::json!([]),
    );
    let mut first = websocket_input(&url, websocket_url.clone(), Some(SESSION), first_body);
    first.request_plan.server_conversation_state_allowed = false;
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();

    let second_body = response_body(
        SESSION,
        serde_json::json!([user_message("first"), user_message("second")]),
        serde_json::json!([]),
    );
    let mut second = websocket_input(&url, websocket_url, Some(SESSION), second_body);
    second.request_plan.server_conversation_state_allowed = false;
    send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();
}

pub(super) async fn next_request(socket: &mut WebSocketStream<TcpStream>) -> Value {
    let message = socket.next().await.unwrap().unwrap();
    let Message::Text(text) = message else {
        panic!("expected text request");
    };
    serde_json::from_str(text.as_ref()).unwrap()
}

pub(super) async fn send_completed(
    socket: &mut WebSocketStream<TcpStream>,
    response_id: &str,
    output: Vec<Value>,
) {
    socket
        .send(Message::Text(
            serde_json::json!({
                "type":"response.completed",
                "response":{
                    "id":response_id,
                    "output":output
                }
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
}
