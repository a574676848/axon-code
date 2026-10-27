//! Gateway HTTP 适配器契约测试：真实 socket 打真实服务线程。
//!
//! axum 承载传输后，以下三条是本模块必须自证的不可回退契约：
//! 1. 慢提交不得堵住其他请求：阻塞型业务在阻塞池执行，IO 线程始终可服务；
//! 2. 路由、方法、鉴权、报文校验的对外口径与既有实现逐分支一致；
//! 3. 访问日志逐请求落盘，状态码与耗时同源，偶发卡顿事后可归因。
//!
//! 语音 WS 的控制面契约在 `gateway_voice_ws_tests.rs`。

use super::test_support::{
    RecordingIngress, TEST_AUTH_TOKEN, TestServer, get, post, request, send_raw,
};
use super::*;

/// 慢提交并发数：必须超过旧实现的 4 个 worker 才能证明队头阻塞已消失。
const BLOCKED_SUBMIT_COUNT: usize = 8;
/// 阻塞期间探针请求的允许耗时上限。
const PROBE_BUDGET: Duration = Duration::from_secs(3);
/// 合法事件报文。
const EVENT_BODY: &str = r#"{"sourceChannel":"http","eventId":"event-1","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1"}"#;

#[test]
fn event_submission_round_trips_with_receipt() {
    let server = TestServer::start(RecordingIngress::idle());
    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/events",
            Some(TEST_AUTH_TOKEN),
            Some("application/json"),
            EVENT_BODY,
        ),
    );
    assert_eq!(status, 202, "合法事件应返回回执：{body}");
    assert!(
        body.contains("interaction-1"),
        "回执应带 Interaction：{body}"
    );
    assert_eq!(server.ingress.submitted_count(), 1);
}

#[test]
fn auth_content_type_and_body_limit_precede_parsing() {
    let server = TestServer::start(RecordingIngress::idle());

    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/events",
            None,
            Some("application/json"),
            EVENT_BODY,
        ),
    );
    assert_eq!(status, 401, "缺凭据必须先于报文校验被拒绝：{body}");
    assert!(body.contains("unauthorized"), "{body}");

    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/events",
            Some(TEST_AUTH_TOKEN),
            Some("text/plain"),
            EVENT_BODY,
        ),
    );
    assert_eq!(status, 415, "{body}");

    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/events",
            Some(TEST_AUTH_TOKEN),
            Some("application/json"),
            "",
        ),
    );
    assert_eq!(status, 411, "缺 Content-Length 必须被拒绝：{body}");

    // 只声明超限长度、不发正文：服务端必须在读体之前按 Content-Length 拒绝，
    // 因此这里不需要真的造出上限级别的字节。
    let declared_length = DEFAULT_MAX_BODY_BYTES + 1;
    let raw = format!(
        "POST /gateway/v1/events HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {TEST_AUTH_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {declared_length}\r\n\r\n"
    );
    let (status, _) = request(&server, &raw);
    assert_eq!(status, 413, "超限请求体必须在读体前被拒绝");
    assert_eq!(server.ingress.submitted_count(), 0);
}

#[test]
fn query_token_channel_still_authenticates() {
    let server = TestServer::start(RecordingIngress::idle());
    let target = format!("/gateway/v1/events?api_token={TEST_AUTH_TOKEN}");
    let (status, body) = request(
        &server,
        &post(&target, None, Some("application/json"), EVENT_BODY),
    );
    assert_eq!(status, 202, "URL 查询串凭据通道必须仍然可用：{body}");
}

#[test]
fn routes_and_methods_keep_the_contract() {
    let server = TestServer::start(RecordingIngress::idle());
    let auth = Some(TEST_AUTH_TOKEN);

    assert_eq!(request(&server, &get("/gateway/v1/events", auth)).0, 404);
    assert_eq!(
        request(
            &server,
            &format!(
                "PUT /gateway/v1/events HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {TEST_AUTH_TOKEN}\r\n\r\n"
            )
        )
        .0,
        405
    );
    // provider 名非法：与既有实现一致，先于注册表校验给 404。
    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/events/JIRA",
            auth,
            Some("application/json"),
            EVENT_BODY,
        ),
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("not_found"), "{body}");
    // 未注册 provider：给出可区分的 404。
    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/events/jira",
            auth,
            Some("application/json"),
            EVENT_BODY,
        ),
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("provider_not_configured"), "{body}");
    // 交互查询：GET 命中，POST 按既有口径给 404。
    let (status, body) = request(&server, &get("/gateway/v1/interactions/int-1", auth));
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("interaction_not_found"), "{body}");
    let (status, body) = request(
        &server,
        &post(
            "/gateway/v1/interactions/int-1",
            auth,
            Some("application/json"),
            "{}",
        ),
    );
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("not_found"), "{body}");
    // 未知路径与未知方法。
    let (status, body) = request(&server, &get("/nope", auth));
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("not_found"), "{body}");
    let (status, body) = request(
        &server,
        &format!(
            "PUT /nope HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {TEST_AUTH_TOKEN}\r\n\r\n"
        ),
    );
    assert_eq!(status, 405, "{body}");
    assert!(body.contains("method_not_allowed"), "{body}");
}

#[test]
fn provider_names_stay_validated_after_path_decoding() {
    let server = TestServer::start(RecordingIngress::idle());
    // 百分号编码路径段由 axum 解码后再交给域名校验：`%2e%2e` 解出 `..`、
    // `A%42` 解出大写名，都必须与直接写明文时同样被拒（409 之外的 404 not_found）。
    for provider in ["..", "%2e%2e", "A%42", "jira.raw"] {
        let target = format!("/gateway/v1/events/{provider}");
        let (status, body) = request(
            &server,
            &post(
                &target,
                Some(TEST_AUTH_TOKEN),
                Some("application/json"),
                EVENT_BODY,
            ),
        );
        assert_eq!(status, 404, "{provider} 必须被拒绝：{body}");
        assert!(body.contains("not_found"), "{provider}: {body}");
    }
    assert_eq!(server.ingress.submitted_count(), 0);
}

/// 请求行与请求头的总长上限对两类路由族同样生效。
#[test]
fn oversized_request_head_is_rejected_on_both_route_families() {
    let server = TestServer::start(RecordingIngress::idle());
    let filler = "x".repeat(MAX_HEADER_BYTES + 1);
    for target in ["/gateway/v1/events", "/api/v1/workspaces"] {
        let raw = format!(
            "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nAuthorization: Bearer {TEST_AUTH_TOKEN}\r\nX-Padding: {filler}\r\n\r\n"
        );
        let (status, body) = request(&server, &raw);
        assert_eq!(status, 431, "{target}: {body}");
        assert!(body.contains("headers_too_large"), "{target}: {body}");
    }
}

#[test]
fn custom_route_family_uses_the_unified_error_model() {
    let server = TestServer::start(RecordingIngress::idle());

    let (status, body) = request(&server, &get("/api/v1/anything", None));
    assert_eq!(status, 401, "{body}");
    assert!(
        body.contains("UNAUTHORIZED"),
        "自定义路由族用统一错误模型：{body}"
    );

    let (status, body) = request(&server, &get("/api/v1/anything", Some(TEST_AUTH_TOKEN)));
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("NOT_FOUND"), "{body}");

    // 未挂载处理器时，`/api/v1/` 前缀内仍按自定义路由族口径处理。
    let (status, body) = request(&server, &get("/api/v1/", Some(TEST_AUTH_TOKEN)));
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("NOT_FOUND"), "{body}");
}

/// 核心契约：慢提交不得让其他请求排队。
///
/// 旧实现把业务跑在 4 个固定 worker 上，超过 4 条在途慢请求就会让网关整体失去
/// 响应；改造后业务在阻塞池执行，IO 线程始终可服务。
#[test]
fn slow_submissions_do_not_block_other_requests() {
    let gated = RecordingIngress::gated();
    let server = TestServer::start(Arc::clone(&gated.ingress));
    let address = server.address;

    let mut handles = Vec::new();
    for _ in 0..BLOCKED_SUBMIT_COUNT {
        handles.push(std::thread::spawn(move || {
            send_raw(
                address,
                &post(
                    "/gateway/v1/events",
                    Some(TEST_AUTH_TOKEN),
                    Some("application/json"),
                    EVENT_BODY,
                ),
            )
        }));
    }
    for _ in 0..BLOCKED_SUBMIT_COUNT {
        gated
            .entered
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("全部慢提交都应进入 handler");
    }

    // 慢提交全部在途时，普通请求必须在预算内拿到响应。
    let started = std::time::Instant::now();
    let (status, body) = request(
        &server,
        &get("/gateway/v1/interactions/int-1", Some(TEST_AUTH_TOKEN)),
    );
    let elapsed = started.elapsed();
    assert_eq!(status, 404, "{body}");
    assert!(
        elapsed < PROBE_BUDGET,
        "慢提交在途时探针请求耗时 {elapsed:?}，超出 {PROBE_BUDGET:?}"
    );

    gated.release();
    for handle in handles {
        let (status, _) = handle.join().expect("慢提交线程应正常结束");
        assert_eq!(status, 202, "放行后慢提交必须正常返回回执");
    }
    assert_eq!(server.ingress.submitted_count(), BLOCKED_SUBMIT_COUNT);
}

#[test]
fn access_log_records_status_and_elapsed_per_request() {
    let server = TestServer::start(RecordingIngress::idle());
    let _ = request(
        &server,
        &post(
            "/gateway/v1/events",
            Some(TEST_AUTH_TOKEN),
            Some("application/json"),
            EVENT_BODY,
        ),
    );
    let _ = request(&server, &get("/nope", Some(TEST_AUTH_TOKEN)));

    let messages = server.sink.messages();
    assert_eq!(messages.len(), 2, "逐请求一条访问日志：{messages:?}");
    assert!(
        messages
            .iter()
            .any(|line| line.contains("path=/gateway/v1/events") && line.contains("status=202")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|line| line.contains("path=/nope") && line.contains("status=404")),
        "{messages:?}"
    );
    assert!(
        messages.iter().all(|line| line.contains("elapsed_ms=")),
        "访问日志必须带耗时：{messages:?}"
    );
}

#[test]
fn local_addr_reports_the_bound_port() {
    let server = TestServer::start(RecordingIngress::idle());
    assert!(server.address.port() > 0, "绑定 0 端口后应能取回真实端口");
    assert!(server.adapter.local_addr().is_some());
}
