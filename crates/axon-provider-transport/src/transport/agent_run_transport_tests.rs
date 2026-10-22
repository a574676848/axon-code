use super::*;

#[test]
fn raw_exchange_capture_keeps_request_head_and_tail_within_bounds() {
    const CAPTURE_LIMIT: usize = 512;
    let body = format!("head-{}-tail", "middle-".repeat(200));
    let (request, request_truncated) = bounded_head_and_tail(&body, CAPTURE_LIMIT);
    assert!(request_truncated);
    assert!(
        request.len() <= CAPTURE_LIMIT,
        "首部 + 省略标记 + 尾部必须落在上限内"
    );
    assert!(request.starts_with("head-"), "必须保留请求体首部");
    assert!(request.ends_with("-tail"), "必须保留请求体尾部");
    assert!(
        request.contains(PROVIDER_RAW_REQUEST_ELISION_PREFIX)
            && request.contains(PROVIDER_RAW_REQUEST_ELISION_SUFFIX),
        "中间省略必须有显式标记"
    );

    let (short_body, short_truncated) = bounded_head_and_tail("short-body", CAPTURE_LIMIT);
    assert_eq!(short_body, "short-body");
    assert!(!short_truncated);

    let (unicode_body, _) = bounded_head_and_tail(&"请求".repeat(200), CAPTURE_LIMIT);
    assert!(unicode_body.len() <= CAPTURE_LIMIT);
    assert!(std::str::from_utf8(unicode_body.as_bytes()).is_ok());
}

#[test]
fn raw_exchange_capture_keeps_the_response_tail_within_bounds() {
    let mut capture = ProviderRawExchangeCapture {
        protocol: ProviderStreamProtocol::OpenAiResponses,
        request_url: "https://provider.example/v1/responses".to_owned(),
        request_body: "request-body".to_owned(),
        request_truncated: false,
        response_frames: VecDeque::new(),
        response_bytes: 0,
        response_truncated: false,
    };
    for index in 0..(PROVIDER_RAW_RESPONSE_FRAME_LIMIT + 1) {
        capture.observe(&format!("frame-{index}"));
    }

    assert_eq!(
        capture.response_frames.len(),
        PROVIDER_RAW_RESPONSE_FRAME_LIMIT
    );
    assert!(capture.response_truncated);
    assert_eq!(capture.response_frames.front().unwrap(), "frame-1");
    assert_eq!(capture.response_frames.back().unwrap(), "frame-128");
}

fn session(
    logical_session: &str,
    preferred: ProviderStreamingWireTransport,
) -> AgentRunProviderTransportSession {
    AgentRunProviderTransportSession::new("provider", logical_session, "endpoint", preferred)
}

#[test]
fn websocket_fallback_is_sticky_after_retry_budget() {
    let logical_session = "session-retry-budget";
    let transport_session = session(logical_session, ProviderStreamingWireTransport::WebSocket);
    let error = ProviderStreamingTransportError::new("provider websocket 连接已关闭", None)
        .with_failure_kind(axon_agent_runtime::ProviderStreamingTransportFailureKind::Connection);

    assert!(!transport_session.record_websocket_failure(&error));
    assert!(!transport_session.record_websocket_failure(&error));
    assert!(transport_session.record_websocket_failure(&error));
    assert_eq!(
        transport_session.effective_transport(),
        ProviderStreamingWireTransport::Http
    );
    drop(transport_session);
    assert_eq!(
        session(logical_session, ProviderStreamingWireTransport::WebSocket).effective_transport(),
        ProviderStreamingWireTransport::Http
    );
}

#[test]
fn request_authentication_cancellation_and_rate_limit_do_not_disable_websocket() {
    use axon_agent_runtime::ProviderStreamingTransportFailureKind;

    for failure_kind in [
        ProviderStreamingTransportFailureKind::Request,
        ProviderStreamingTransportFailureKind::Authentication,
        ProviderStreamingTransportFailureKind::Cancelled,
        ProviderStreamingTransportFailureKind::RateLimited,
        ProviderStreamingTransportFailureKind::Upstream,
    ] {
        let transport_session = session(
            &format!("ignored-websocket-failure-{failure_kind:?}"),
            ProviderStreamingWireTransport::WebSocket,
        );
        let error = ProviderStreamingTransportError::new("provider failure", None)
            .with_failure_kind(failure_kind);

        for _ in 0..provider_streaming_retry_policy().websocket_failures_before_fallback {
            assert!(!transport_session.record_websocket_failure(&error));
        }
        assert_eq!(
            transport_session.effective_transport(),
            ProviderStreamingWireTransport::WebSocket
        );
    }
}

#[test]
fn retry_gate_wakes_when_cancel_token_is_triggered() {
    const CANCEL_TRIGGER_DELAY_MS: u64 = 5;

    let transport_session = session(
        "session-cancel-retry-gate",
        ProviderStreamingWireTransport::Http,
    );
    transport_session.record_request_failure(&ProviderStreamingTransportError::new(
        "connection reset",
        None,
    ));
    let cancel_token = RunCancelToken::default();
    let cancel_trigger = cancel_token.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(CANCEL_TRIGGER_DELAY_MS));
        cancel_trigger.cancel();
    });
    let started_at = Instant::now();

    assert!(!transport_session.wait_for_retry(&cancel_token));
    assert!(
        started_at.elapsed() < Duration::from_secs(1),
        "retry gate 未及时响应取消令牌"
    );
    trigger.join().unwrap();
}

#[test]
fn websocket_capability_failure_falls_back_immediately() {
    let session = session(
        "session-capability",
        ProviderStreamingWireTransport::WebSocket,
    );
    let error = ProviderStreamingTransportError::new("provider websocket 握手失败", Some(426));

    assert!(session.record_websocket_failure(&error));
    assert_eq!(
        session.effective_transport(),
        ProviderStreamingWireTransport::Http
    );
}

#[test]
fn startup_prewarm_fallback_is_seen_by_first_logical_session() {
    let endpoint = "http://prewarm-sticky-endpoint.test/v1/responses";
    mark_websocket_endpoint_fallback(endpoint);
    let transport_session = AgentRunProviderTransportSession::new(
        "provider-prewarm",
        "logical-session-prewarm",
        "http://prewarm-sticky-endpoint.test/v1",
        ProviderStreamingWireTransport::WebSocket,
    );
    assert_eq!(
        transport_session.effective_transport(),
        ProviderStreamingWireTransport::Http
    );
}

#[test]
fn expired_endpoint_fallback_is_reclaimed_for_new_sessions() {
    let endpoint = ProviderTransportEndpointKey("https://expired.example".to_owned());
    let now = Instant::now();
    let expired = now
        .checked_sub(Duration::from_millis(
            PROVIDER_WEBSOCKET_ENDPOINT_FALLBACK_COOLDOWN_MS,
        ))
        .unwrap_or(now);
    let mut endpoints = HashMap::from([(endpoint.clone(), expired)]);

    retain_active_fallback_endpoints(&mut endpoints, now);

    assert!(!endpoints.contains_key(&endpoint));
}

#[test]
fn websocket_capability_is_limited_to_openai_responses_protocol() {
    assert!(websocket_protocol_supported(
        ProviderStreamProtocol::OpenAiResponses
    ));
    assert!(!websocket_protocol_supported(
        ProviderStreamProtocol::OpenAiCompatible
    ));
    assert!(!websocket_protocol_supported(
        ProviderStreamProtocol::AnthropicMessages
    ));
}
