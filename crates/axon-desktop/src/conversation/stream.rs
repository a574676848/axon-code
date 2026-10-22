//! 网关事件流订阅：一条连接并行服务多路订阅。
//!
//! 网关协议允许同一条连接并存多条订阅（每条一个 `subscribeId`），因此本进程只维护
//! **一个**订阅器实例：车道流与子代理会话流共用这一条连接、这一条 Tauri 事件通道，
//! 帧内自带 `subscribeId` 由前端分发。这样窗口在车道与子代理视图之间来回切换时
//! 不必重建连接，两条流也不会互相掐断。
//!
//! 每条订阅各持自己的 `task` 与游标：取消一路只 abort 它自己的任务。同一个目标
//! 重复订阅直接忽略：界面任何一次无关重渲染都可能重复请求同一目标，放任它重建
//! 会让正在进行的这一轮丢掉终态帧（分割线因此消失）。
//!
//! 服务端不主动发心跳，静默连接靠客户端周期性 `ping` 维持；断流由前端按
//! 「可重试」结论决定是否重连，重连时带上最后一次见到的游标即可续订。
//!
//! 事件只有一个生产者，消费者有两个：桌面窗口经 Tauri 事件通道接收，本地调试桥
//! 经进程内订阅队列转发给浏览器的 SSE 连接。两者拿到的是同一条订阅流上的同一批帧，
//! 因此浏览器验证与真机窗口看到的事实完全一致。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::conversation::wire::{ClientFrame, ServerFrame, StreamEvent, WS_PATH};
use crate::gateway;

/// 前端监听的事件名；车道流与会话流共用这一条通道，按帧内 `subscribeId` 分发。
pub const CONVERSATION_EVENT: &str = "axon://conversation";
/// 建连超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 保活间隔。
const PING_INTERVAL: Duration = Duration::from_secs(20);

use std::time::Duration;

/// 会话流订阅器。
///
/// 订阅连接跑在自有的单线程运行时上，不依赖桌面宿主的异步运行时，也不引用
/// 任何界面类型：这样链路可以独立测试，桌面窗口只是它的一个消费者。
#[derive(Default)]
pub struct ConversationStream {
    runtime: OnceLock<tokio::runtime::Runtime>,
    /// 按 `subscribeId` 索引的活跃订阅；每个条目持有自己那条连接任务。
    tasks: Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
    emitter: Mutex<Option<StreamEmitter>>,
    subscribers: Mutex<Vec<mpsc::Sender<StreamEvent>>>,
}

/// 事件发射器：桌面窗口把它接到 Tauri 事件通道上。
type StreamEmitter = Arc<dyn Fn(StreamEvent) + Send + Sync>;

/// 进程内唯一的订阅器。
///
/// 桌面窗口与本地调试桥共享同一个实例：车道流与子代理会话流跑在同一条连接的不同
/// 订阅上，若两处各自持有一份，就会出现两条连接争抢同一批事件。
pub fn shared() -> &'static Arc<ConversationStream> {
    static SHARED: OnceLock<Arc<ConversationStream>> = OnceLock::new();
    SHARED.get_or_init(|| Arc::new(ConversationStream::new()))
}

impl ConversationStream {
    /// 构造订阅器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 安装事件发射器，使事件经桌面窗口的事件通道下发。
    pub fn install_emitter(&self, emitter: StreamEmitter) {
        *self
            .emitter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(emitter);
    }

    /// 订阅事件流，供本地调试桥转发给浏览器。
    #[must_use]
    pub fn subscribe_events(&self) -> mpsc::Receiver<StreamEvent> {
        let (sender, receiver) = mpsc::channel();
        self.subscribers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(sender);
        receiver
    }

    /// 按目标建立一条订阅；同一目标已在订阅时直接忽略。
    ///
    /// `watch` 与 [`unwatch`](Self::unwatch) 都由桌面命令串行化调用（编排器是同一个
    /// 事件循环），因此这里「先占位、再换真实任务句柄」的两步不需要额外加锁语义。
    pub fn watch(&self, target: SubscribeTarget) {
        let subscribe_id = target.subscribe_id();
        // 任务一律落在订阅器自有的运行时上：命令线程上没有环境运行时，
        // 用 `tokio::task::spawn` 会直接 panic（桌面端一订阅就崩）。
        let runtime = match self.runtime() {
            Ok(runtime) => runtime,
            Err(message) => {
                self.publish_failure(&subscribe_id, &message, false);
                return;
            }
        };
        {
            let mut tasks = self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if tasks.contains_key(&subscribe_id) {
                return;
            }
            // 先占位：并发调用时第二次会在这里被挡下。真实的 abort 句柄在下面替换进来。
            tasks.insert(subscribe_id.clone(), runtime.spawn(async {}));
        }
        let publisher = shared();
        let tag = subscribe_id.clone();
        let handle = runtime.spawn(async move {
            if let Err(message) = run_subscription(publisher, target).await {
                publisher.publish_failure(&tag, &message, true);
            }
        });
        self.tasks()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(subscribe_id, handle);
    }

    /// 关闭指定目标的订阅；目标未被订阅时不做任何事。
    pub fn unwatch(&self, target: &SubscribeTarget) {
        self.forget(&target.subscribe_id());
    }

    /// 按订阅标识关闭一条订阅；标识不存在时是幂等的。
    pub fn unsubscribe(&self, subscribe_id: &str) {
        self.forget(subscribe_id);
    }

    /// 关闭全部订阅；进程退出或调试桥整体收口时使用。
    pub fn close(&self) {
        let handles = {
            let mut tasks = self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            tasks.drain().map(|(_, handle)| handle).collect::<Vec<_>>()
        };
        for handle in handles {
            handle.abort();
        }
    }

    /// 当前活跃订阅数；用于真机与集成验证对账。
    #[must_use]
    pub fn active_subscriptions(&self) -> usize {
        self.tasks()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 摘除并取消一条订阅。
    fn forget(&self, subscribe_id: &str) {
        let handle = self
            .tasks()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(subscribe_id);
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    /// 广播一条事件。
    fn publish(&self, event: StreamEvent) {
        let emitter = self
            .emitter
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(emitter) = emitter {
            emitter(event.clone());
        }
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    }

    fn publish_failure(&self, subscribe_id: &str, message: &str, retryable: bool) {
        self.publish(StreamEvent::Failure {
            subscribe_id: subscribe_id.to_owned(),
            message: message.to_string(),
            retryable,
        });
    }

    /// 订阅连接自有的运行时；首次使用时建立。
    fn runtime(&self) -> Result<&tokio::runtime::Runtime, String> {
        if let Some(runtime) = self.runtime.get() {
            return Ok(runtime);
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_io()
            .enable_time()
            .build()
            .map_err(|error| format!("建立会话事件流运行时失败：{error}"))?;
        let _ = self.runtime.set(runtime);
        self.runtime
            .get()
            .ok_or_else(|| "建立会话事件流运行时失败".to_string())
    }

    fn tasks(&self) -> &Mutex<HashMap<String, tokio::task::JoinHandle<()>>> {
        &self.tasks
    }

    fn subscribers(&self) -> &Mutex<Vec<mpsc::Sender<StreamEvent>>> {
        &self.subscribers
    }
}

/// 一条订阅的连接目标。
///
/// `subscribe_id` 直接由目标推导：同一个目标只有一条稳定标识，重复订阅才可能被
/// 幂等挡下；协议只要求它在单连接内唯一，不要求跨连接唯一。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeTarget {
    /// 按 Interaction 订阅：主会话车道用它。
    Interaction {
        /// 目标所属工作区。
        workspace_id: String,
        /// 目标交互标识。
        interaction_id: String,
        /// 断点续订游标；缺省表示全量重放。
        resume_cursor: Option<String>,
    },
    /// 按会话订阅：子代理会话用它（不属于任何 Interaction）。
    Session {
        /// 目标所属工作区。
        workspace_id: String,
        /// 目标会话标识。
        session_id: String,
        /// 断点续订游标；缺省表示全量重放。
        resume_cursor: Option<String>,
    },
}

impl SubscribeTarget {
    /// 单连接内唯一的订阅标识。
    #[must_use]
    pub fn subscribe_id(&self) -> String {
        match self {
            Self::Interaction { interaction_id, .. } => format!("interaction:{interaction_id}"),
            Self::Session { session_id, .. } => format!("session:{session_id}"),
        }
    }

    fn into_frame(self, subscribe_id: String) -> ClientFrame {
        match self {
            Self::Interaction {
                workspace_id,
                interaction_id,
                resume_cursor,
            } => ClientFrame::Subscribe {
                subscribe_id,
                workspace_id,
                interaction_id: Some(interaction_id),
                session_id: None,
                resume_cursor,
            },
            Self::Session {
                workspace_id,
                session_id,
                resume_cursor,
            } => ClientFrame::Subscribe {
                subscribe_id,
                workspace_id,
                interaction_id: None,
                session_id: Some(session_id),
                resume_cursor,
            },
        }
    }
}

/// 建立连接、发出订阅帧，并持续把服务端帧广播出去。
async fn run_subscription(
    stream: &ConversationStream,
    target: SubscribeTarget,
) -> Result<(), String> {
    let subscribe_id = target.subscribe_id();
    let request = build_request()?;
    // 下行帧不设客户端上限：帧由网关按持久事件生成，一条大事件本来就可能很大，
    // 客户端自设上限只会掐断自己的订阅（64 KiB 是网关对客户端上行帧的限制，
    // 不是下行契约）。库自带的兜底上限仍在，用于挡住失控的对端。
    let (mut socket, _) =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| "连接会话事件流超时".to_string())?
            .map_err(|error| describe_handshake_error(&error))?;

    let subscribe = serde_json::to_string(&target.into_frame(subscribe_id.clone()))
        .map_err(|error| format!("构造订阅帧失败：{error}"))?;
    socket
        .send(Message::Text(subscribe.into()))
        .await
        .map_err(|error| format!("发送订阅帧失败：{error}"))?;

    let ping = serde_json::to_string(&ClientFrame::Ping {})
        .map_err(|error| format!("构造保活帧失败：{error}"))?;
    let mut ticker = tokio::time::interval(PING_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !forward_frame(stream, &subscribe_id, &text) {
                            return Ok(());
                        }
                    }
                    // 协议级 Ping 由 tungstenite 自动回 Pong，无需在此处理。
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Binary(_))) | Some(Ok(Message::Close(_))) | None => {
                        stream.publish_failure(&subscribe_id, "会话事件流已断开", true);
                        return Ok(());
                    }
                    // 只有真实的连接级错误才算断流：一帧装不下属于帧级问题，
                    // 丢掉它继续读下一帧，不因为一条大事件把订阅判死。
                    Some(Err(error)) if is_oversized_frame(&error) => {}
                    Some(Err(error)) => return Err(format!("会话事件流读取出错：{error}")),
                }
            }
            _ = ticker.tick() => {
                socket
                    .send(Message::Text(ping.clone().into()))
                    .await
                    .map_err(|error| format!("会话事件流保活失败：{error}"))?;
            }
        }
    }
}

/// 解析一帧服务端消息并广播；返回 false 表示应当收口。
///
/// 服务端一律回显 `subscribeId`，因此帧落到哪一路订阅不靠时序猜测；订阅确认与
/// 失败同样带上标识，前端才能只把属于自己目标的结论算进状态机。
fn forward_frame(stream: &ConversationStream, subscribe_id: &str, text: &str) -> bool {
    match serde_json::from_str::<ServerFrame>(text) {
        Ok(ServerFrame::Subscribed { .. }) => {
            stream.publish(StreamEvent::Subscribed {
                subscribe_id: subscribe_id.to_owned(),
            });
            true
        }
        Ok(ServerFrame::Event {
            subscribe_id: frame_subscribe_id,
            event_type,
            payload,
            cursor,
            replay,
        }) => {
            // 标识缺失（理论不可达）时归到发起订阅的那一路，帧本身仍然要投出去。
            let owner = frame_subscribe_id.unwrap_or_else(|| subscribe_id.to_owned());
            // 内部管线仍以 JSON 文本流转：对象形态在这里序列化一次，
            // 换来的是网络上不再有转义层。
            stream.publish(StreamEvent::Frame {
                subscribe_id: owner,
                event_type,
                payload_json: payload.to_string(),
                cursor,
                replay,
            });
            true
        }
        Ok(ServerFrame::Pong | ServerFrame::Unsubscribed { .. } | ServerFrame::Other) => true,
        Ok(ServerFrame::Error {
            error,
            subscribe_id: frame_subscribe_id,
        }) => {
            // 订阅被拒说明目标或凭据不对，重试同样的请求不会成功。
            let retryable = error == "gateway_busy";
            let owner = frame_subscribe_id.unwrap_or_else(|| subscribe_id.to_owned());
            stream.publish_failure(&owner, &describe_stream_error(&error), retryable);
            false
        }
        Err(error) => {
            stream.publish_failure(
                subscribe_id,
                &format!("会话事件流帧无法解析：{error}"),
                true,
            );
            false
        }
    }
}

/// 组装带鉴权握手头的 WebSocket 请求。
///
/// 网关 WebSocket 只从 `Authorization` 握手头取凭据，不接受查询串令牌，
/// 因此这条连接必须由 Rust 侧建立，浏览器 WebSocket 无法完成鉴权。
fn build_request() -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let url = format!("{}{WS_PATH}", gateway::websocket_base_url()?);
    let mut request = url
        .into_client_request()
        .map_err(|error| format!("构造会话事件流地址失败：{error}"))?;
    if let Some(token) = gateway::api_token() {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| "网关凭据含非法字符".to_string())?;
        request.headers_mut().insert("authorization", value);
    }
    Ok(request)
}

/// 把握手期错误翻译成可读结论；401/404 属于配置与版本问题，重试无益。
fn describe_handshake_error(error: &tokio_tungstenite::tungstenite::Error) -> String {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        return match response.status().as_u16() {
            401 | 403 => "会话事件流拒绝访问：网关凭据缺失或不匹配".to_string(),
            404 => "会话事件流地址不存在，后台服务版本可能过旧".to_string(),
            status => format!("会话事件流握手失败：HTTP {status}"),
        };
    }
    format!("会话事件流不可达：{error}")
}

/// 单帧超限：这一帧读不进来，但连接本身是好的。
///
/// 下行帧把事件的 `payload_json` 再转义一层，大事件会明显膨胀；这类错误属于帧级
/// 问题而不是连接级问题，判成断流会让界面停在最后一帧上不再刷新。
fn is_oversized_frame(error: &tokio_tungstenite::tungstenite::Error) -> bool {
    matches!(
        error,
        tokio_tungstenite::tungstenite::Error::Capacity(
            tokio_tungstenite::tungstenite::error::CapacityError::MessageTooLong { .. }
        )
    )
}

/// 把服务端错误码翻译成可读结论。
fn describe_stream_error(code: &str) -> String {
    match code {
        "unauthorized" => "会话事件流拒绝访问：网关凭据缺失或不匹配".to_string(),
        "subscription_rejected" => "订阅被拒绝：目标会话不存在，或与所选工作区不匹配".to_string(),
        "subscribe_missing_id" => {
            "订阅被拒绝：订阅标识缺失，客户端与后台版本可能不一致".to_string()
        }
        "subscribe_duplicate_id" => "订阅被拒绝：该订阅标识已在此连接上使用".to_string(),
        "gateway_busy" => "网关繁忙，请稍后重试".to_string(),
        other => format!("会话事件流返回错误：{other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_error_codes_map_to_actionable_chinese() {
        assert!(describe_stream_error("subscription_rejected").contains("工作区"));
        assert!(describe_stream_error("subscribe_missing_id").contains("版本"));
        assert!(describe_stream_error("subscribe_duplicate_id").contains("标识"));
        assert!(describe_stream_error("gateway_busy").contains("重试"));
        assert!(describe_stream_error("unknown_code").contains("unknown_code"));
    }

    #[test]
    fn handshake_errors_classify_status_codes() {
        let unauthorized = tokio_tungstenite::tungstenite::Error::Http(Box::new(
            tokio_tungstenite::tungstenite::http::Response::builder()
                .status(401)
                .body(None)
                .expect("构造 401 响应"),
        ));
        assert!(describe_handshake_error(&unauthorized).contains("凭据"));

        let missing = tokio_tungstenite::tungstenite::Error::Http(Box::new(
            tokio_tungstenite::tungstenite::http::Response::builder()
                .status(404)
                .body(None)
                .expect("构造 404 响应"),
        ));
        assert!(describe_handshake_error(&missing).contains("版本"));
    }

    #[test]
    fn subscribe_ids_are_derived_from_the_target_and_stay_distinct() {
        let lane = SubscribeTarget::Interaction {
            workspace_id: "workspace-a".to_owned(),
            interaction_id: "interaction-1".to_owned(),
            resume_cursor: None,
        };
        let session = SubscribeTarget::Session {
            workspace_id: "workspace-a".to_owned(),
            session_id: "parent--subagent--root".to_owned(),
            resume_cursor: None,
        };
        assert_eq!(lane.subscribe_id(), "interaction:interaction-1");
        assert_eq!(session.subscribe_id(), "session:parent--subagent--root");
        assert_ne!(
            lane.subscribe_id(),
            session.subscribe_id(),
            "车道流与会话流必须落在不同的订阅标识上，否则一条连接上会互相顶掉"
        );
    }

    #[test]
    fn session_subscribe_frame_carries_the_id_and_no_interaction() {
        let frame = SubscribeTarget::Session {
            workspace_id: "workspace-a".to_owned(),
            session_id: "parent--subagent--root".to_owned(),
            resume_cursor: Some("{\"generation\":\"g1\",\"sequence\":4}".to_owned()),
        }
        .into_frame("session:parent--subagent--root".to_owned());
        let value = serde_json::to_value(frame).expect("订阅帧应可序列化");
        assert_eq!(value["type"], "subscribe");
        assert_eq!(value["subscribeId"], "session:parent--subagent--root");
        assert_eq!(value["sessionId"], "parent--subagent--root");
        assert!(
            value.get("interactionId").is_none(),
            "会话级订阅不得带 Interaction 占位"
        );
        assert_eq!(
            value["resumeCursor"],
            "{\"generation\":\"g1\",\"sequence\":4}"
        );
    }

    #[test]
    fn published_events_reach_every_live_subscriber_and_drop_dead_ones() {
        let stream = ConversationStream::new();
        let first = stream.subscribe_events();
        let second = stream.subscribe_events();
        drop(second);
        stream.publish(StreamEvent::Subscribed {
            subscribe_id: "interaction:interaction-1".to_owned(),
        });
        assert_eq!(
            first.recv().expect("首个订阅者应收到事件"),
            StreamEvent::Subscribed {
                subscribe_id: "interaction:interaction-1".to_owned()
            }
        );
        assert_eq!(
            stream.subscribers().lock().expect("订阅表").len(),
            1,
            "已关闭的订阅者必须被剔除，避免无界堆积"
        );
    }

    #[test]
    fn frames_are_routed_by_the_echoed_subscribe_id() {
        let stream = ConversationStream::new();
        let receiver = stream.subscribe_events();
        assert!(forward_frame(
            &stream,
            "interaction:interaction-1",
            r#"{"type":"event","subscribeId":"session:parent--subagent--root","event_type":"wire_event","payload":{},"cursor":"c1"}"#,
        ));
        match receiver.recv().expect("应收到帧") {
            StreamEvent::Frame { subscribe_id, .. } => assert_eq!(
                subscribe_id, "session:parent--subagent--root",
                "同一条连接上的帧必须按服务端回显的标识归位，而不是按发起连接的订阅"
            ),
            other => panic!("应为事件帧，实际 {other:?}"),
        }
    }

    #[test]
    fn replay_marker_is_preserved_when_forwarding_frames() {
        let stream = ConversationStream::new();
        let receiver = stream.subscribe_events();
        for replay in [true, false] {
            let text = serde_json::json!({
                "type": "event", "subscribeId": "interaction:a",
                "event_type": "turn_completed", "payload": {}, "replay": replay
            })
            .to_string();
            assert!(forward_frame(&stream, "interaction:a", &text));
            let StreamEvent::Frame {
                replay: forwarded, ..
            } = receiver.recv().expect("应收到帧")
            else {
                panic!("应为事件帧");
            };
            assert_eq!(forwarded, replay, "转发不得丢失历史与实时的区别");
        }
    }

    #[test]
    fn unknown_subscription_is_not_tracked_twice() {
        let stream = ConversationStream::new();
        let target = SubscribeTarget::Interaction {
            workspace_id: "workspace-a".to_owned(),
            interaction_id: "interaction-1".to_owned(),
            resume_cursor: None,
        };
        assert_eq!(
            stream.active_subscriptions(),
            0,
            "初始状态下不应有任何活跃订阅"
        );
        stream.unwatch(&target);
        stream.unsubscribe(&target.subscribe_id());
        assert_eq!(
            stream.active_subscriptions(),
            0,
            "取消一个不存在的订阅必须是幂等的，不得报错或留下残余条目"
        );
    }

    #[test]
    fn watch_and_unwatch_track_one_task_per_target_without_an_ambient_runtime() {
        // 这里是普通测试线程，没有环境运行时。`watch` 必须自己把任务落到订阅器
        // 自有的运行时上：早先版本用 `tokio::task::spawn` 占位，一订阅就 panic。
        //
        // 目标故意指向不可达的网关：订阅任务会快速失败并把结论作为失败帧发出来，
        // 因此断言只看「登记与摘除」这一层，不等网络结果。
        let stream = ConversationStream::new();
        let lane = SubscribeTarget::Interaction {
            workspace_id: "workspace-a".to_owned(),
            interaction_id: "interaction-1".to_owned(),
            resume_cursor: None,
        };
        let session = SubscribeTarget::Session {
            workspace_id: "workspace-a".to_owned(),
            session_id: "parent--subagent--root".to_owned(),
            resume_cursor: None,
        };

        stream.watch(lane.clone());
        assert_eq!(stream.active_subscriptions(), 1, "第一条订阅必须登记在册");
        stream.watch(lane.clone());
        assert_eq!(
            stream.active_subscriptions(),
            1,
            "同一目标重复订阅必须被幂等挡下，不得堆出第二条任务"
        );

        stream.watch(session.clone());
        assert_eq!(
            stream.active_subscriptions(),
            2,
            "另一个目标必须能并存：单连接多路订阅正是这个模型"
        );

        stream.unwatch(&lane);
        assert_eq!(
            stream.active_subscriptions(),
            1,
            "取消一路不得连带摘除另一路"
        );
        stream.unsubscribe(&session.subscribe_id());
        assert_eq!(stream.active_subscriptions(), 0, "两路都取消后必须清空");
    }
}
