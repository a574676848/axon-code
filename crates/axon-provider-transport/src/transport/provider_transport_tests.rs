use super::*;
use axon_agent_runtime::RunCancelToken;
use axon_agent_runtime::{
    ProviderStreamingAuthHeaderPlan, ProviderStreamingCredentialPlan, ProviderStreamingHttpHeader,
    ProviderStreamingTransportFailureKind,
};
use reqwest::header::HeaderMap;
use std::{
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::Duration,
};

const LOCAL_HTTP_TEST_TIMEOUT: Duration = Duration::from_millis(500);
const LOCAL_HTTP_TEST_TIMEOUT_MS: u64 = 500;
const HTTP_HEADER_END: &[u8] = b"\r\n\r\n";
const TEST_AUTH_TOKEN: &str = "prewarm-token";

#[test]
fn bearer_fallback_requires_explicit_provider_instruction() {
    let request_plan = ProviderStreamingHttpRequestPlan {
        auth_header: Some(ProviderStreamingAuthHeaderPlan {
            credential: ProviderStreamingCredentialPlan::ApiKey("key".to_owned()),
            header_name: "x-api-key".to_owned(),
            value_prefix: String::new(),
        }),
        ..test_request_plan("http://127.0.0.1:1/explicit")
    };
    let explicit = StatusResponseParts {
        status: 401,
        retry_after_ms: None,
        request_id: None,
        message: "must start with Bearer in Authorization header".to_owned(),
    };
    assert!(should_retry_with_bearer(&request_plan, &explicit));

    let missing_key = StatusResponseParts {
        message: "{\"error\":{\"code\":\"invalid_api_key\",\"message\":\"missing_api_key\"}}"
            .to_owned(),
        status: 401,
        retry_after_ms: None,
        request_id: None,
    };
    assert!(should_retry_with_bearer(&request_plan, &missing_key));

    let generic = StatusResponseParts {
        message: "unauthorized".to_owned(),
        status: 401,
        retry_after_ms: None,
        request_id: None,
    };
    assert!(!should_retry_with_bearer(&request_plan, &generic));
}

#[test]
fn bearer_auth_choice_is_remembered_per_endpoint_and_credential() {
    let request_plan = ProviderStreamingHttpRequestPlan {
        auth_header: Some(ProviderStreamingAuthHeaderPlan {
            credential: ProviderStreamingCredentialPlan::ApiKey("memory-key".to_owned()),
            header_name: "x-api-key".to_owned(),
            value_prefix: String::new(),
        }),
        ..test_request_plan("http://127.0.0.1:1/remembered")
    };
    assert_eq!(
        preferred_auth_request_plan(&request_plan)
            .auth_header
            .unwrap()
            .header_name,
        "x-api-key"
    );
    remember_bearer_auth(&request_plan);
    let preferred = preferred_auth_request_plan(&request_plan);
    let auth = preferred.auth_header.unwrap();
    assert_eq!(auth.header_name, "Authorization");
    assert_eq!(auth.value_prefix, "Bearer ");
}

#[test]
fn http_client_is_initialized_once_for_all_request_timeouts() {
    assert!(shared_http_client_initialized());

    let first = shared_http_client().unwrap();
    let second = shared_http_client().unwrap();

    drop((first, second));
}

#[test]
fn http_prewarm_reuses_connection_and_request_headers_for_post() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (requests_tx, requests_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(LOCAL_HTTP_TEST_TIMEOUT))
            .unwrap();

        let head = read_http_request(&mut stream);
        write_http_response(&mut stream, "204 No Content", "");
        let post = read_http_request(&mut stream);
        write_http_response(&mut stream, "200 OK", "data: [DONE]\n\n");
        requests_tx.send((head, post)).unwrap();
    });
    let input = ProviderHttpTransportInput {
        request_plan: authenticated_test_request_plan(&format!("http://{address}/v1/messages")),
        timeout_ms: LOCAL_HTTP_TEST_TIMEOUT_MS,
        enable_request_compression: false,
    };

    ProviderHttpTransportRuntime.prewarm(&input).unwrap();
    let client = shared_http_client().unwrap();
    let request_body =
        ProviderHttpRequestBody::encode(&input.request_plan.body_text, false).unwrap();
    let outbound = build_request(&client, &input.request_plan, &request_body, None).unwrap();
    let status = shared_runtime().unwrap().block_on(async move {
        let response = outbound.send().await.unwrap();
        let status = response.status().as_u16();
        response.bytes().await.unwrap();
        status
    });
    let (head, post) = requests_rx.recv_timeout(LOCAL_HTTP_TEST_TIMEOUT).unwrap();
    server.join().unwrap();

    assert!(head.starts_with("HEAD /v1/messages HTTP/1.1"));
    assert!(post.starts_with("POST /v1/messages HTTP/1.1"));
    assert!(
        head.to_ascii_lowercase()
            .contains("authorization: bearer prewarm-token")
    );
    assert!(
        post.to_ascii_lowercase()
            .contains("authorization: bearer prewarm-token")
    );
    assert_eq!(status, 200);
}

#[test]
fn http_transport_defers_dropped_connection_to_outer_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(LOCAL_HTTP_TEST_TIMEOUT))
            .unwrap();
        let _ = read_http_request(&mut stream);
    });
    let mut request_plan = test_request_plan(&format!("http://{address}/v1/chat/completions"));
    request_plan.body_text = r#"{"model":"test","input":[],"stream":true}"#.to_owned();
    let input = ProviderHttpTransportInput {
        request_plan,
        timeout_ms: LOCAL_HTTP_TEST_TIMEOUT_MS,
        enable_request_compression: false,
    };

    let cancel_token = RunCancelToken::default();
    let outcome = ProviderHttpTransportRuntime
        .send(input, &cancel_token)
        .expect("断连应保留给 outer retry");
    server.join().unwrap();

    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        panic!("内层 transport 不得重发流式请求")
    };
    assert!(error.failure_kind.is_retryable());
}

#[test]
fn http_transport_compresses_request_when_capability_is_enabled() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(LOCAL_HTTP_TEST_TIMEOUT))
            .unwrap();
        let request = read_http_request_bytes(&mut stream);
        write_http_response(&mut stream, "200 OK", "data: [DONE]\n\n");
        request_tx.send(request).unwrap();
    });
    let request_body =
        r#"{"model":"test","input":[{"role":"user","content":"hello"}],"stream":true}"#;
    let mut request_plan = test_request_plan(&format!("http://{address}/v1/responses"));
    request_plan.body_text = request_body.to_owned();
    let input = ProviderHttpTransportInput {
        request_plan,
        timeout_ms: LOCAL_HTTP_TEST_TIMEOUT_MS,
        enable_request_compression: true,
    };

    let cancel_token = RunCancelToken::default();
    let outcome = ProviderHttpTransportRuntime
        .send(input, &cancel_token)
        .expect("压缩 HTTP 请求应成功");
    let request = request_rx.recv_timeout(LOCAL_HTTP_TEST_TIMEOUT).unwrap();
    server.join().unwrap();
    let header_end = request
        .windows(HTTP_HEADER_END.len())
        .position(|window| window == HTTP_HEADER_END)
        .map(|position| position + HTTP_HEADER_END.len())
        .unwrap();
    let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
    let decoded =
        zstd::stream::decode_all(Cursor::new(&request[header_end..])).expect("zstd 请求体应可解码");

    assert!(matches!(
        outcome,
        ProviderStreamingTransportOutcome::StreamCompleted {
            http_status: 200,
            ..
        }
    ));
    assert!(headers.contains(&format!(
        "content-encoding: {PROVIDER_HTTP_ZSTD_CONTENT_ENCODING}"
    )));
    assert_eq!(decoded, request_body.as_bytes());
}

#[test]
fn provider_response_headers_extract_retry_delay_and_request_id() {
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        reqwest::header::HeaderValue::from_static("2"),
    );
    headers.insert(
        "x-request-id",
        reqwest::header::HeaderValue::from_static("req-test-1"),
    );

    assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(2)));
    assert_eq!(provider_request_id(&headers).as_deref(), Some("req-test-1"));
}

#[test]
fn stream_completion_detection_requires_protocol_terminal_events() {
    assert!(stream_data_marks_completed("[DONE]"));
    assert!(stream_data_marks_completed(
        r#"{"type":"response.completed","response":{"status":"completed"}}"#
    ));
    assert!(stream_data_marks_completed(r#"{"type":"message_stop"}"#));
    assert!(!stream_data_marks_completed(
        r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#
    ));
    assert!(!stream_data_marks_completed(
        r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#
    ));
    assert!(!stream_data_marks_completed(
        r#"{"type":"response.output_text.delta","delta":"hi"}"#
    ));
    assert!(!stream_data_marks_completed(
        r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":null}]}"#
    ));
}

#[test]
fn normalized_sse_event_data_injects_named_event_type() {
    let data = normalized_sse_event_data("response.completed", r#"{"response":{"id":"r1"}}"#);

    assert!(data.contains(r#""type":"response.completed""#));
}

#[test]
fn http_transport_returns_cancelled_before_network_dispatch() {
    let input = ProviderHttpTransportInput {
        request_plan: test_request_plan("http://127.0.0.1:9/v1/responses"),
        timeout_ms: 50,
        enable_request_compression: false,
    };

    let cancel_token = RunCancelToken::default();
    cancel_token.cancel();
    let outcome = ProviderHttpTransportRuntime
        .send(input, &cancel_token)
        .expect("cancelled transport should return an outcome");

    match outcome {
        ProviderStreamingTransportOutcome::Error(error) => {
            assert_eq!(error.http_status, None);
            assert!(error.message.contains("已取消"));
        }
        other => {
            panic!("unexpected provider outcome: {other:?}")
        }
    }
}

#[tokio::test]
async fn http_transport_async_returns_cancelled_before_network_dispatch() {
    let input = ProviderHttpTransportInput {
        request_plan: test_request_plan("http://127.0.0.1:9/v1/responses"),
        timeout_ms: 50,
        enable_request_compression: false,
    };

    let cancel_token = RunCancelToken::default();
    cancel_token.cancel();
    let outcome = send_http_async(input, &cancel_token)
        .await
        .expect("cancelled transport should return an outcome");

    match outcome {
        ProviderStreamingTransportOutcome::Error(error) => {
            assert_eq!(error.http_status, None);
            assert!(error.message.contains("已取消"));
        }
        other => {
            panic!("unexpected provider outcome: {other:?}")
        }
    }
}

#[test]
fn http_transport_timeout_returns_error_outcome() {
    let input = ProviderHttpTransportInput {
        request_plan: test_request_plan("http://127.0.0.1:9/v1/responses"),
        timeout_ms: 1,
        enable_request_compression: false,
    };

    let cancel_token = RunCancelToken::default();
    let outcome = ProviderHttpTransportRuntime
        .send(input, &cancel_token)
        .expect("transport failure should be represented as an outcome");

    match outcome {
        ProviderStreamingTransportOutcome::Error(error) => {
            assert_eq!(error.http_status, None);
            assert!(!error.message.trim().is_empty());
        }
        other => {
            panic!("unexpected provider outcome: {other:?}")
        }
    }
}

#[test]
fn http_stream_read_timeout_after_headers_returns_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(LOCAL_HTTP_TEST_TIMEOUT))
            .unwrap();
        let _ = read_http_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
    });
    let input = ProviderHttpTransportInput {
        request_plan: test_request_plan(&format!("http://{address}/v1/chat/completions")),
        timeout_ms: 50,
        enable_request_compression: false,
    };

    let cancel_token = RunCancelToken::default();
    let mut events = Vec::new();
    let outcome = ProviderHttpTransportRuntime
        .send_with_line_callback(input, &cancel_token, |line| events.push(line.to_owned()))
        .expect("响应体读取超时应返回 transport outcome");
    server.join().unwrap();

    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        panic!("响应体静默时应返回错误 outcome");
    };
    assert_eq!(
        error.failure_kind,
        ProviderStreamingTransportFailureKind::Timeout
    );
    assert!(error.message.contains("响应读取超时"));
}

#[test]
fn http_stream_idle_timeout_resets_after_each_event() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\nTransfer-Encoding: chunked\r\n\r\n",
            )
            .unwrap();
        for event in [
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
        ] {
            if try_write_http_chunk(&mut stream, event.as_bytes()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        let _ = stream.write_all(b"0\r\n\r\n");
        let _ = stream.flush();
    });
    let input = ProviderHttpTransportInput {
        request_plan: test_request_plan(&format!("http://{address}/v1/responses")),
        timeout_ms: 70,
        enable_request_compression: false,
    };

    let mut events = Vec::new();
    let outcome = ProviderHttpTransportRuntime
        .send_with_line_callback(input, &RunCancelToken::default(), |line| {
            events.push(line.to_owned());
        })
        .expect("持续收到事件时不应触发 idle timeout");
    server.join().unwrap();

    let ProviderStreamingTransportOutcome::StreamCompleted { .. } = outcome else {
        panic!("持续事件流应完整完成");
    };
    assert!(
        events
            .iter()
            .any(|event| event.contains("response.completed"))
    );
}

#[test]
fn http_stream_partial_body_disconnect_returns_connection_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = read_http_request(&mut stream);
        let partial =
            b"data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"partial\"}}\n\n";
        write_partial_http_response(&mut stream, partial.len() + 128, partial);
    });
    let outcome = shared_runtime().unwrap().block_on(async move {
        let response = shared_http_client()
            .unwrap()
            .get(format!("http://{address}/stream"))
            .send()
            .await
            .unwrap();
        response_to_raw_stream(
            response,
            LOCAL_HTTP_TEST_TIMEOUT_MS,
            &RunCancelToken::default(),
        )
        .await
    });
    server.join().unwrap();

    let ProviderStreamingTransportOutcome::Error(error) = outcome else {
        panic!("partial SSE 断链不得返回半截完成结果");
    };
    assert_eq!(
        error.failure_kind,
        ProviderStreamingTransportFailureKind::Connection
    );
    assert!(error.message.contains("响应读取失败"));
}

fn authenticated_test_request_plan(url: &str) -> ProviderStreamingHttpRequestPlan {
    ProviderStreamingHttpRequestPlan {
        headers: vec![ProviderStreamingHttpHeader {
            name: "x-prewarm-test".to_owned(),
            value: "enabled".to_owned(),
        }],
        auth_header: Some(ProviderStreamingAuthHeaderPlan {
            credential: ProviderStreamingCredentialPlan::ApiKey(TEST_AUTH_TOKEN.to_owned()),
            header_name: "Authorization".to_owned(),
            value_prefix: "Bearer ".to_owned(),
        }),
        ..test_request_plan(url)
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    String::from_utf8_lossy(&read_http_request_bytes(stream)).into_owned()
}

fn read_http_request_bytes(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1_024];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "HTTP connection closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request
            .windows(HTTP_HEADER_END.len())
            .position(|window| window == HTTP_HEADER_END)
            .map(|position| position + HTTP_HEADER_END.len())
        else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let body_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")?
                    .trim()
                    .parse::<usize>()
                    .ok()
            })
            .unwrap_or_default();
        if request.len() >= header_end + body_length {
            request.truncate(header_end + body_length);
            return request;
        }
    }
}

fn write_http_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn write_partial_http_response(stream: &mut TcpStream, declared_bytes: usize, body: &[u8]) {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {declared_bytes}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(headers.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
}

fn try_write_http_chunk(stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    write!(stream, "{:X}\r\n", body.len())?;
    stream.write_all(body)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

fn test_request_plan(url: &str) -> ProviderStreamingHttpRequestPlan {
    ProviderStreamingHttpRequestPlan {
        method: "POST",
        url: url.to_owned(),
        headers: Vec::new(),
        auth_header: None,
        body_text: "{}".to_owned(),
        server_conversation_state_allowed: false,
        remote_request_body_allowed: false,
        reasons: Vec::new(),
    }
}
