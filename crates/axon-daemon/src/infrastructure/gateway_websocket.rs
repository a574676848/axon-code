//! Gateway 通用 WebSocket 传输适配器。
//!
//! 全部连接共享同一个 current_thread runtime，因此接入线程只允许执行异步 IO：
//! durable 桥调用（提交、订阅）与连接收口都必须离开 runtime 线程，否则单条连接的
//! 一次同步阻塞或同步等待就会停掉整条接入面。
//!
//! **一条连接可以并存多条订阅**（会话流与车道流各占一路），每条订阅由自己的
//! `subscribeId` 标识，事件帧、订阅确认与订阅级错误一律回显该标识；每条订阅
//! 独立持有游标与投影线程，任一放慢或断开都不影响同一连接上的其它订阅。

use super::gateway_kernel::{GatewayAdapter, GatewayContext, GatewayEvent, GatewayEventStream};
use crate::application::ExternalEventEnvelope;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_hdr_async_with_config, tungstenite};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::Response as HttpResponse;
use tungstenite::protocol::{Message, WebSocketConfig};

const ADAPTER_ID: &str = "gateway-websocket";
const WS_PATH: &str = "/gateway/v1/ws";
/// 上行帧上限：只用来挡住失控的对端，不用来限制正常业务帧。
///
/// 64 KiB 会误伤正常用法（一条稍长的提交正文就能顶到上限），因此取 WebSocket 的
/// 常规档位：单帧 16 MiB、整条消息 64 MiB。超限仍回 `message_too_large` 并保持连接。
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// 事件队列容量。
const EVENT_QUEUE_CAPACITY: usize = 64;
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 兜底帧：一条常量，不会挤掉正常事件帧。
const EVENT_PROJECTION_FAILED_FRAME: &str = r#"{"type":"error","error":"event_projection_failed"}"#;

/// 通用 WebSocket 适配器。
pub struct WebSocketGatewayAdapter {
    bind_addr: String,
    stop: Arc<AtomicBool>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl WebSocketGatewayAdapter {
    /// 创建 WebSocket 适配器。
    #[must_use]
    pub fn new(bind_addr: impl Into<String>) -> Self {
        Self {
            bind_addr: bind_addr.into(),
            stop: Arc::new(AtomicBool::new(false)),
            handle: Mutex::new(None),
        }
    }
}

impl GatewayAdapter for WebSocketGatewayAdapter {
    fn adapter_id(&self) -> &str {
        ADAPTER_ID
    }

    fn start(&self, context: GatewayContext) -> Result<(), String> {
        let listener = TcpListener::bind(&self.bind_addr)
            .map_err(|error| format!("Gateway WebSocket 监听 {} 失败：{error}", self.bind_addr))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Gateway WebSocket 设置非阻塞 listener 失败：{error}"))?;
        let stop = Arc::clone(&self.stop);
        stop.store(false, Ordering::Release);
        let handle = thread::spawn(move || run_server(listener, stop, context));
        *self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
        Ok(())
    }

    fn drain(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = handle.join();
        }
    }
}

fn run_server(listener: TcpListener, stop: Arc<AtomicBool>, context: GatewayContext) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return,
    };
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(_) => return,
        };
        while !stop.load(Ordering::Acquire) {
            let accepted = tokio::time::timeout(ACCEPT_POLL_INTERVAL, listener.accept()).await;
            let Ok(Ok((stream, peer))) = accepted else {
                continue;
            };
            let connection_context = context.clone();
            let connection_stop = Arc::clone(&stop);
            tokio::spawn(async move {
                if let Err(error) =
                    handle_connection(stream, connection_context, connection_stop).await
                {
                    // 客户端只看到一句「连接被重置」：服务端必须留下对端地址与原因，
                    // 否则分不清是进程退出、写不动还是链路被拆。安装态的 stderr 不进任何
                    // 文件（Windows 计划任务把子进程输出交给 `Out-Host`），因此走 logger，
                    // 日志落在 data/logs 的当日文件里。
                    axon_logger::error(&format!(
                        "[gateway-ws] 连接级失败 peer={peer} 原因={error}"
                    ));
                }
            });
        }
    });
}

async fn handle_connection(
    stream: TcpStream,
    context: GatewayContext,
    stop: Arc<AtomicBool>,
) -> Result<(), tungstenite::Error> {
    let handshake_context = context.clone();
    let callback =
        move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            if request.uri().path() != WS_PATH {
                return Err(error_response(404, "not_found"));
            }
            let authorization = request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok());
            if !handshake_context.authorize_header(authorization) {
                return Err(error_response(401, "unauthorized"));
            }
            Ok(response)
        };
    let config = WebSocketConfig::default()
        .read_buffer_size(8 * 1024)
        .write_buffer_size(8 * 1024)
        .max_write_buffer_size(MAX_FRAME_BYTES)
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let mut socket = accept_hdr_async_with_config(stream, callback, Some(config)).await?;
    let permit = match context.try_acquire_connection() {
        Ok(permit) => permit,
        Err(_) => {
            socket
                .send(Message::Text(error_json("gateway_busy").into()))
                .await?;
            let _ = socket.close(None).await;
            return Ok(());
        }
    };
    // 连接级事件队列：每条订阅的投影线程按 `subscribeId` 打标签后写入，socket 循环统一取走。
    let (event_sender, mut event_receiver) = mpsc::channel::<TaggedEvent>(EVENT_QUEUE_CAPACITY);
    let mut subscriptions: Subscriptions = HashMap::new();
    loop {
        if stop.load(Ordering::Acquire) {
            let _ = socket.close(None).await;
            break;
        }
        tokio::select! {
            incoming = socket.next() => {
                let Some(incoming) = incoming else { break; };
                match incoming? {
                    Message::Text(text) => {
                        if text.len() > MAX_FRAME_BYTES {
                            socket.send(Message::Text(error_json("message_too_large").into())).await?;
                            continue;
                        }
                        if !handle_request(&mut socket, &context, &event_sender, &mut subscriptions, &text).await? {
                            break;
                        }
                    }
                    Message::Binary(_) => {
                        socket.send(Message::Text(error_json("text_message_required").into())).await?;
                    }
                    Message::Ping(payload) => socket.send(Message::Pong(payload)).await?,
                    Message::Pong(_) => {}
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            event = event_receiver.recv() => {
                let Some(event) = event else { break; };
                socket.send(Message::Text(event_json(&event).into())).await?;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    // 断开连接即按连接整体释放：丢弃订阅表会让每条订阅的生命周期哨兵一并释放，
    // 各自的投影线程随之收口。
    let threads = subscriptions
        .drain()
        .filter_map(|(_, mut entry)| entry.take_thread())
        .collect::<Vec<_>>();
    drop(event_sender);
    // 先关闭接收端，投影线程才能观察到收口信号；顺序颠倒会让 `join` 永久等待。
    drop(event_receiver);
    drop(permit);
    if !threads.is_empty() {
        // 投影线程最多再等一次 100ms 轮询；放进阻塞池，避免 runtime 线程被同步 join 拖住。
        let _ = tokio::task::spawn_blocking(move || {
            for thread in threads {
                let _ = thread.join();
            }
        })
        .await;
    }
    Ok(())
}

async fn handle_request(
    socket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    context: &GatewayContext,
    event_sender: &mpsc::Sender<TaggedEvent>,
    subscriptions: &mut Subscriptions,
    text: &str,
) -> Result<bool, tungstenite::Error> {
    let request = match serde_json::from_str::<WsClientRequest>(text) {
        Ok(request) => request,
        Err(_) => {
            socket
                .send(Message::Text(error_json("invalid_json").into()))
                .await?;
            return Ok(true);
        }
    };
    match request {
        WsClientRequest::Submit(event) => {
            // durable append 是同步 sqlite 写入（含 FULL 同步刷盘），必须离开 runtime 线程。
            let submit_context = context.clone();
            let result = tokio::task::spawn_blocking(move || {
                submit_context.submit_external_event(event.into_event())
            })
            .await;
            let response = match result {
                Ok(Ok(receipt)) => serde_json::json!({ "type": "accepted", "receipt": receipt }),
                // 阻塞池任务 panic 与 ingress 拒绝同属「未 durable 入账」，对外回执一致。
                Ok(Err(_)) | Err(_) => {
                    serde_json::json!({ "type": "error", "error": "event_rejected" })
                }
            };
            socket
                .send(Message::Text(response.to_string().into()))
                .await?;
        }
        WsClientRequest::Subscribe {
            subscribe_id,
            workspace_id,
            interaction_id,
            session_id,
            resume_cursor,
        } => {
            let subscribe_id = subscribe_id.unwrap_or_default().trim().to_owned();
            if subscribe_id.is_empty() {
                socket
                    .send(Message::Text(error_json("subscribe_missing_id").into()))
                    .await?;
                return Ok(true);
            }
            // 同一连接内标识重复即拒绝，但**不关闭连接**：其它订阅仍在服务。
            // 错误帧照例回显标识，客户端才知道被拒的是哪一路。
            if subscriptions.contains_key(&subscribe_id) {
                socket
                    .send(Message::Text(
                        subscription_error_json("subscribe_duplicate_id", Some(&subscribe_id))
                            .into(),
                    ))
                    .await?;
                return Ok(true);
            }
            // 订阅要读 durable 尾游标并分页重放，同样是同步 IO，必须离开 runtime 线程。
            let subscription = super::gateway_kernel::GatewaySubscriptionRequest {
                workspace_id,
                interaction_id: interaction_id.unwrap_or_default(),
                session_id,
                resume_cursor,
            };
            let subscribe_context = context.clone();
            let stream = match tokio::task::spawn_blocking(move || {
                subscribe_context.subscribe(subscription)
            })
            .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => {
                    // 只回可读错误码，但后端要留下可诊断的原因：被拒的三种口径
                    // （目标不存在 / 工作区不匹配 / 不是子代理会话）修复动作完全不同。
                    axon_logger::warning(&format!(
                        "[gateway-ws] 订阅被拒 id={subscribe_id} 原因={error:?}"
                    ));
                    socket
                        .send(Message::Text(
                            subscription_error_json("subscription_rejected", Some(&subscribe_id))
                                .into(),
                        ))
                        .await?;
                    return Ok(true);
                }
                Err(_) => {
                    socket
                        .send(Message::Text(
                            subscription_error_json("subscription_rejected", Some(&subscribe_id))
                                .into(),
                        ))
                        .await?;
                    return Ok(true);
                }
            };
            // 线程持一份发送端用于投递；订阅表持收口标志，条目摘除即要求线程退出。
            let thread_sender = event_sender.clone();
            let shutdown = Arc::new(AtomicBool::new(false));
            let thread_shutdown = Arc::clone(&shutdown);
            let tag = subscribe_id.clone();
            let thread =
                thread::spawn(move || forward_events(&tag, stream, thread_sender, thread_shutdown));
            subscriptions.insert(
                subscribe_id.clone(),
                SubscriptionEntry {
                    thread: Some(thread),
                    shutdown,
                },
            );
            socket
                .send(Message::Text(
                    serde_json::json!({ "type": "subscribed", "subscribeId": subscribe_id })
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        WsClientRequest::Unsubscribe { subscribe_id } => {
            // 丢弃条目即置位该订阅的收口标志，投影线程随之退出；其余订阅不受影响。
            // 针对不存在的标识取消订阅是幂等的：不报错、不影响其它订阅。
            subscriptions.remove(subscribe_id.trim());
            socket
                .send(Message::Text(
                    serde_json::json!({ "type": "unsubscribed", "subscribeId": subscribe_id })
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        WsClientRequest::Ping {} => {
            socket
                .send(Message::Text(
                    serde_json::json!({ "type": "pong" }).to_string().into(),
                ))
                .await?;
        }
    }
    Ok(true)
}

/// 一条订阅在连接内的资源。
///
/// 投影线程空闲时阻塞在自己的事件流上，而**丢弃 `JoinHandle` 并不会停掉线程**；
/// 又因为 tokio 有界通道的 `Sender::is_closed()` 只反映「接收端已丢弃」，
/// 拿它当收口信号是错的（接收端一直握在 socket 循环里）。因此这里给每条订阅
/// 一个显式的收口标志：条目一被摘除就置位，线程在下一轮询里自行退出并释放事件流。
struct SubscriptionEntry {
    /// 投影线程句柄；交出一次后为空。
    thread: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl SubscriptionEntry {
    /// 交出投影线程句柄；调用方负责 join。
    fn take_thread(&mut self) -> Option<JoinHandle<()>> {
        self.thread.take()
    }
}

impl Drop for SubscriptionEntry {
    fn drop(&mut self) {
        // 只置位不 join：join 会让 socket 循环空等一轮 100ms 级轮询，
        // 收口统一交给连接收尾处理。
        self.shutdown.store(true, Ordering::Release);
    }
}

/// 一条连接上并存的全部订阅，按 `subscribeId` 索引。
type Subscriptions = HashMap<String, SubscriptionEntry>;

fn forward_events(
    subscribe_id: &str,
    mut stream: Box<dyn GatewayEventStream>,
    sender: mpsc::Sender<TaggedEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let subscribe_id = subscribe_id.to_owned();
    loop {
        // 收口只有一个来源：订阅表条目被摘除（取消订阅、或连接整体收尾）置位的标志。
        // 事件流空闲时 `recv_timeout` 只返回 `Ok(None)`，永远不会给出「该退出了」的信号，
        // 因此这里必须显式检查，否则线程会一直陪跑到进程结束。
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match stream.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(event)) => {
                if sender
                    .blocking_send(TaggedEvent {
                        subscribe_id: subscribe_id.clone(),
                        event,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum WsClientRequest {
    Submit(WsEventRequest),
    /// 建立一条订阅。
    ///
    /// `subscribeId` **必填**：客户端与 daemon 同仓同装同发，版本由 launcher 的版本
    /// 目录统一，因此这里不保留旧形态、不做版本协商与字段别名。旧客户端缺该字段会
    /// 收到可读的 `subscribe_missing_id`（因此声明为 `Option` 并在此显式校验，
    /// 而不是依赖 serde 的裸报错文案），旧 daemon 会因 `deny_unknown_fields` 明确
    /// 拒绝新帧：两者都以可见的协议错误暴露，不会静默降级成单订阅。
    ///
    /// 帧字段与 `Submit` 内层信封同为 camelCase：枚举级 `rename_all` 只作用于
    /// 变体标签，结构体变体的字段必须单独声明，否则同一协议两套命名并存。
    #[serde(rename_all = "camelCase")]
    Subscribe {
        /// 单连接内唯一的订阅标识，由客户端生成（建议用目标的稳定串）。
        #[serde(default)]
        subscribe_id: Option<String>,
        workspace_id: String,
        /// 目标 Interaction；会话级订阅（子代理会话）时为空。
        #[serde(default)]
        interaction_id: Option<String>,
        /// 目标会话；给出时按会话级订阅处理，不看 Interaction。
        #[serde(default)]
        session_id: Option<String>,
        resume_cursor: Option<String>,
    },
    /// 取消一条订阅；只影响该标识对应的那一路。
    #[serde(rename_all = "camelCase")]
    Unsubscribe {
        subscribe_id: String,
    },
    Ping {},
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WsEventRequest {
    source_channel: String,
    event_id: String,
    /// 事件所属 Workspace；缺省时由服务端解析默认 Workspace。
    ///
    /// 与 HTTP 信封同语义：外部发送方不填，自带工作区上下文的客户端才填。
    #[serde(default)]
    workspace_id: Option<String>,
    observed_at_ms: u64,
    expires_at_ms: u64,
    content: String,
    reply_channel: String,
    reply_destination: String,
    /// 客户端已落地的本地附件绝对路径。
    #[serde(default)]
    local_files: Vec<String>,
}

impl WsEventRequest {
    fn into_event(self) -> ExternalEventEnvelope {
        ExternalEventEnvelope {
            source_channel: self.source_channel,
            event_id: self.event_id,
            workspace_id: self.workspace_id.unwrap_or_default().trim().to_owned(),
            observed_at_ms: self.observed_at_ms,
            expires_at_ms: self.expires_at_ms,
            content: self.content,
            title: None,
            correlation_key: None,
            reply_channel: self.reply_channel,
            reply_destination: self.reply_destination,
            attachments: self.local_files,
        }
    }
}

#[path = "gateway_websocket_frames.rs"]
mod gateway_websocket_frames;
use gateway_websocket_frames::*;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_wire_rejects_unknown_fields() {
        let result = serde_json::from_str::<WsClientRequest>(r#"{"type":"ping","unknown":true}"#);
        assert!(result.is_err());
    }

    #[test]
    fn websocket_submit_wire_carries_explicit_workspace_id() {
        let result = serde_json::from_str::<WsClientRequest>(
            r#"{"type":"submit","sourceChannel":"http","eventId":"event-1","workspaceId":"workspace-a","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1"}"#,
        )
        .expect("显式 workspaceId 必须可解析");
        let WsClientRequest::Submit(request) = result else {
            panic!("应为 submit");
        };
        assert_eq!(
            request.into_event().workspace_id,
            "workspace-a",
            "客户端显式给出的 Workspace 必须原样透传，由 bridge 决定采纳或回落"
        );
    }

    #[test]
    fn websocket_submit_wire_allows_server_side_workspace_resolution() {
        let result = serde_json::from_str::<WsClientRequest>(
            r#"{"type":"submit","sourceChannel":"http","eventId":"event-1","observedAtMs":100,"expiresAtMs":200,"content":"hello","replyChannel":"http","replyDestination":"client-1"}"#,
        )
        .expect("缺省 workspaceId 应可解析");
        let WsClientRequest::Submit(request) = result else {
            panic!("应为 submit");
        };
        assert!(request.into_event().workspace_id.is_empty());
    }

    #[test]
    fn websocket_subscribe_wire_uses_camel_case_fields() {
        let result = serde_json::from_str::<WsClientRequest>(
            r#"{"type":"subscribe","subscribeId":"interaction:interaction-1","workspaceId":"workspace-a","interactionId":"interaction-1","resumeCursor":"cursor"}"#,
        )
        .expect("订阅帧字段应与 Submit 信封同为 camelCase");
        let WsClientRequest::Subscribe {
            subscribe_id,
            workspace_id,
            interaction_id,
            session_id,
            resume_cursor,
        } = result
        else {
            panic!("应为 subscribe");
        };
        assert_eq!(subscribe_id.as_deref(), Some("interaction:interaction-1"));
        assert_eq!(workspace_id, "workspace-a");
        assert_eq!(interaction_id.as_deref(), Some("interaction-1"));
        assert!(session_id.is_none(), "车道订阅不得带会话目标");
        assert_eq!(resume_cursor.as_deref(), Some("cursor"));

        assert!(
            serde_json::from_str::<WsClientRequest>(
                r#"{"type":"subscribe","subscribeId":"interaction:interaction-1","workspace_id":"workspace-a","interaction_id":"interaction-1"}"#,
            )
            .is_err(),
            "旧 snake_case 字段必须被拒绝，同一协议不允许两套命名并存"
        );
    }

    #[test]
    fn websocket_subscribe_wire_accepts_session_target_and_omits_interaction() {
        let result = serde_json::from_str::<WsClientRequest>(
            r#"{"type":"subscribe","subscribeId":"session:parent--subagent--root","workspaceId":"workspace-a","sessionId":"parent--subagent--root"}"#,
        )
        .expect("会话级订阅帧应可解析");
        let WsClientRequest::Subscribe {
            subscribe_id,
            interaction_id,
            session_id,
            resume_cursor,
            ..
        } = result
        else {
            panic!("应为 subscribe");
        };
        assert_eq!(
            subscribe_id.as_deref(),
            Some("session:parent--subagent--root")
        );
        assert!(
            interaction_id.is_none(),
            "会话级订阅缺省 Interaction 时必须解析为 None，不能退化成空串目标"
        );
        assert_eq!(
            session_id.as_deref(),
            Some("parent--subagent--root"),
            "会话级订阅必须原样透传会话标识"
        );
        assert!(resume_cursor.is_none());
    }

    #[test]
    fn websocket_subscribe_wire_keeps_missing_subscribe_id_visible() {
        // 缺标识不交给 serde 报错：解析成 None，由 handle_request 回可读的
        // `subscribe_missing_id`，错误码稳定且与文案解耦。
        let result = serde_json::from_str::<WsClientRequest>(
            r#"{"type":"subscribe","workspaceId":"workspace-a","interactionId":"interaction-1"}"#,
        )
        .expect("缺 subscribeId 的订阅帧必须仍可解析，才能回可读错误码");
        let WsClientRequest::Subscribe { subscribe_id, .. } = result else {
            panic!("应为 subscribe");
        };
        assert!(subscribe_id.is_none(), "缺字段必须解析为 None 而不是空串");
    }

    #[test]
    fn websocket_unsubscribe_wire_carries_subscribe_id() {
        let result = serde_json::from_str::<WsClientRequest>(
            r#"{"type":"unsubscribe","subscribeId":"session:parent--subagent--root"}"#,
        )
        .expect("取消订阅帧应可解析");
        let WsClientRequest::Unsubscribe { subscribe_id } = result else {
            panic!("应为 unsubscribe");
        };
        assert_eq!(subscribe_id, "session:parent--subagent--root");
    }

    #[test]
    fn websocket_event_projection_is_bounded_to_structured_fields() {
        let tagged = TaggedEvent {
            subscribe_id: "interaction:interaction-1".to_owned(),
            event: GatewayEvent {
                event_type: "turn_delta".to_owned(),
                payload_json: "{\"text\":\"ok\"}".to_owned(),
                cursor: Some("cursor".to_owned()),
                replay: true,
            },
        };
        let output = event_json(&tagged);
        assert!(output.contains("turn_delta"));
        // 载荷必须是内嵌对象而不是转义字符串：帧体量不该被二次转义放大。
        assert!(output.contains(r#""payload":{"text":"ok"}"#), "{output}");
        assert!(
            !output.contains(r#"\"text\""#),
            "载荷不得再转义一层：{output}"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).expect("事件帧")["replay"],
            true
        );
        assert!(
            output.contains("\"subscribeId\":\"interaction:interaction-1\""),
            "事件帧必须回显订阅标识，多路订阅才能各归其位"
        );
        assert!(!output.contains("Authorization"));
    }

    #[test]
    fn subscription_error_frames_echo_the_subscribe_id_when_known() {
        assert!(
            subscription_error_json("subscribe_duplicate_id", Some("session:a"))
                .contains("\"subscribeId\":\"session:a\""),
            "订阅级错误必须带标识，客户端才能定位被拒的那一路"
        );
        assert!(
            subscription_error_json("subscription_rejected", Some("interaction:b"))
                .contains("\"subscribeId\":\"interaction:b\""),
            "目标被拒同样要回显标识：一条连接上可能并存多路订阅"
        );
        assert!(
            !error_json("subscribe_missing_id").contains("subscribeId"),
            "连标识都没有的请求无法回显标识，只能给出可读错误码"
        );
    }
}

#[cfg(test)]
#[path = "gateway_websocket_tests.rs"]
mod gateway_websocket_tests;
