//! 探活的失败三分类与载荷严格解析。
//!
//! 这里的两条 fixture 起在回环上：一条固定返回 401，一条指向已关闭端口。
//! 自签证书那条属于阶段 1 的 TLS 施工项，与 TLS 客户端改造同一批交付。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use super::{
    Deadline, PROTOCOL_VERSION, ProbeFailure, ProbeOutcome, WsProbeOutcome, parse_meta,
    probe_base_with_token, probe_websocket, ws_status_outcome,
};

/// 起一个只应答一次的固定状态码服务，返回它的根地址。
fn spawn_status_server(status_line: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("回环端口应可绑定");
    let address = listener.local_addr().expect("应能取回真实端口");
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            let response = format!(
                "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{address}")
}

/// 取一个必然没有监听者的回环端口。
fn closed_port_base() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("回环端口应可绑定");
    let address = listener.local_addr().expect("应能取回真实端口");
    drop(listener);
    // 端口刚被释放，短时间内不会有别的进程占用它。
    format!("http://{address}")
}

#[test]
fn unauthorized_fixture_is_classified_as_credential_rejection() {
    let base = spawn_status_server("HTTP/1.1 401 Unauthorized", "{}");
    let outcome = probe_base_with_token(&base, Some("sk-wrong"));
    match outcome {
        ProbeOutcome::Failed(ProbeFailure::CredentialRejected { status }) => {
            assert_eq!(status, 401);
        }
        other => panic!("401 必须归为凭据不匹配，实际：{other:?}"),
    }
}

#[test]
fn closed_port_fixture_is_classified_as_unreachable() {
    let base = closed_port_base();
    let outcome = probe_base_with_token(&base, None);
    match outcome {
        ProbeOutcome::Failed(ProbeFailure::Unreachable(_)) => {}
        other => panic!("已关闭端口必须归为连不上，实际：{other:?}"),
    }
}

#[test]
fn certificate_failures_are_never_reported_as_unreachable() {
    let failure = super::classify_transport_failure(&ureq::Error::Tls("invalid peer certificate"));
    assert!(matches!(failure, ProbeFailure::CertificateInvalid(_)));
    assert!(failure.message().contains("证书"), "{failure:?}");

    let failure = super::classify_transport_failure(&ureq::Error::TlsRequired);
    assert!(matches!(failure, ProbeFailure::CertificateInvalid(_)));

    // 明确是 DNS 失败时仍然算连不上，不能被证书关键词兜走。
    let failure = super::classify_transport_failure(&ureq::Error::HostNotFound);
    assert!(matches!(failure, ProbeFailure::Unreachable(_)));
}

#[test]
fn tls_keywords_from_wrapped_io_errors_still_classify_as_certificate() {
    assert!(super::looks_like_tls_failure(
        "received fatal alert: CertificateUnknown"
    ));
    assert!(super::looks_like_tls_failure("handshake failure"));
    assert!(!super::looks_like_tls_failure("connection refused"));
}

#[test]
fn meta_payload_requires_the_five_declared_fields() {
    let body = format!(
        r#"{{"protocolVersion":{PROTOCOL_VERSION},
            "capabilities":{{"files":true,"dirs":true,"attachments":true,"config":true,
              "secrets":true,"providers":true,"mcp":true,"skills":true,"documents":true,
              "system_status":true,"service_control":true,"terminal":false}},
            "serverTimeMs":1760000000000,
            "uploadLimitBytes":67108864,
            "superAdmin":true}}"#
    );
    let meta = parse_meta(&body).expect("五字段齐全应解析成功");
    assert_eq!(meta.protocol_version, PROTOCOL_VERSION);
    assert!(meta.super_admin);
    assert_eq!(meta.upload_limit_bytes, 64 * 1024 * 1024);
    assert!(!meta.capabilities.terminal);

    let missing: serde_json::Value = serde_json::json!({"protocolVersion": 1});
    let failure = parse_meta(&missing.to_string()).expect_err("缺字段必须失败");
    assert!(matches!(failure, ProbeFailure::BodyInvalid(_)));
}

#[test]
fn unknown_protocol_version_asks_the_user_to_upgrade_the_client() {
    let body = serde_json::json!({
        "protocolVersion": 7,
        "capabilities": {},
        "serverTimeMs": 1,
        "uploadLimitBytes": 1,
        "superAdmin": false
    });
    let failure = parse_meta(&body.to_string()).expect_err("未知协议版本必须失败");
    match failure {
        ProbeFailure::ProtocolUnsupported(version) => assert_eq!(version, 7),
        other => panic!("应归为协议不受支持，实际：{other:?}"),
    }
    assert!(
        failure.message().contains("升级客户端"),
        "{}",
        failure.message()
    );
    assert!(failure.is_outdated_backend());
}

#[test]
fn outdated_backend_maps_to_a_minimal_capability_set_with_a_version_warning() {
    let caps = ProbeOutcome::Outdated.caps_state();
    assert!(caps.backend_outdated(), "老后端必须给出升级后台的结论");
    let set = caps.set().expect("老后端仍属已知能力位");
    assert!(!set.config, "老后端不含本协议的路由族");
}

#[test]
fn failed_probe_maps_to_unknown_instead_of_outdated() {
    let caps =
        ProbeOutcome::Failed(ProbeFailure::Unreachable("连接被拒绝".to_owned())).caps_state();
    assert!(caps.set().is_none());
    assert!(!caps.backend_outdated(), "连不上不得被判为版本过旧");
}

#[test]
fn status_server_that_closes_early_does_not_panic() {
    // 只监听不读也不写的服务：探活必须给结论而不是卡死或 panic。
    let listener = TcpListener::bind("127.0.0.1:0").expect("回环端口应可绑定");
    let address = listener.local_addr().expect("应能取回真实端口");
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            drop(stream);
        }
    });
    let outcome = probe_base_with_token(&format!("http://{address}"), None);
    assert!(
        matches!(outcome, ProbeOutcome::Failed(_)),
        "半开连接必须归为失败，实际：{outcome:?}"
    );
}

#[test]
fn deadline_shrinks_and_reports_exhaustion() {
    let deadline = Deadline::after(Duration::from_millis(200));
    let first = deadline.remaining().expect("预算刚开始，必须还有剩余");
    assert!(
        first <= Duration::from_millis(200),
        "剩余预算不得超过总预算"
    );
    thread::sleep(Duration::from_millis(20));
    let second = deadline.remaining().expect("20 毫秒后仍有剩余");
    assert!(
        second < first,
        "派生请求共享同一份截止时间：剩余预算必须随时间递减，实际 {second:?} 不小于 {first:?}"
    );
    let expired = Deadline::after(Duration::ZERO);
    assert!(
        expired.remaining().is_none(),
        "零预算必须立刻判为耗尽，不能让派生请求再吃满一份超时"
    );
}

#[test]
fn websocket_status_mapping_only_accepts_upgrade_semantics() {
    for status in [101_u16, 400, 426] {
        assert_eq!(
            ws_status_outcome(status),
            WsProbeOutcome::Ready,
            "HTTP {status} 表明该端口是 WebSocket 监听（升级成功或拒绝非升级请求）"
        );
    }
    assert_eq!(ws_status_outcome(404), WsProbeOutcome::WrongPort);
    for status in [401_u16, 403] {
        assert_eq!(
            ws_status_outcome(status),
            WsProbeOutcome::CredentialRejected { status }
        );
    }
    for status in [200_u16, 500, 502, 304] {
        assert_eq!(
            ws_status_outcome(status),
            WsProbeOutcome::Unexpected { status },
            "HTTP {status} 无法证明该端口是事件流监听，不得算作 Ready"
        );
    }
}

#[test]
fn websocket_probe_recognises_an_accepted_upgrade() {
    /*
     * 探测必须发真实升级头：网关的 WebSocket 面在握手回调之前就校验升级头，
     * 缺头时直接断连、不回任何响应，于是「端口对不对」会退化成「连不上」。
     * 这条 fixture 只回一个 101，用来固定「握手成功即 Ready」这一判定。
     */
    let base = spawn_status_server("HTTP/1.1 101 Switching Protocols", "");
    assert_eq!(
        probe_websocket(&base.replace("http://", "ws://"), None),
        WsProbeOutcome::Ready,
        "握手成功说明该端口就是 WebSocket 监听"
    );
}

#[test]
fn websocket_probe_tells_a_missing_route_from_a_credential_rejection() {
    let missing = spawn_status_server("HTTP/1.1 404 Not Found", "{}");
    assert_eq!(
        probe_websocket(&missing.replace("http://", "ws://"), None),
        WsProbeOutcome::WrongPort
    );
    let unauthorized = spawn_status_server("HTTP/1.1 401 Unauthorized", "{}");
    assert_eq!(
        probe_websocket(&unauthorized.replace("http://", "ws://"), Some("sk-wrong")),
        WsProbeOutcome::CredentialRejected { status: 401 }
    );
}

#[test]
fn websocket_probe_sends_the_upgrade_headers_instead_of_a_plain_get() {
    /*
     * 把服务端收到的请求头读回来：缺任何一项升级头，真实网关都会静默断连，
     * 这条断言因此是「探活能过」的前提，而不是实现细节。
     */
    let listener = TcpListener::bind("127.0.0.1:0").expect("回环端口应可绑定");
    let address = listener.local_addr().expect("应能取回真实端口");
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0_u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_ascii_lowercase());
            let response = "HTTP/1.1 101 Switching Protocols\r\nconnection: Upgrade\r\nupgrade: websocket\r\n\r\n";
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    let outcome = probe_websocket(&format!("ws://{address}"), None);
    let request = receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("服务端应收到探测请求");
    assert_eq!(outcome, WsProbeOutcome::Ready);
    for header in [
        "upgrade: websocket",
        "connection: upgrade",
        "sec-websocket-version: 13",
        "sec-websocket-key:",
    ] {
        assert!(
            request.contains(header),
            "探测请求缺少升级头 {header}，实际请求：\n{request}"
        );
    }
}
