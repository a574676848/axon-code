//! WebSocket 侧的轮次头订阅装配。
//!
//! 只做「按前缀分派」这一件事：`session-turns:<sessionId>` 走轮次头，其余仍走普通
//! 会话 / 交互订阅。装配流程与普通订阅一致（校验、起线程、登记、回显 `subscribed`），
//! 因此这里只把这段流程从协议处理里抽出来，避免主文件无限增长。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use futures_util::SinkExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::protocol::Message;

use super::gateway_kernel::{GatewayContext, GatewaySessionTurnsRequest};
use super::gateway_session_turns_stream::session_turns_target;
use super::gateway_websocket::{SubscriptionEntry, Subscriptions, forward_events};
use super::gateway_websocket::{TaggedEvent, subscription_error_json};

/// 命中轮次头前缀时返回目标会话标识；其它订阅前缀一律不命中。
pub(super) fn turns_target(subscribe_id: &str) -> Option<&str> {
    session_turns_target(subscribe_id)
}

/// 装配一条轮次头订阅：建流、起转发线程、登记并回显确认。
///
/// 被拒时先回订阅级错误帧再返回，调用方据此结束本次请求处理；连接与其它订阅不受影响。
pub(super) async fn handle_subscribe(
    context: &GatewayContext,
    subscribe_id: &str,
    workspace_id: &str,
    session_id: &str,
    event_sender: &mpsc::Sender<TaggedEvent>,
    subscriptions: &mut Subscriptions,
    socket: &mut WebSocketStream<TcpStream>,
) -> Result<bool, WsError> {
    let request = GatewaySessionTurnsRequest {
        workspace_id: workspace_id.to_owned(),
        session_id: session_id.to_owned(),
    };
    let subscribe_context = context.clone();
    let stream = match tokio::task::spawn_blocking(move || {
        subscribe_context.subscribe_session_turns(request)
    })
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            axon_logger::warning(&format!(
                "[gateway-ws] 轮次头订阅被拒 id={subscribe_id} 原因={error:?}"
            ));
            socket
                .send(Message::Text(
                    subscription_error_json("subscription_rejected", Some(subscribe_id)).into(),
                ))
                .await?;
            return Ok(true);
        }
        Err(_) => {
            socket
                .send(Message::Text(
                    subscription_error_json("subscription_rejected", Some(subscribe_id)).into(),
                ))
                .await?;
            return Ok(true);
        }
    };
    let thread_sender = event_sender.clone();
    let shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown = Arc::clone(&shutdown);
    let tag = subscribe_id.to_owned();
    let thread =
        std::thread::spawn(move || forward_events(&tag, stream, thread_sender, thread_shutdown));
    subscriptions.insert(
        subscribe_id.to_owned(),
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
    Ok(true)
}
