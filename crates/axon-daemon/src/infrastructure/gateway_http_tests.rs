//! Gateway HTTP 适配器契约测试：真实 socket 打真实服务线程。
//!
//! axum 承载传输后，以下三条是本模块必须自证的不可回退契约：
//! 1. 慢提交不得堵住其他请求：阻塞型业务在阻塞池执行，IO 线程始终可服务；
//! 2. 路由、方法、鉴权、报文校验的对外口径与既有实现逐分支一致；
//! 3. 访问日志逐请求落盘，状态码与耗时同源，偶发卡顿事后可归因。

use super::*;
use crate::application::ExternalEventEnvelope;
use crate::infrastructure::gateway_auth::GatewayAuth;
use crate::infrastructure::gateway_kernel::{
    GatewayEgress, GatewayEventStream, GatewayIngress, GatewayKernel, GatewayKernelError,
    GatewaySubscriptionRequest,
};
use axon_domain::InteractionId;
use axon_protocol::SubmitInteractionReceipt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc as std_mpsc;
use std::sync::{Condvar, Mutex as StdMutex};

/// 测试用 Gateway token。
const TEST_AUTH_TOKEN: &str = "gateway-test-token";
/// 测试用连接上限。
const TEST_CONNECTION_LIMIT: usize = 256;
/// 单次真实请求的读取超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 慢提交并发数：必须超过旧实现的 4 个 worker 才能证明队头阻塞已消失。
const BLOCKED_SUBMIT_COUNT: usize = 8;
/// 阻塞期间探针请求的允许耗时上限。
const PROBE_BUDGET: Duration = Duration::from_secs(3);
/// 合法事件报文。
const EVENT_BODY: &str = r#"{"sourceChannel":"http","eventId":"event-1","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1"}"#;

/// 记录提交并可选择性阻塞的 ingress。
struct RecordingIngress {
    submitted: StdMutex<Vec<ExternalEventEnvelope>>,
    entered: Option<std_mpsc::Sender<()>>,
    release: Option<Arc<(StdMutex<bool>, Condvar)>>,
}

/// 阻塞闸门：提交进入后停在这里，直到测试放行。
type SubmitGate = Arc<(StdMutex<bool>, Condvar)>;

/// 已进入阻塞态的 ingress 及其实验句柄。
struct GatedIngress {
    ingress: Arc<RecordingIngress>,
    /// 每有一条提交进入 handler 就收到一次通知。
    entered: std_mpsc::Receiver<()>,
    gate: SubmitGate,
}

impl GatedIngress {
    /// 放行全部阻塞中的提交。
    fn release(&self) {
        let (lock, condvar) = &*self.gate;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condvar.notify_all();
    }
}

impl RecordingIngress {
    fn idle() -> Arc<Self> {
        Arc::new(Self {
            submitted: StdMutex::new(Vec::new()),
            entered: None,
            release: None,
        })
    }

    /// 每次提交都先阻塞，直到 `GatedIngress::release` 放行。
    fn gated() -> GatedIngress {
        let (sender, receiver) = std_mpsc::channel();
        let gate: SubmitGate = Arc::new((StdMutex::new(false), Condvar::new()));
        let ingress = Arc::new(Self {
            submitted: StdMutex::new(Vec::new()),
            entered: Some(sender),
            release: Some(Arc::clone(&gate)),
        });
        GatedIngress {
            ingress,
            entered: receiver,
            gate,
        }
    }

    fn submitted_count(&self) -> usize {
        self.submitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl GatewayIngress for RecordingIngress {
    fn submit_external_event(
        &self,
        event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
        if let Some(entered) = self.entered.as_ref() {
            let _ = entered.send(());
        }
        if let Some(gate) = self.release.as_ref() {
            let (lock, condvar) = &**gate;
            let mut released = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*released {
                released = condvar
                    .wait(released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        self.submitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
        Ok(SubmitInteractionReceipt {
            interaction_id: InteractionId::new("interaction-1").expect("合法 Interaction ID"),
            accepted: true,
            deduplicated: false,
        })
    }
}

/// 订阅一律失败：HTTP 面不需要出站流。
struct RejectingEgress;

impl GatewayEgress for RejectingEgress {
    fn subscribe(
        &self,
        _request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        Err(GatewayKernelError::Ingress(
            "测试 egress 拒绝订阅".to_owned(),
        ))
    }
}

/// 收集访问日志的 sink。
#[derive(Default)]
struct RecordingSink {
    lines: StdMutex<Vec<(String, String)>>,
}

impl RecordingSink {
    fn messages(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(_, message)| message.clone())
            .collect()
    }
}

impl LogSink for RecordingSink {
    fn append(&self, level: &str, message: &str) {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((level.to_owned(), message.to_owned()));
    }
}

/// 已启动的测试服务；析构即收口。
struct TestServer {
    adapter: HttpGatewayAdapter,
    address: SocketAddr,
    ingress: Arc<RecordingIngress>,
    sink: Arc<RecordingSink>,
}

impl TestServer {
    fn start(ingress: Arc<RecordingIngress>) -> Self {
        let kernel = GatewayKernel::new_with_auth(
            Arc::clone(&ingress) as Arc<dyn GatewayIngress>,
            Arc::new(RejectingEgress),
            TEST_CONNECTION_LIMIT,
            Some(Arc::new(
                GatewayAuth::from_token(TEST_AUTH_TOKEN).expect("测试 token"),
            )),
        );
        kernel.start().expect("启动测试 Gateway Kernel");
        let context = kernel.context().expect("运行中的 Gateway 上下文");
        let sink = Arc::new(RecordingSink::default());
        let adapter = HttpGatewayAdapter::new("127.0.0.1:0", test_providers())
            .with_log_sink(Arc::clone(&sink) as Arc<dyn LogSink>);
        adapter.start(context).expect("启动 HTTP 适配器");
        let address = adapter.local_addr().expect("已绑定监听地址");
        Self {
            adapter,
            address,
            ingress,
            sink,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.adapter.stop();
    }
}

/// 测试用 provider 注册表：指向一个不存在的根目录，等价于「没有任何 provider 注册」。
fn test_providers() -> Arc<EventProviderRegistry> {
    Arc::new(EventProviderRegistry::new(
        std::env::temp_dir().join("axon-gateway-http-tests-no-providers"),
    ))
}

/// 发送一条原始请求并取回状态码与正文。
fn request(server: &TestServer, raw: &str) -> (u16, String) {
    send_raw(server.address, raw)
}

fn send_raw(address: SocketAddr, raw: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).expect("连接测试服务");
    stream
        .set_read_timeout(Some(REQUEST_TIMEOUT))
        .expect("设置读取超时");
    stream.write_all(raw.as_bytes()).expect("写入请求");
    let mut buffer = Vec::new();
    let _ = stream.read_to_end(&mut buffer);
    let text = String::from_utf8_lossy(&buffer).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map_or(String::new(), |(_, body)| body.to_owned());
    (status, body)
}

/// 组一条带凭据的请求；`body` 为空即无请求体。
fn post(target: &str, token: Option<&str>, content_type: Option<&str>, body: &str) -> String {
    let mut raw = format!("POST {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(token) = token {
        raw.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(content_type) = content_type {
        raw.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if !body.is_empty() {
        raw.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    raw.push_str("\r\n");
    raw.push_str(body);
    raw
}

/// 组一条带凭据的 GET。
fn get(target: &str, token: Option<&str>) -> String {
    let mut raw = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(token) = token {
        raw.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    raw.push_str("\r\n");
    raw
}

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
            .recv_timeout(REQUEST_TIMEOUT)
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
    #[test]
    #[ignore = "需要 AXON_VOICE_IDLE_TIMEOUT_SECS=1 以缩短等待"]
    fn voice_ws_idle_sends_session_idle() {
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
            let _ready = next_ws_text(&mut socket).await;
            let text = next_ws_text(&mut socket).await;
            println!("idle frame: {text}");
            let value: serde_json::Value = serde_json::from_str(&text).expect("错误帧是 JSON");
            assert_eq!(value["code"], "session_idle");
        });
    }
    panic!("文本帧数量超预期");
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
            assert!(
                std::time::Instant::now() < deadline,
                "等待 final 超时"
            );
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
