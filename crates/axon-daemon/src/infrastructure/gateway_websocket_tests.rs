//! Gateway WebSocket 适配器契约测试：runtime 线程不得被 durable 桥调用或连接收口阻塞。
//!
//! 全部连接共享同一个 current_thread runtime，因此任何同步阻塞都会放大为
//! 「一条连接的动作停掉整个接入面」。以下用真实 socket 固定三条不可回退的契约：
//! 1. durable submit 阻塞期间，其他连接仍必须能握手并完成 ping/pong；
//! 2. 连接收口（含事件投影线程 join）不得让 runtime 线程失去服务能力；
//! 3. 一条连接并行服务多路订阅：各自游标单调、互不阻塞，`unsubscribe` 只停掉
//!    指定的那一路，断开连接释放该连接的全部订阅。

use super::*;
use crate::application::ExternalEventEnvelope;
use crate::infrastructure::gateway_auth::GatewayAuth;
use crate::infrastructure::gateway_kernel::{
    GatewayEgress, GatewayEvent, GatewayIngress, GatewayKernel, GatewayKernelError,
    GatewaySubscriptionRequest,
};
use axon_domain::InteractionId;
use axon_protocol::SubmitInteractionReceipt;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc as std_mpsc;
use std::sync::{Condvar, Mutex as StdMutex};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

/// 测试用 Gateway token。
const TEST_AUTH_TOKEN: &str = "gateway-test-token";
/// 测试用连接上限：满足两条并发连接即可。
const TEST_CONNECTION_LIMIT: usize = 8;
/// 等待 runtime 恢复服务能力的上限；正常实现应在毫秒级完成。
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);
/// 等待服务端关闭客户端连接的收口观察窗口。
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// 订阅流预置事件条数：足够证明投影线程走过「有事件」分支。
const STORY_EVENT_COUNT: usize = 2;
/// 每路订阅各投递多少条事件；两路交错时足够看出各自独立。
const MULTI_STREAM_EVENT_COUNT: usize = 2;
/// 多路订阅测试中等待一路收口的窗口。
const SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(3);

type TestSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// 进入即阻塞的 durable ingress：用于固定 submit 期间 runtime 的可用性。
struct BlockingIngress {
    entered: std_mpsc::Sender<()>,
    release: Arc<(StdMutex<bool>, Condvar)>,
}

impl GatewayIngress for BlockingIngress {
    fn submit_external_event(
        &self,
        _event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
        let _ = self.entered.send(());
        let (lock, condvar) = &*self.release;
        let mut released = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released {
            released = condvar
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        Ok(SubmitInteractionReceipt {
            interaction_id: InteractionId::new("interaction-1").expect("合法 Interaction ID"),
            accepted: true,
            deduplicated: false,
        })
    }
}

/// 不使用 durable ingress 的场景：提交一律失败，不参与断言。
struct RejectingIngress;

impl GatewayIngress for RejectingIngress {
    fn submit_external_event(
        &self,
        _event: ExternalEventEnvelope,
    ) -> Result<SubmitInteractionReceipt, GatewayKernelError> {
        Err(GatewayKernelError::Ingress(
            "测试 ingress 拒绝提交".to_owned(),
        ))
    }
}

/// 订阅一律失败：submit 阻塞场景不需要出站流。
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

/// 订阅成功但很快进入空闲的事件流。
///
/// 空闲期（`Ok(None)`）不会触发任何一次发送失败，因此投影线程只能在显式收口信号
/// 下退出：这正是收口路径必须自证的地方。
struct IdleEventStream {
    pending: VecDeque<GatewayEvent>,
}

impl GatewayEventStream for IdleEventStream {
    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GatewayEvent>, GatewayKernelError> {
        if let Some(event) = self.pending.pop_front() {
            return Ok(Some(event));
        }
        thread::sleep(timeout);
        Ok(None)
    }
}

/// 返回固定长度事件序列的 egress。
struct StoryEgress;

impl GatewayEgress for StoryEgress {
    fn subscribe(
        &self,
        _request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        Ok(Box::new(IdleEventStream {
            pending: (0..STORY_EVENT_COUNT)
                .map(|index| GatewayEvent {
                    event_type: "turn_delta".to_owned(),
                    payload_json: format!("{{\"index\":{index}}}"),
                    cursor: None,
                    replay: false,
                })
                .collect(),
        }))
    }
}

/// 两路订阅各自独立的事件序列：载荷与游标都按目标区分。
///
/// 游标刻意做成「每路各自从 0 单调递增」，这样交错读取时可以直接验证
/// 「每一路自己的游标是单调的」，而不是靠时序猜测。
struct TaggedEgress;

impl GatewayEgress for TaggedEgress {
    fn subscribe(
        &self,
        request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        let target = if request.session_id.is_some() {
            "session".to_owned()
        } else {
            request.interaction_id.clone()
        };
        Ok(Box::new(IdleEventStream {
            pending: (0..MULTI_STREAM_EVENT_COUNT)
                .map(|index| GatewayEvent {
                    event_type: "turn_delta".to_owned(),
                    payload_json: format!("{{\"target\":\"{target}\",\"index\":{index}}}"),
                    cursor: Some(format!("{{\"sequence\":{index}}}")),
                    replay: false,
                })
                .collect(),
        }))
    }
}

/// 统计订阅建立次数并在每个流上留一个存活标记：用于证明句柄释放时机。
struct CountingEgress {
    live: Arc<AtomicUsize>,
}

impl GatewayEgress for CountingEgress {
    fn subscribe(
        &self,
        _request: GatewaySubscriptionRequest,
    ) -> Result<Box<dyn GatewayEventStream>, GatewayKernelError> {
        self.live.fetch_add(1, Ordering::AcqRel);
        Ok(Box::new(MarkedStream {
            live: Arc::clone(&self.live),
            released: false,
        }))
    }
}

/// 句柄被丢弃（订阅真正释放）时递减存活计数。
struct MarkedStream {
    live: Arc<AtomicUsize>,
    released: bool,
}

impl GatewayEventStream for MarkedStream {
    fn recv_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<GatewayEvent>, GatewayKernelError> {
        thread::sleep(timeout);
        Ok(None)
    }
}

impl Drop for MarkedStream {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            self.live.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// 用测试 ingress/egress 启动一个真实 Gateway Kernel，并取得运行态上下文。
fn test_context(
    ingress: Arc<dyn GatewayIngress>,
    egress: Arc<dyn GatewayEgress>,
) -> GatewayContext {
    let kernel = GatewayKernel::new_with_auth(
        ingress,
        egress,
        TEST_CONNECTION_LIMIT,
        Some(Arc::new(
            GatewayAuth::from_token(TEST_AUTH_TOKEN).expect("测试 token"),
        )),
    );
    kernel.start().expect("启动测试 Gateway Kernel");
    kernel.context().expect("运行中的 Gateway 上下文")
}

/// 绑定本机端口并启动真实 WebSocket 接入循环；返回监听地址与服务端线程。
fn start_server(context: GatewayContext) -> (SocketAddr, Arc<AtomicBool>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定测试端口");
    listener
        .set_nonblocking(true)
        .expect("测试 listener 应为非阻塞");
    let address = listener.local_addr().expect("测试监听地址");
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let handle = thread::spawn(move || run_server(listener, server_stop, context));
    (address, stop, handle)
}

/// 携带凭据完成一次真实 WebSocket 建连。
async fn connect_socket(address: SocketAddr) -> TestSocket {
    let mut request = format!("ws://{address}{WS_PATH}")
        .into_client_request()
        .expect("构造握手请求");
    request.headers_mut().insert(
        HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {TEST_AUTH_TOKEN}")).expect("凭据 header"),
    );
    connect_async(request)
        .await
        .expect("WebSocket 建连应成功")
        .0
}

/// 在超时内发送一帧并取回响应文本。
async fn round_trip(address: SocketAddr, frame: String) -> Option<String> {
    tokio::time::timeout(RECOVERY_TIMEOUT, async {
        let mut socket = connect_socket(address).await;
        socket
            .send(Message::Text(frame.into()))
            .await
            .expect("发送客户端帧");
        let message = socket
            .next()
            .await
            .expect("应有响应帧")
            .expect("响应帧可读");
        message.to_text().unwrap_or_default().to_owned()
    })
    .await
    .ok()
}

fn submit_frame() -> String {
    serde_json::json!({
        "type": "submit",
        "sourceChannel": "http",
        "eventId": "event-1",
        "observedAtMs": 1_000,
        "expiresAtMs": 2_000,
        "content": "hello",
        "replyChannel": "http",
        "replyDestination": "client-1",
    })
    .to_string()
}

/// 车道订阅帧：标识按 `interaction:<id>` 约定生成。
fn subscribe_frame(interaction_id: &str) -> String {
    serde_json::json!({
        "type": "subscribe",
        "subscribeId": format!("interaction:{interaction_id}"),
        "workspaceId": "workspace-a",
        "interactionId": interaction_id,
    })
    .to_string()
}

/// 会话级订阅帧：不带 Interaction，走会话流。
fn session_subscribe_frame(subscribe_id: &str, session_id: &str) -> String {
    serde_json::json!({
        "type": "subscribe",
        "subscribeId": subscribe_id,
        "workspaceId": "workspace-a",
        "sessionId": session_id,
    })
    .to_string()
}

fn unsubscribe_frame(subscribe_id: &str) -> String {
    serde_json::json!({ "type": "unsubscribe", "subscribeId": subscribe_id }).to_string()
}

fn ping_frame() -> String {
    serde_json::json!({ "type": "ping" }).to_string()
}

fn client_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("客户端 runtime")
}

/// 在超时内读取下一帧文本；连接关闭或超时返回 None。
async fn next_frame(socket: &mut TestSocket) -> Option<String> {
    let Ok(Some(message)) = tokio::time::timeout(SUBSCRIPTION_TIMEOUT, socket.next()).await else {
        return None;
    };
    message
        .ok()
        .and_then(|message| message.to_text().ok().map(str::to_owned))
}

/// 读取帧直到看到指定订阅的确认；期间可能夹带订阅流的前几条事件。
async fn read_until_subscribed(socket: &mut TestSocket, subscribe_id: &str) -> bool {
    loop {
        let Some(text) = next_frame(socket).await else {
            return false;
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
            && value.get("type").and_then(serde_json::Value::as_str) == Some("subscribed")
        {
            return value.get("subscribeId").and_then(serde_json::Value::as_str)
                == Some(subscribe_id);
        }
    }
}

/// 两路订阅各自已收的游标，以及两路确认帧是否都已到达。
struct TwoSubscriptionTaps {
    first_confirmed: bool,
    second_confirmed: bool,
    first_cursors: Vec<String>,
    second_cursors: Vec<String>,
}

/// 把一帧流事件按订阅标识分流；不属于这两路的帧一律忽略。
fn route_stream_event(
    value: &serde_json::Value,
    first_id: &str,
    second_id: &str,
    first: &mut Vec<String>,
    second: &mut Vec<String>,
) {
    if value.get("type").and_then(serde_json::Value::as_str) != Some("event") {
        return;
    }
    let Some(id) = value.get("subscribeId").and_then(serde_json::Value::as_str) else {
        return;
    };
    let bucket = if id == first_id {
        first
    } else if id == second_id {
        second
    } else {
        return;
    };
    bucket.push(
        value
            .get("cursor")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    );
}

/// 读到两路订阅都收到确认，并把期间夹带的流事件按标识缓冲下来。
///
/// 两路确认帧谁先到由服务端调度决定，不能按「先发的那一路先确认」去读：那样
/// 一次调度抖动就会把第二路的确认读成第一路的。事件也可能先于确认帧到达，
/// 因此这里把它们收下而不是丢掉，交给后面的收齐循环继续用。
async fn read_until_two_subscribed(
    socket: &mut TestSocket,
    first_id: &str,
    second_id: &str,
) -> TwoSubscriptionTaps {
    let mut taps = TwoSubscriptionTaps {
        first_confirmed: false,
        second_confirmed: false,
        first_cursors: Vec::new(),
        second_cursors: Vec::new(),
    };
    while !(taps.first_confirmed && taps.second_confirmed) {
        let Some(text) = next_frame(socket).await else {
            break;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        match value.get("type").and_then(serde_json::Value::as_str) {
            Some("subscribed") => {
                match value.get("subscribeId").and_then(serde_json::Value::as_str) {
                    Some(id) if id == first_id => taps.first_confirmed = true,
                    Some(id) if id == second_id => taps.second_confirmed = true,
                    _ => {}
                }
            }
            Some("event") => route_stream_event(
                &value,
                first_id,
                second_id,
                &mut taps.first_cursors,
                &mut taps.second_cursors,
            ),
            _ => {}
        }
    }
    taps
}

/// 一次读取循环同时收两路事件，返回「第一路、第二路」各自的 cursor 序列。
///
/// 交错到达正是这个用例要固定的事实，因此不能按路分别读到收满为止：那会把
/// 「另一路的帧插在中间」误判成超时，机器一忙就假失败。这里按每路各自的目标条数
/// 收齐即止；确认阶段已经收到的事件由调用方传进来，不重复等待。
async fn collect_two_stream_cursors(
    socket: &mut TestSocket,
    first_id: &str,
    second_id: &str,
    mut first: Vec<String>,
    mut second: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    while first.len() < MULTI_STREAM_EVENT_COUNT || second.len() < MULTI_STREAM_EVENT_COUNT {
        let Some(text) = next_frame(socket).await else {
            break;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        route_stream_event(&value, first_id, second_id, &mut first, &mut second);
    }
    (first, second)
}

/// 解析一帧 JSON，非 JSON 或非对象返回 None。
fn parse_frame(text: &str) -> Option<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(text).ok()
}

#[path = "gateway_websocket_flow_tests.rs"]
mod gateway_websocket_flow_tests;
