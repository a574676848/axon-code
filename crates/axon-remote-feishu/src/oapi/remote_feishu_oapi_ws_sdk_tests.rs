use super::*;
use crate::remote_feishu_oapi_ws_frame_codec::WS_FRAME_MAX_HEADERS;
use crate::remote_feishu_oapi_ws_ping::{
    DEFAULT_PING_INTERVAL_SECONDS, feishu_oapi_ws_ping_interval_seconds,
};

#[test]
fn oapi_ws_sdk_builds_bootstrap_request_and_endpoint() {
    let config = RemoteFeishuOapiWsSdkConfig::new("cli_xxx".to_owned(), "secret".to_owned());
    let request =
        RemoteFeishuOapiWsSdkRuntime::bootstrap_request(&config).expect("bootstrap request");
    let endpoint =
        RemoteFeishuOapiWsSdkRuntime::endpoint_from_response(RemoteFeishuOapiWsEndpointResponse {
            code: 0,
            msg: "ok".to_owned(),
            data: Some(RemoteFeishuOapiWsEndpoint {
                url: "wss://open.feishu.cn/ws?device_id=d1&service_id=42".to_owned(),
                client_config: Some(RemoteFeishuOapiWsClientConfig {
                    reconnect_count: -1,
                    reconnect_interval: 120,
                    reconnect_nonce: 30,
                    ping_interval: 120,
                }),
            }),
        })
        .expect("endpoint");

    assert_eq!(request.app_id, "cli_xxx");
    let request_json = serde_json::to_value(&request).expect("request json");
    assert_eq!(request_json["AppID"], "cli_xxx");
    assert!(request_json.get("AppId").is_none());
    assert_eq!(
        RemoteFeishuOapiWsSdkRuntime::bootstrap_url("https://open.feishu.cn/"),
        "https://open.feishu.cn/callback/ws/endpoint"
    );
    assert_eq!(
        RemoteFeishuOapiWsSdkRuntime::service_id_from_endpoint(&endpoint.url).expect("service id"),
        42
    );
}

#[test]
fn oapi_ws_sdk_extracts_http_error_message_without_request_secret() {
    let message = feishu_oapi_ws_endpoint_error_message(
        r#"{"code":19001,"msg":"app websocket event is disabled"}"#,
    );

    assert_eq!(message, "app websocket event is disabled");
    assert!(!message.contains("AppSecret"));
}

#[test]
fn oapi_ws_sdk_frame_codec_round_trips_ping_frame() {
    let frame = RemoteFeishuOapiWsSdkRuntime::ping_frame(42);
    let bytes = encode_frame(&frame).expect("encode");
    let decoded = decode_frame(&bytes).expect("decode");

    assert_eq!(decoded.service, 42);
    assert_eq!(decoded.method, FRAME_TYPE_CONTROL);
    assert_eq!(
        header_value(&decoded.headers, HEADER_TYPE).as_deref(),
        Some(MESSAGE_TYPE_PING)
    );
}

#[test]
fn oapi_ws_sdk_frame_codec_rejects_header_count_above_bound() {
    let frame = RemoteFeishuOapiWsFrame {
        headers: vec![RemoteFeishuOapiWsHeader::default(); WS_FRAME_MAX_HEADERS + 1],
        ..RemoteFeishuOapiWsFrame::default()
    };
    let encode_error = encode_frame(&frame).expect_err("超限 header 必须不能编码");
    assert!(format!("{encode_error:?}").contains("header 数量超出上限"));

    let mut bytes = Vec::new();
    for _ in 0..=WS_FRAME_MAX_HEADERS {
        bytes.extend([0x2a, 0]);
    }
    let decode_error = decode_frame(&bytes).expect_err("超限 header 必须不能解码");
    assert!(format!("{decode_error:?}").contains("header 数量超出上限"));
}

#[tokio::test]
async fn oapi_ws_sdk_handles_event_data_frame_and_builds_response() {
    let frame = RemoteFeishuOapiWsFrame {
        service: 42,
        method: FRAME_TYPE_DATA,
        headers: vec![
            RemoteFeishuOapiWsHeader {
                key: HEADER_TYPE.to_owned(),
                value: MESSAGE_TYPE_EVENT.to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: HEADER_MESSAGE_ID.to_owned(),
                value: "msg-1".to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: HEADER_TRACE_ID.to_owned(),
                value: "trace-1".to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: "sum".to_owned(),
                value: "1".to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: "seq".to_owned(),
                value: "0".to_owned(),
            },
        ],
        payload: br#"{"event":{"message":{"message_id":"om_1"}}}"#.to_vec(),
        ..RemoteFeishuOapiWsFrame::default()
    };
    let bytes = encode_frame(&frame).expect("encode");

    let report = RemoteFeishuOapiWsSdkRuntime::handle_binary_frame(&bytes, |payload| async move {
        assert_eq!(payload.kind, MESSAGE_TYPE_EVENT);
        assert_eq!(payload.message_id, "msg-1");
        Ok(None)
    })
    .await
    .expect("handle");

    assert!(report.incoming.is_some());
    let response = decode_frame(&report.response_frame.expect("response")).expect("decode");
    assert_eq!(response.method, FRAME_TYPE_DATA);
    assert!(header_value(&response.headers, HEADER_BIZ_RT).is_some());
    let body = serde_json::from_slice::<Value>(&response.payload).expect("response json");
    assert_eq!(body["code"], HTTP_STATUS_OK);
}

#[tokio::test]
async fn oapi_ws_sdk_marks_callback_stop_after_response() {
    let frame = RemoteFeishuOapiWsFrame {
        service: 42,
        method: FRAME_TYPE_DATA,
        headers: vec![
            RemoteFeishuOapiWsHeader {
                key: HEADER_TYPE.to_owned(),
                value: MESSAGE_TYPE_EVENT.to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: HEADER_MESSAGE_ID.to_owned(),
                value: "msg-1".to_owned(),
            },
        ],
        payload: br#"{"event":{"message":{"message_id":"om_1"}}}"#.to_vec(),
        ..RemoteFeishuOapiWsFrame::default()
    };
    let bytes = encode_frame(&frame).expect("encode");

    let report = RemoteFeishuOapiWsSdkRuntime::handle_binary_frame(&bytes, |_| async {
        Ok(Some(serde_json::json!({
            "axonControl": {
                "stop": true
            }
        })))
    })
    .await
    .expect("handle");

    assert!(report.stop_requested);
    assert!(report.response_frame.is_some());
}

#[tokio::test]
async fn oapi_ws_sdk_reads_pong_client_config() {
    let frame = RemoteFeishuOapiWsFrame {
        service: 42,
        method: FRAME_TYPE_CONTROL,
        headers: vec![RemoteFeishuOapiWsHeader {
            key: HEADER_TYPE.to_owned(),
            value: MESSAGE_TYPE_PONG.to_owned(),
        }],
        payload:
            br#"{"ReconnectCount":3,"ReconnectInterval":5,"ReconnectNonce":1,"PingInterval":7}"#
                .to_vec(),
        ..RemoteFeishuOapiWsFrame::default()
    };
    let bytes = encode_frame(&frame).expect("encode");
    let report = RemoteFeishuOapiWsSdkRuntime::handle_binary_frame(&bytes, |_| async { Ok(None) })
        .await
        .expect("handle");

    assert_eq!(
        report.client_config.expect("client config").ping_interval,
        7
    );
    assert!(report.response_frame.is_none());
}

#[test]
fn oapi_ws_sdk_combines_fragmented_data_frames() {
    let mut fragments = FragmentBuffer::new();
    let first = fragmented_frame(0, br#"{"event":{"#);
    let second = fragmented_frame(1, br#""message":{"message_id":"om_1"}}}"#);

    assert!(
        combine_fragmented_frame(first, &mut fragments)
            .expect("first fragment")
            .is_none()
    );
    let combined = combine_fragmented_frame(second, &mut fragments)
        .expect("second fragment")
        .expect("combined frame");

    assert_eq!(
        String::from_utf8(combined.payload).expect("payload"),
        r#"{"event":{"message":{"message_id":"om_1"}}}"#
    );
    assert_eq!(fragments.buffered_bytes(), 0);
}

#[test]
fn oapi_ws_sdk_replaces_duplicate_fragment_without_leaking_budget() {
    let mut fragments = FragmentBuffer::new();

    assert!(
        combine_fragmented_frame(fragmented_frame(0, b"old"), &mut fragments)
            .expect("首次分片")
            .is_none()
    );
    assert!(
        combine_fragmented_frame(fragmented_frame(0, b"new"), &mut fragments)
            .expect("覆盖分片")
            .is_none()
    );

    assert_eq!(fragments.buffered_bytes(), 3);
}

#[test]
fn oapi_ws_sdk_discards_fragment_buffer_after_inconsistent_part_count() {
    let mut fragments = FragmentBuffer::new();
    assert!(
        combine_fragmented_frame(fragmented_frame(0, b"part"), &mut fragments)
            .expect("首次分片")
            .is_none()
    );
    let mut inconsistent = fragmented_frame(1, b"part");
    let sum = inconsistent
        .headers
        .iter_mut()
        .find(|header| header.key == HEADER_SUM)
        .expect("sum header");
    sum.value = "3".to_owned();

    let error = combine_fragmented_frame(inconsistent, &mut fragments)
        .expect_err("数量不一致的分片必须被拒绝");

    assert!(format!("{error:?}").contains("分片数量不一致"));
    assert_eq!(fragments.buffered_bytes(), 0);
    assert!(fragments.is_empty());
}

#[test]
fn oapi_ws_sdk_rejects_fragment_count_above_bound() {
    let mut fragments = FragmentBuffer::new();
    let mut frame = fragmented_frame(0, b"part");
    let sum = frame
        .headers
        .iter_mut()
        .find(|header| header.key == HEADER_SUM)
        .expect("sum header");
    sum.value = (WS_FRAGMENT_MAX_PARTS + 1).to_string();

    let error = combine_fragmented_frame(frame, &mut fragments).expect_err("超限分片必须被拒绝");

    assert!(format!("{error:?}").contains("分片数量超出上限"));
    assert!(fragments.is_empty());
}

#[test]
fn oapi_ws_sdk_rejects_fragment_with_oversized_message_id() {
    let mut fragments = FragmentBuffer::new();
    let mut frame = fragmented_frame(0, b"part");
    let message_id = frame
        .headers
        .iter_mut()
        .find(|header| header.key == HEADER_MESSAGE_ID)
        .expect("message id header");
    message_id.value = "x".repeat(WS_FRAGMENT_MAX_MESSAGE_ID_BYTES + 1);

    let error = combine_fragmented_frame(frame, &mut fragments)
        .expect_err("超限 message_id 的分片必须被拒绝");

    assert!(format!("{error:?}").contains("message_id 超出上限"));
    assert!(fragments.is_empty());
}

#[test]
fn oapi_ws_sdk_rejects_new_fragment_when_inflight_bound_is_reached() {
    let mut fragments = FragmentBuffer::new();
    for index in 0..WS_FRAGMENT_MAX_INFLIGHT_MESSAGES {
        let mut frame = fragmented_frame(0, b"part");
        let message_id = frame
            .headers
            .iter_mut()
            .find(|header| header.key == HEADER_MESSAGE_ID)
            .expect("message id header");
        message_id.value = format!("msg-{index}");
        assert!(
            combine_fragmented_frame(frame, &mut fragments)
                .expect("分片应可缓存")
                .is_none()
        );
    }

    let error = combine_fragmented_frame(fragmented_frame(0, b"part"), &mut fragments)
        .expect_err("超过并发缓存上限必须被拒绝");

    assert!(format!("{error:?}").contains("未完成分片消息数超出上限"));
}

#[test]
fn oapi_ws_sdk_rejects_fragment_when_buffered_bytes_reach_bound() {
    let mut fragments = FragmentBuffer::new();
    for index in 0..WS_FRAGMENT_MAX_BUFFERED_MESSAGES {
        let payload = vec![0; WS_FRAGMENT_MAX_MESSAGE_BYTES];
        let mut frame = fragmented_frame(0, &payload);
        let message_id = frame
            .headers
            .iter_mut()
            .find(|header| header.key == HEADER_MESSAGE_ID)
            .expect("message id header");
        message_id.value = format!("buffered-{index}");
        assert!(
            combine_fragmented_frame(frame, &mut fragments)
                .expect("分片应可缓存")
                .is_none()
        );
    }
    assert_eq!(fragments.buffered_bytes(), WS_FRAGMENT_MAX_BUFFERED_BYTES);

    let error = combine_fragmented_frame(fragmented_frame(0, b"part"), &mut fragments)
        .expect_err("超出 aggregate budget 的分片必须被拒绝");

    assert!(format!("{error:?}").contains("未完成分片缓存超出上限"));
    assert_eq!(fragments.buffered_bytes(), WS_FRAGMENT_MAX_BUFFERED_BYTES);
}

#[test]
fn oapi_ws_sdk_caps_transport_frames_to_fragment_budget() {
    let config = feishu_oapi_ws_transport_config();

    assert_eq!(config.max_message_size, Some(WS_FRAGMENT_MAX_MESSAGE_BYTES));
    assert_eq!(config.max_frame_size, Some(WS_FRAGMENT_MAX_MESSAGE_BYTES));
}

#[test]
fn oapi_ws_sdk_applies_server_reconnect_policy() {
    let config = RemoteFeishuOapiWsClientConfig {
        reconnect_count: 2,
        reconnect_interval: 7,
        reconnect_nonce: 3,
        ping_interval: 11,
    };
    let policy = config.reconnect_policy();

    assert!(policy.permits_retry(1));
    assert!(policy.permits_retry(2));
    assert!(!policy.permits_retry(3));
    assert_eq!(
        policy.delay_before_retry(1, 1_234),
        Duration::from_millis(1_234)
    );
    assert_eq!(policy.delay_before_retry(2, 1_234), Duration::from_secs(7));
}

#[test]
fn oapi_ws_sdk_uses_official_reconnect_defaults_and_shared_config_state() {
    let config = RemoteFeishuOapiWsClientConfig::default();
    let state = crate::remote_feishu_oapi_ws_loop::RemoteFeishuOapiWsClientConfigState::default();
    let control = RemoteFeishuOapiWsLoopControl::new(None).with_client_config_state(state.clone());

    assert_eq!(config.reconnect_count, -1);
    assert_eq!(config.reconnect_interval, 120);
    assert_eq!(config.reconnect_nonce, 30);
    assert!(config.reconnect_policy().permits_retry(usize::MAX));

    control.replace_client_config(RemoteFeishuOapiWsClientConfig {
        reconnect_count: 0,
        reconnect_interval: 1,
        reconnect_nonce: 0,
        ping_interval: 1,
    });
    assert_eq!(state.snapshot().reconnect_count, 0);
}

#[test]
fn oapi_ws_sdk_uses_default_ping_interval_when_server_omits_it() {
    let seconds = feishu_oapi_ws_ping_interval_seconds(RemoteFeishuOapiWsClientConfig {
        ping_interval: 0,
        ..RemoteFeishuOapiWsClientConfig::default()
    });

    assert_eq!(seconds, DEFAULT_PING_INTERVAL_SECONDS);
}

#[test]
fn oapi_ws_sdk_uses_server_ping_interval_when_present() {
    let seconds = feishu_oapi_ws_ping_interval_seconds(RemoteFeishuOapiWsClientConfig {
        ping_interval: 7,
        ..RemoteFeishuOapiWsClientConfig::default()
    });

    assert_eq!(seconds, 7);
}

#[test]
fn oapi_ws_sdk_preserves_peer_close_code_and_reason() {
    let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
        code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy,
        reason: "server rotation".into(),
    };
    let reason = close_frame_reason(Some(&frame));

    assert!(reason.contains("Policy"));
    assert!(reason.contains("server rotation"));
}

#[test]
fn oapi_ws_sdk_uses_bounded_read_idle_timeout() {
    assert_eq!(WS_READ_IDLE_TIMEOUT_SECS, 180);
}

fn fragmented_frame(seq: usize, payload: &[u8]) -> RemoteFeishuOapiWsFrame {
    RemoteFeishuOapiWsFrame {
        method: FRAME_TYPE_DATA,
        headers: vec![
            RemoteFeishuOapiWsHeader {
                key: HEADER_TYPE.to_owned(),
                value: MESSAGE_TYPE_EVENT.to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: HEADER_MESSAGE_ID.to_owned(),
                value: "msg-fragment".to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: HEADER_SUM.to_owned(),
                value: "2".to_owned(),
            },
            RemoteFeishuOapiWsHeader {
                key: HEADER_SEQ.to_owned(),
                value: seq.to_string(),
            },
        ],
        payload: payload.to_vec(),
        ..RemoteFeishuOapiWsFrame::default()
    }
}
