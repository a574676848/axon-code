use super::*;
use axon_agent_runtime::RunCancelToken;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::WebSocketStream;

const DYNAMIC_TOOL_SESSION: &str = "dynamic-tool-session";
const INITIAL_PROMPT_CACHE_KEY: &str = "dynamic-tool-initial";
const ACTIVATED_PROMPT_CACHE_KEY: &str = "dynamic-tool-activated";
const BUILTIN_TOOL_NAME: &str = "builtin_search";
const MCP_LIST_REPOS_NAME: &str = "mcp__gitnexus__list_repos";

#[tokio::test]
async fn websocket_prewarm_is_claimed_and_chained_by_first_session() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handshakes = Arc::new(AtomicUsize::new(0));
    let server_handshakes = Arc::clone(&handshakes);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        server_handshakes.fetch_add(1, Ordering::SeqCst);
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();

        let warmup = next_request(&mut socket).await;
        assert_eq!(warmup["type"], "response.create");
        assert_eq!(warmup["generate"], false);
        assert_eq!(warmup["input"], serde_json::json!([]));
        assert!(warmup.get("stream").is_none());
        send_completed(&mut socket, "resp-warm", Vec::new()).await;

        let generated = next_request(&mut socket).await;
        assert_eq!(generated["previous_response_id"], "resp-warm");
        assert_eq!(generated["input"][0]["role"], "user");
        assert!(generated.get("generate").is_none());
        send_completed(&mut socket, "resp-first", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let prewarm = websocket_input(
        &url,
        websocket_url.clone(),
        None,
        response_body("startup", serde_json::json!([]), serde_json::json!([])),
    );
    prewarm_websocket(prewarm).await.unwrap();

    let request = websocket_input(
        &url,
        websocket_url,
        Some("session-claimed"),
        response_body(
            "session-claimed",
            serde_json::json!([user_message("hello")]),
            serde_json::json!([{"type":"function","name":"shell"}]),
        ),
    );
    let telemetry = websocket_connection_telemetry(&request);
    assert!(telemetry.connection_reused);
    assert!(telemetry.connection_age_ms.is_some());
    let cancel_token = RunCancelToken::default();
    let outcome = send_websocket_async(request, &cancel_token).await.unwrap();
    server.await.unwrap();

    assert!(matches!(
        outcome,
        ProviderStreamingTransportOutcome::StreamCompleted { .. }
    ));
    assert_eq!(handshakes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn websocket_tool_continuation_sends_only_incremental_output() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let first = next_request(&mut socket).await;
        assert!(first.get("previous_response_id").is_none());
        let function_call = serde_json::json!({
            "type":"function_call",
            "id":"fc-1",
            "call_id":"call-1",
            "name":"shell",
            "arguments":"{}",
            "status":"completed"
        });
        send_completed(&mut socket, "resp-1", vec![function_call]).await;

        let second = next_request(&mut socket).await;
        assert_eq!(second["previous_response_id"], "resp-1");
        assert_eq!(
            second["input"],
            serde_json::json!([{
                "type":"function_call_output",
                "call_id":"call-1",
                "output":"ok"
            }])
        );
        send_completed(&mut socket, "resp-2", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first_input = serde_json::json!([user_message("run")]);
    let first = websocket_input(
        &url,
        websocket_url.clone(),
        Some("tool-session"),
        response_body("tool-session", first_input.clone(), serde_json::json!([])),
    );
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();

    let second = websocket_input(
        &url,
        websocket_url,
        Some("tool-session"),
        response_body(
            "tool-session",
            serde_json::json!([
                first_input[0].clone(),
                {
                    "type":"function_call",
                    "call_id":"call-1",
                    "name":"shell",
                    "arguments":"{}"
                },
                {
                    "type":"function_call_output",
                    "call_id":"call-1",
                    "output":"ok"
                }
            ]),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn websocket_custom_tool_continuation_sends_only_incremental_output() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let first = next_request(&mut socket).await;
        assert!(first.get("previous_response_id").is_none());
        let custom_call = serde_json::json!({
            "type":"custom_tool_call",
            "id":"ctc-1",
            "call_id":"call-patch",
            "name":"builtin_apply_patch",
            "input":"*** Begin Patch\n*** End Patch",
            "status":"completed"
        });
        send_completed(&mut socket, "resp-patch-1", vec![custom_call]).await;

        let second = next_request(&mut socket).await;
        assert_eq!(second["previous_response_id"], "resp-patch-1");
        assert_eq!(
            second["input"],
            serde_json::json!([{
                "type":"custom_tool_call_output",
                "call_id":"call-patch",
                "output":"ok"
            }])
        );
        send_completed(&mut socket, "resp-patch-2", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first_input = serde_json::json!([user_message("patch")]);
    let first = websocket_input(
        &url,
        websocket_url.clone(),
        Some("custom-tool-session"),
        response_body(
            "custom-tool-session",
            first_input.clone(),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();

    let second = websocket_input(
        &url,
        websocket_url,
        Some("custom-tool-session"),
        response_body(
            "custom-tool-session",
            serde_json::json!([
                first_input[0].clone(),
                {
                    "type":"custom_tool_call",
                    "call_id":"call-patch",
                    "name":"builtin_apply_patch",
                    "input":"*** Begin Patch\n*** End Patch"
                },
                {
                    "type":"custom_tool_call_output",
                    "call_id":"call-patch",
                    "output":"ok"
                }
            ]),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn websocket_tool_schema_change_keeps_incremental_chain() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handshakes = Arc::new(AtomicUsize::new(0));
    let server = tokio::spawn(serve_tool_schema_change(listener, Arc::clone(&handshakes)));
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first_input = serde_json::json!([user_message("list repositories")]);
    let first = websocket_input(
        &url,
        websocket_url.clone(),
        Some(DYNAMIC_TOOL_SESSION),
        response_body(
            INITIAL_PROMPT_CACHE_KEY,
            first_input.clone(),
            serde_json::json!([function_tool(BUILTIN_TOOL_NAME)]),
        ),
    );
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();

    let second = websocket_input(
        &url,
        websocket_url,
        Some(DYNAMIC_TOOL_SESSION),
        response_body(
            ACTIVATED_PROMPT_CACHE_KEY,
            builtin_tool_followup_input(&first_input),
            serde_json::json!([
                function_tool(BUILTIN_TOOL_NAME),
                function_tool(MCP_LIST_REPOS_NAME)
            ]),
        ),
    );
    send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(handshakes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn websocket_pump_replies_to_ping_between_tool_requests() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (pong_tx, pong_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = next_request(&mut socket).await;
        let function_call = serde_json::json!({
            "type":"function_call",
            "id":"fc-ping",
            "call_id":"call-ping",
            "name":"shell",
            "arguments":"{}",
            "status":"completed"
        });
        send_completed(&mut socket, "resp-ping-1", vec![function_call]).await;
        let ping_payload = vec![1_u8, 2, 3];
        socket
            .send(Message::Ping(ping_payload.clone().into()))
            .await
            .unwrap();
        let pong = socket.next().await.unwrap().unwrap();
        assert_eq!(pong, Message::Pong(ping_payload.into()));
        let _ = pong_tx.send(());

        let second = next_request(&mut socket).await;
        assert_eq!(second["previous_response_id"], "resp-ping-1");
        send_completed(&mut socket, "resp-ping-2", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first_input = serde_json::json!([user_message("run")]);
    let first = websocket_input(
        &url,
        websocket_url.clone(),
        Some("tool-ping-session"),
        response_body(
            "tool-ping-session",
            first_input.clone(),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(500), pong_rx)
        .await
        .expect("工具执行期间应回复 provider ping")
        .unwrap();

    let second = websocket_input(
        &url,
        websocket_url,
        Some("tool-ping-session"),
        response_body(
            "tool-ping-session",
            serde_json::json!([
                first_input[0].clone(),
                {
                    "type":"function_call",
                    "call_id":"call-ping",
                    "name":"shell",
                    "arguments":"{}"
                },
                {
                    "type":"function_call_output",
                    "call_id":"call-ping",
                    "output":"ok"
                }
            ]),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn websocket_closed_between_tool_requests_reconnects_with_full_input() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (closed_tx, closed_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (first_stream, _) = listener.accept().await.unwrap();
        let mut first_socket = tokio_tungstenite::accept_async(first_stream).await.unwrap();
        let _ = next_request(&mut first_socket).await;
        let function_call = serde_json::json!({
            "type":"function_call",
            "id":"fc-close",
            "call_id":"call-close",
            "name":"shell",
            "arguments":"{}",
            "status":"completed"
        });
        send_completed(&mut first_socket, "resp-close-1", vec![function_call]).await;
        first_socket.close(None).await.unwrap();
        let _ = closed_tx.send(());

        let (second_stream, _) = listener.accept().await.unwrap();
        let mut second_socket = tokio_tungstenite::accept_async(second_stream)
            .await
            .unwrap();
        let second = next_request(&mut second_socket).await;
        assert!(second.get("previous_response_id").is_none());
        assert_eq!(second["input"].as_array().map(Vec::len), Some(3));
        send_completed(&mut second_socket, "resp-close-2", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first_input = serde_json::json!([user_message("run")]);
    let first = websocket_input(
        &url,
        websocket_url.clone(),
        Some("tool-close-session"),
        response_body(
            "tool-close-session",
            first_input.clone(),
            serde_json::json!([]),
        ),
    );
    let session_request =
        build_websocket_request(&first.request_plan, first.websocket_url.as_deref()).unwrap();
    let session = websocket_session(session_key(
        &session_request,
        first.logical_session.as_deref(),
    ));
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();
    closed_rx.await.unwrap();
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let closed = session
                .state
                .lock()
                .await
                .socket
                .as_ref()
                .is_none_or(ProviderWebSocketPump::is_closed);
            if closed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("远端关闭后后台 pump 应及时进入 closed 状态");

    let second = websocket_input(
        &url,
        websocket_url,
        Some("tool-close-session"),
        response_body(
            "tool-close-session",
            serde_json::json!([
                first_input[0].clone(),
                {
                    "type":"function_call",
                    "call_id":"call-close",
                    "name":"shell",
                    "arguments":"{}"
                },
                {
                    "type":"function_call_output",
                    "call_id":"call-close",
                    "output":"ok"
                }
            ]),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn missing_previous_response_returns_incomplete_for_outer_retry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = next_request(&mut socket).await;
        send_completed(&mut socket, "resp-1", Vec::new()).await;

        let chained = next_request(&mut socket).await;
        assert_eq!(chained["previous_response_id"], "resp-1");
        socket
            .send(Message::Text(
                serde_json::json!({
                    "type":"error",
                    "status":400,
                    "error":{
                        "code":"previous_response_not_found",
                        "message":"missing",
                        "param":"previous_response_id"
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();

        let recovered = next_request(&mut socket).await;
        assert!(recovered.get("previous_response_id").is_none());
        assert_eq!(recovered["input"].as_array().map(Vec::len), Some(2));
        send_completed(&mut socket, "resp-2", Vec::new()).await;
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    let first = websocket_input(
        &url,
        websocket_url.clone(),
        Some("recover-session"),
        response_body(
            "recover-session",
            serde_json::json!([user_message("one")]),
            serde_json::json!([]),
        ),
    );
    send_websocket_async(first, &RunCancelToken::default())
        .await
        .unwrap();
    let second = websocket_input(
        &url,
        websocket_url,
        Some("recover-session"),
        response_body(
            "recover-session",
            serde_json::json!([user_message("one"), user_message("two")]),
            serde_json::json!([]),
        ),
    );
    let first_retry_outcome = send_websocket_async(second.clone(), &RunCancelToken::default())
        .await
        .unwrap();
    let ProviderStreamingTransportOutcome::Error(error) = first_retry_outcome else {
        panic!("previous_response_not_found 必须交给 outer retry")
    };
    assert_eq!(
        error.failure_kind,
        ProviderStreamingTransportFailureKind::Incomplete
    );

    let outcome = send_websocket_async(second, &RunCancelToken::default())
        .await
        .unwrap();
    server.await.unwrap();

    let ProviderStreamingTransportOutcome::StreamCompleted { .. } = outcome else {
        panic!("expected recovered stream completion");
    };
}

#[tokio::test]
async fn websocket_uses_distinct_connections_for_logical_sessions() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handshakes = Arc::new(AtomicUsize::new(0));
    let server_handshakes = Arc::clone(&handshakes);
    let server = tokio::spawn(async move {
        for index in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            server_handshakes.fetch_add(1, Ordering::SeqCst);
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let _ = next_request(&mut socket).await;
            send_completed(&mut socket, &format!("resp-{index}"), Vec::new()).await;
        }
    });
    let url = format!("http://{address}/v1/responses");
    let websocket_url = Some(format!("ws://{address}/v1/responses"));
    for scope in ["session-a", "session-b"] {
        let input = websocket_input(
            &url,
            websocket_url.clone(),
            Some(scope),
            response_body("shared-cache", serde_json::json!([]), serde_json::json!([])),
        );
        send_websocket_async(input, &RunCancelToken::default())
            .await
            .unwrap();
    }
    server.await.unwrap();

    assert_eq!(handshakes.load(Ordering::SeqCst), 2);
}

#[path = "provider_transport_websocket_session_tests.rs"]
mod provider_transport_websocket_session_tests;
use provider_transport_websocket_session_tests::{next_request, send_completed};
fn websocket_input(
    url: &str,
    websocket_url: Option<String>,
    logical_session: Option<&str>,
    body_text: String,
) -> ProviderWebSocketTransportInput {
    ProviderWebSocketTransportInput {
        request_plan: ProviderStreamingHttpRequestPlan {
            method: "POST",
            url: url.to_owned(),
            headers: Vec::new(),
            auth_header: None,
            body_text,
            server_conversation_state_allowed: true,
            remote_request_body_allowed: false,
            reasons: Vec::new(),
        },
        timeout_ms: 2_000,
        websocket_url,
        logical_session: logical_session.map(ToOwned::to_owned),
    }
}

fn response_body(scope: &str, input: Value, tools: Value) -> String {
    serde_json::json!({
        "model":"gpt-5.6",
        "instructions":"",
        "input":input,
        "tools":tools,
        "tool_choice":"auto",
        "parallel_tool_calls":false,
        "store":false,
        "stream":true,
        "include":[],
        "prompt_cache_key":scope
    })
    .to_string()
}

fn user_message(text: &str) -> Value {
    serde_json::json!({
        "type":"message",
        "role":"user",
        "content":[{"type":"input_text","text":text}]
    })
}

async fn serve_tool_schema_change(listener: tokio::net::TcpListener, handshakes: Arc<AtomicUsize>) {
    let (stream, _) = listener.accept().await.unwrap();
    handshakes.fetch_add(1, Ordering::SeqCst);
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    let first = next_request(&mut socket).await;
    assert_eq!(first["tools"].as_array().map(Vec::len), Some(1));
    let function_call = serde_json::json!({
        "type":"function_call",
        "id":"fc-search",
        "call_id":"call-search",
        "name":BUILTIN_TOOL_NAME,
        "arguments":"{}",
        "status":"completed"
    });
    send_completed(&mut socket, "resp-search", vec![function_call]).await;

    let second = next_request(&mut socket).await;
    assert_eq!(second["previous_response_id"], "resp-search");
    assert_eq!(second["input"].as_array().map(Vec::len), Some(1));
    assert_eq!(second["input"][0]["type"], "function_call_output");
    assert_eq!(second["tools"].as_array().map(Vec::len), Some(2));
    assert_eq!(second["tools"][1]["name"], MCP_LIST_REPOS_NAME);
    send_completed(&mut socket, "resp-mcp", Vec::new()).await;
}

fn builtin_tool_followup_input(first_input: &Value) -> Value {
    serde_json::json!([
        first_input[0].clone(),
        {
            "type":"function_call",
            "call_id":"call-search",
            "name":BUILTIN_TOOL_NAME,
            "arguments":"{}"
        },
        {
            "type":"function_call_output",
            "call_id":"call-search",
            "output":"activated"
        }
    ])
}

fn function_tool(name: &str) -> Value {
    serde_json::json!({
        "type":"function",
        "name":name
    })
}
