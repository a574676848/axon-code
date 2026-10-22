//! Provider WebSocket 后台帧泵。

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Error as WebSocketError, Message},
};

const PROVIDER_WEBSOCKET_INBOUND_FRAME_CHANNEL_CAPACITY: usize = 64;

pub(super) type ProviderWebSocket =
    WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

enum WebSocketPumpCommand {
    Send {
        message: Message,
        result: oneshot::Sender<Result<(), WebSocketError>>,
    },
}

/// 持续读取 Provider WebSocket，保证本地工具执行期间仍能处理 Ping/Close。
pub(super) struct ProviderWebSocketPump {
    commands: mpsc::Sender<WebSocketPumpCommand>,
    messages: mpsc::Receiver<Result<Message, WebSocketError>>,
    task: tokio::task::JoinHandle<()>,
}

impl ProviderWebSocketPump {
    pub(super) fn new(socket: ProviderWebSocket) -> Self {
        let (commands, mut command_rx) = mpsc::channel::<WebSocketPumpCommand>(32);
        let (message_tx, messages) =
            mpsc::channel(PROVIDER_WEBSOCKET_INBOUND_FRAME_CHANNEL_CAPACITY);
        let task = tokio::spawn(async move {
            let mut socket = socket;
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let Some(WebSocketPumpCommand::Send { message, result }) = command else {
                            break;
                        };
                        let send_result = socket.send(message).await;
                        let failed = send_result.is_err();
                        let _ = result.send(send_result);
                        if failed {
                            break;
                        }
                    }
                    message = socket.next() => {
                        let Some(message) = message else {
                            break;
                        };
                        match message {
                            Ok(Message::Ping(payload)) => {
                                if let Err(error) = socket.send(Message::Pong(payload)).await {
                                    let _ = send_inbound_message(&message_tx, Err(error)).await;
                                    break;
                                }
                            }
                            Ok(Message::Pong(_)) => {}
                            Ok(message) => {
                                let closed = matches!(message, Message::Close(_));
                                if !send_inbound_message(&message_tx, Ok(message)).await || closed {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = send_inbound_message(&message_tx, Err(error)).await;
                                break;
                            }
                        }
                    }
                }
            }
        });
        Self {
            commands,
            messages,
            task,
        }
    }

    pub(super) fn is_closed(&self) -> bool {
        self.commands.is_closed() || self.task.is_finished()
    }

    pub(super) async fn send(&self, message: Message) -> Result<(), WebSocketError> {
        let (result, result_rx) = oneshot::channel();
        self.commands
            .send(WebSocketPumpCommand::Send { message, result })
            .await
            .map_err(|_| WebSocketError::ConnectionClosed)?;
        result_rx
            .await
            .unwrap_or(Err(WebSocketError::ConnectionClosed))
    }

    pub(super) async fn next(&mut self) -> Option<Result<Message, WebSocketError>> {
        self.messages.recv().await
    }
}

async fn send_inbound_message(
    messages: &mpsc::Sender<Result<Message, WebSocketError>>,
    message: Result<Message, WebSocketError>,
) -> bool {
    messages.send(message).await.is_ok()
}

impl Drop for ProviderWebSocketPump {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    use super::*;

    #[tokio::test]
    async fn pump_backpressures_unsolicited_frames_without_dropping_them() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("测试服务应绑定本机端口");
        let address = listener.local_addr().expect("测试服务应提供监听地址");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("测试服务应接受连接");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("测试服务应完成 WebSocket 握手");
            for index in 0..=PROVIDER_WEBSOCKET_INBOUND_FRAME_CHANNEL_CAPACITY {
                if socket
                    .send(Message::Text(format!("{{\"frame\":{index}}}").into()))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let (socket, _) = connect_async(format!("ws://{address}"))
            .await
            .expect("测试客户端应连接");
        let mut pump = ProviderWebSocketPump::new(socket);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(
            !pump.is_closed(),
            "入站队列暂满时应对上游施加背压，而不是主动断连"
        );
        for index in 0..=PROVIDER_WEBSOCKET_INBOUND_FRAME_CHANNEL_CAPACITY {
            let message = tokio::time::timeout(std::time::Duration::from_secs(1), pump.next())
                .await
                .expect("pump 应在消费者恢复后继续转发帧")
                .expect("连接不应提前关闭")
                .expect("每个帧应可读取");
            assert_eq!(
                message.into_text().expect("应为文本帧"),
                format!("{{\"frame\":{index}}}")
            );
        }
        server.await.expect("测试服务任务应结束");
    }
}
