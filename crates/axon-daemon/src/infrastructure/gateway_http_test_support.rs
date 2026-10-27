//! Gateway HTTP 与语音 WS 契约测试共用的替身与真实 socket 夹具。
//!
//! 只服务于 `#[cfg(test)]`：这里不出现生产逻辑，测试脚手架不得反向污染适配器。
//! 断言留在各测试文件里，本模块只提供「造服务、发请求、记日志」三件事。

use super::gateway_voice_worker::HostLauncher;
use super::*;
use crate::application::ExternalEventEnvelope;
use crate::infrastructure::gateway_auth::GatewayAuth;
use crate::infrastructure::gateway_kernel::{
    GatewayEgress, GatewayEvent, GatewayEventStream, GatewayIngress, GatewayKernel,
    GatewayKernelError, GatewaySubscriptionRequest,
};
use axon_domain::InteractionId;
use axon_protocol::SubmitInteractionReceipt;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Condvar, Mutex as StdMutex};

/// 测试用 Gateway token。
pub(super) const TEST_AUTH_TOKEN: &str = "gateway-test-token";
/// 测试用连接上限。
pub(super) const TEST_CONNECTION_LIMIT: usize = 256;
/// 单次真实请求的读取超时。
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 记录提交并可选择性阻塞的 ingress。
pub(super) struct RecordingIngress {
    submitted: StdMutex<Vec<ExternalEventEnvelope>>,
    entered: Option<std_mpsc::Sender<()>>,
    release: Option<Arc<(StdMutex<bool>, Condvar)>>,
}

/// 阻塞闸门：提交进入后停在这里，直到测试放行。
type SubmitGate = Arc<(StdMutex<bool>, Condvar)>;

/// 已进入阻塞态的 ingress 及其实验句柄。
pub(super) struct GatedIngress {
    pub(super) ingress: Arc<RecordingIngress>,
    /// 每有一条提交进入 handler 就收到一次通知。
    pub(super) entered: std_mpsc::Receiver<()>,
    gate: SubmitGate,
}

impl GatedIngress {
    /// 放行全部阻塞中的提交。
    pub(super) fn release(&self) {
        let (lock, condvar) = &*self.gate;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condvar.notify_all();
    }
}

impl RecordingIngress {
    pub(super) fn idle() -> Arc<Self> {
        Arc::new(Self {
            submitted: StdMutex::new(Vec::new()),
            entered: None,
            release: None,
        })
    }

    /// 每次提交都先阻塞，直到 `GatedIngress::release` 放行。
    pub(super) fn gated() -> GatedIngress {
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

    pub(super) fn submitted_count(&self) -> usize {
        self.submitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 已提交事件快照：语音用例据此核对车道键与渠道没有另起一套。
    pub(super) fn submitted(&self) -> Vec<ExternalEventEnvelope> {
        self.submitted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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

/// 由测试脚本推送事件的出口：订阅后即可收到脚本投递的帧。
pub(super) struct ScriptedEgress {
    senders: StdMutex<Vec<std_mpsc::Sender<GatewayEvent>>>,
    registered: AtomicUsize,
}

impl ScriptedEgress {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            senders: StdMutex::new(Vec::new()),
            registered: AtomicUsize::new(0),
        })
    }

    /// 向当前所有订阅者投递一帧；已断开的订阅自动清退。
    pub(super) fn push(&self, event: GatewayEvent) {
        let mut senders = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        senders.retain(|sender| sender.send(event.clone()).is_ok());
    }

    /// 已建立的订阅数；测试据此等待订阅线程就绪后再投递，避免竞态。
    pub(super) fn subscriber_count(&self) -> usize {
        self.registered.load(Ordering::Relaxed)
    }
}

impl GatewayEgress for ScriptedEgress {
    fn subscribe(
        &self,
        _request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        let (sender, receiver) = std_mpsc::channel();
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(sender);
        self.registered.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(ScriptedEventStream { receiver }))
    }
}

struct ScriptedEventStream {
    receiver: std_mpsc::Receiver<GatewayEvent>,
}

impl GatewayEventStream for ScriptedEventStream {
    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GatewayEvent>, GatewayKernelError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(std_mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std_mpsc::RecvTimeoutError::Disconnected) => {
                Err(GatewayKernelError::Ingress("测试事件流已关闭".to_owned()))
            }
        }
    }
}

/// 收集访问日志的 sink。
#[derive(Default)]
pub(super) struct RecordingSink {
    lines: StdMutex<Vec<(String, String)>>,
}

impl RecordingSink {
    pub(super) fn messages(&self) -> Vec<String> {
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
pub(super) struct TestServer {
    pub(super) adapter: HttpGatewayAdapter,
    pub(super) address: SocketAddr,
    pub(super) ingress: Arc<RecordingIngress>,
    pub(super) sink: Arc<RecordingSink>,
}

impl TestServer {
    pub(super) fn start(ingress: Arc<RecordingIngress>) -> Self {
        Self::start_with_timeouts(ingress, VoiceTimeouts::production())
    }

    /// 用注入的语音超时启动服务：断连用例不必真的等 30 s / 120 s。
    pub(super) fn start_with_timeouts(
        ingress: Arc<RecordingIngress>,
        timeouts: VoiceTimeouts,
    ) -> Self {
        Self::start_with(ingress, timeouts, Arc::new(RejectingEgress), None)
    }

    /// 用脚本出口与进程内脚本宿主启动服务：语音回合闭环用例走这里。
    pub(super) fn start_scripted(
        ingress: Arc<RecordingIngress>,
        timeouts: VoiceTimeouts,
        host: HostLauncher,
    ) -> (Self, Arc<ScriptedEgress>) {
        let egress = ScriptedEgress::new();
        let server = Self::start_with(
            ingress,
            timeouts,
            Arc::clone(&egress) as Arc<dyn GatewayEgress>,
            Some(host),
        );
        (server, egress)
    }

    fn start_with(
        ingress: Arc<RecordingIngress>,
        timeouts: VoiceTimeouts,
        egress: Arc<dyn GatewayEgress>,
        host: Option<HostLauncher>,
    ) -> Self {
        let kernel = GatewayKernel::new_with_auth(
            Arc::clone(&ingress) as Arc<dyn GatewayIngress>,
            egress,
            TEST_CONNECTION_LIMIT,
            Some(Arc::new(
                GatewayAuth::from_token(TEST_AUTH_TOKEN).expect("测试 token"),
            )),
        );
        kernel.start().expect("启动测试 Gateway Kernel");
        let context = kernel.context().expect("运行中的 Gateway 上下文");
        let sink = Arc::new(RecordingSink::default());
        let mut adapter = HttpGatewayAdapter::new("127.0.0.1:0", test_providers())
            .with_log_sink(Arc::clone(&sink) as Arc<dyn LogSink>)
            .with_voice_timeouts(timeouts);
        if let Some(host) = host {
            adapter = adapter.with_voice_host(host);
        }
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
pub(super) fn request(server: &TestServer, raw: &str) -> (u16, String) {
    send_raw(server.address, raw)
}

pub(super) fn send_raw(address: SocketAddr, raw: &str) -> (u16, String) {
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
pub(super) fn post(
    target: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: &str,
) -> String {
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
pub(super) fn get(target: &str, token: Option<&str>) -> String {
    let mut raw = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(token) = token {
        raw.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    raw.push_str("\r\n");
    raw
}
