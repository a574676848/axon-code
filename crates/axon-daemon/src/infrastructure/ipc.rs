//! IPC 端口的 JSONL server/client。
//!
//! 本模块保留服务端（accept、连接限额、派发与响应写出）与共享的端点命名和订阅心跳；
//! 客户端（`JsonlIpcClient` 与订阅句柄）在 `client` 子模块中，两侧各自演进互不牵连。

use axon_protocol::{DaemonRequest, DaemonResponse};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, Listener, ListenerOptions, Stream, prelude::*,
};
use std::{
    io::{self, BufRead, BufReader, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

#[path = "ipc/client.rs"]
mod client;

pub use client::{JsonlIpcClient, JsonlIpcSubscription};

const MAX_CONCURRENT_CONNECTIONS: usize = 256;
/// 订阅长连接的服务端心跳周期，也是客户端空闲 watchdog 的基本时间片。
pub const SUBSCRIPTION_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const CONNECTION_THREAD_NAME: &str = "axon-daemon-ipc";

fn local_socket_name(endpoint: &str) -> io::Result<interprocess::local_socket::Name<'_>> {
    if Path::new(endpoint).is_absolute() {
        endpoint.to_fs_name::<GenericFilePath>()
    } else {
        endpoint.to_ns_name::<GenericNamespaced>()
    }
}

/// 仅授权目标用户、SYSTEM 和管理员，并允许同一用户的中完整性客户端读写。
#[cfg(windows)]
const WINDOWS_PIPE_SDDL_TEMPLATE: &str =
    "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{user_sid})S:(ML;;NW;;;ME)";

/// 构造授予目标用户访问权的命名管道安全描述符。
#[cfg(windows)]
fn windows_pipe_security_descriptor(
    user_sid: &str,
) -> io::Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    let sddl = WINDOWS_PIPE_SDDL_TEMPLATE.replace("{user_sid}", user_sid);
    let wide = widestring::U16CString::from_str(sddl)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    interprocess::os::windows::security_descriptor::SecurityDescriptor::deserialize(&wide)
}

#[path = "ipc/snapshot_stream.rs"]
pub mod snapshot_stream;

/// JSONL 本地 IPC 服务端。
pub struct JsonlIpcServer {
    listener: Listener,
}

/// 一个 IPC 请求的完整响应或持续响应流。
pub enum IpcDispatch {
    /// 请求完成后关闭本轮响应。
    Complete(Vec<DaemonResponse>),
    /// 先发送 durable replay，再持续推送实时响应。
    Subscription {
        /// 订阅建立时的 durable replay。
        initial: Vec<DaemonResponse>,
        /// 实时响应源。
        stream: Box<dyn IpcResponseStream>,
    },
}

/// IPC 长连接的响应源。
pub trait IpcResponseStream: Send {
    /// 等待下一条响应；返回 None 表示等待超时，由 IPC 层发送连接心跳。
    fn recv_timeout(&mut self, timeout: Duration) -> io::Result<Option<DaemonResponse>>;

    /// 等待一批响应。默认实现保持单响应流兼容，具体订阅流可在既有边界内批量返回。
    fn recv_timeout_batch(&mut self, timeout: Duration) -> io::Result<Option<Vec<DaemonResponse>>> {
        self.recv_timeout(timeout)
            .map(|response| response.map(|value| vec![value]))
    }
}

struct ActiveConnection {
    count: Arc<AtomicUsize>,
}

impl Drop for ActiveConnection {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::AcqRel);
    }
}

impl IpcDispatch {
    /// 构造一次性响应。
    #[must_use]
    pub fn complete(responses: Vec<DaemonResponse>) -> Self {
        Self::Complete(responses)
    }
}

impl JsonlIpcServer {
    /// 绑定跨平台 namespaced endpoint。
    pub fn bind(endpoint: &str) -> io::Result<Self> {
        let name = local_socket_name(endpoint)?;
        let listener = ListenerOptions::new().name(name).create_sync()?;
        Ok(Self { listener })
    }

    /// 绑定 endpoint，并在 Windows 上授予指定用户 SID 访问命名管道的权限。
    ///
    /// Windows 最高权限任务与普通权限 CLI/TUI 的令牌不同；显式授权同一用户 SID，
    /// 并把管道完整性标签设为中级，避免默认权限导致 `Access Denied`。
    /// `user_sid` 为空或非 Windows 平台时与 [`JsonlIpcServer::bind`] 等价。
    pub fn bind_with_user_sid(endpoint: &str, user_sid: Option<&str>) -> io::Result<Self> {
        let name = local_socket_name(endpoint)?;
        #[cfg(windows)]
        let options = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            let options = ListenerOptions::new().name(name);
            match user_sid {
                Some(sid) => options.security_descriptor(windows_pipe_security_descriptor(sid)?),
                None => options,
            }
        };
        #[cfg(not(windows))]
        let options = {
            let _ = user_sid;
            ListenerOptions::new().name(name)
        };
        Ok(Self {
            listener: options.create_sync()?,
        })
    }

    /// 串行处理一个连接；每个请求可返回多条 JSONL 响应。
    pub fn serve_next<F>(&self, mut handler: F) -> io::Result<()>
    where
        F: FnMut(DaemonRequest) -> Vec<DaemonResponse>,
    {
        let connection = self.listener.accept()?;
        serve_connection(connection, &mut handler)
    }

    /// 持续接受连接；客户端断开不会终止 Daemon Worker。
    pub fn serve_forever<F>(&self, mut handler: F) -> io::Result<()>
    where
        F: FnMut(DaemonRequest) -> Vec<DaemonResponse>,
    {
        loop {
            let connection = self.listener.accept()?;
            serve_connection(connection, &mut handler)?;
        }
    }

    /// 并发接受连接；订阅连接持续推送时不会阻塞其他命令。
    pub fn serve_forever_streaming<F>(&self, handler: F) -> io::Result<()>
    where
        F: FnMut(DaemonRequest) -> IpcDispatch + Send + 'static,
    {
        self.serve_streaming_connections(handler, None)
    }

    /// 并发接受连接，并在 `should_stop` 为真时结束接受循环。
    ///
    /// Windows 命名管道在客户端发起连接时即进入 connected 状态，等待中的
    /// `accept` 会被一次唤醒连接放行；调用方据此在宿主停机请求后退出循环，
    /// 让 Daemon 走完优雅停机，而不是直接终止进程。
    pub fn serve_forever_streaming_until<F, S>(&self, handler: F, should_stop: S) -> io::Result<()>
    where
        F: FnMut(DaemonRequest) -> IpcDispatch + Send + 'static,
        S: Fn() -> bool,
    {
        self.serve_streaming_connections_until(handler, None, &should_stop)
    }

    fn serve_streaming_connections<F>(
        &self,
        handler: F,
        connection_limit: Option<usize>,
    ) -> io::Result<()>
    where
        F: FnMut(DaemonRequest) -> IpcDispatch + Send + 'static,
    {
        self.serve_streaming_connections_until(handler, connection_limit, &|| false)
    }

    fn serve_streaming_connections_until<F, S>(
        &self,
        handler: F,
        connection_limit: Option<usize>,
        should_stop: &S,
    ) -> io::Result<()>
    where
        F: FnMut(DaemonRequest) -> IpcDispatch + Send + 'static,
        S: Fn() -> bool,
    {
        let handler = Arc::new(Mutex::new(handler));
        let active_connections = Arc::new(AtomicUsize::new(0));
        let mut accepted_connections = 0_usize;
        while connection_limit.is_none_or(|limit| accepted_connections < limit) {
            if should_stop() {
                return Ok(());
            }
            let connection = self.listener.accept()?;
            if should_stop() {
                return Ok(());
            }
            let Some(active) = acquire_connection(&active_connections) else {
                drop(connection);
                continue;
            };
            let handler = Arc::clone(&handler);
            thread::Builder::new()
                .name(CONNECTION_THREAD_NAME.to_owned())
                .spawn(move || {
                    let _active = active;
                    let _ = serve_streaming_connection(connection, &handler);
                })
                .map_err(io::Error::other)?;
            accepted_connections = accepted_connections.saturating_add(1);
        }
        Ok(())
    }
}

fn acquire_connection(count: &Arc<AtomicUsize>) -> Option<ActiveConnection> {
    count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < MAX_CONCURRENT_CONNECTIONS).then_some(current.saturating_add(1))
        })
        .ok()
        .map(|_| ActiveConnection {
            count: Arc::clone(count),
        })
}

fn serve_streaming_connection<F>(connection: Stream, handler: &Arc<Mutex<F>>) -> io::Result<()>
where
    F: FnMut(DaemonRequest) -> IpcDispatch,
{
    let mut reader = BufReader::new(connection);
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let dispatch = parse_and_dispatch(line.trim_end(), handler)?;
        write_dispatch(reader.get_mut(), dispatch)?;
        line.clear();
    }
    Ok(())
}

fn parse_and_dispatch<F>(line: &str, handler: &Arc<Mutex<F>>) -> io::Result<IpcDispatch>
where
    F: FnMut(DaemonRequest) -> IpcDispatch,
{
    let request = match serde_json::from_str::<DaemonRequest>(line) {
        Ok(request) => request,
        Err(error) => {
            return Ok(IpcDispatch::complete(vec![invalid_request_response(error)]));
        }
    };
    let mut handler = handler
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(handler(request))
}

fn write_dispatch(writer: &mut Stream, dispatch: IpcDispatch) -> io::Result<()> {
    match dispatch {
        IpcDispatch::Complete(responses) => write_responses(writer, responses),
        IpcDispatch::Subscription {
            initial,
            mut stream,
        } => {
            let request_id = initial
                .first()
                .map(|response| response.request_id.clone())
                .unwrap_or_default();
            write_responses(writer, initial)?;
            loop {
                let responses = stream
                    .recv_timeout_batch(SUBSCRIPTION_HEARTBEAT_INTERVAL)?
                    .unwrap_or_else(|| vec![subscription_heartbeat(&request_id)]);
                write_responses(writer, responses)?;
            }
        }
    }
}

fn write_responses(
    writer: &mut Stream,
    responses: impl IntoIterator<Item = DaemonResponse>,
) -> io::Result<()> {
    for response in responses {
        write_response_body(writer, &response)?;
    }
    writer.flush()
}

fn write_response_body(writer: &mut Stream, response: &DaemonResponse) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")
}

fn invalid_request_response(error: serde_json::Error) -> DaemonResponse {
    DaemonResponse {
        request_id: String::new(),
        protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
        ok: false,
        body: serde_json::json!({"error": error.to_string()}),
        cursor: None,
    }
}

fn subscription_heartbeat(request_id: &str) -> DaemonResponse {
    DaemonResponse::success(
        request_id,
        serde_json::json!({"event": "subscription_heartbeat"}),
    )
}

fn serve_connection<F>(connection: Stream, handler: &mut F) -> io::Result<()>
where
    F: FnMut(DaemonRequest) -> Vec<DaemonResponse>,
{
    let mut reader = BufReader::new(connection);
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let response = match serde_json::from_str::<DaemonRequest>(line.trim_end()) {
            Ok(request) => handler(request),
            Err(error) => vec![DaemonResponse {
                request_id: String::new(),
                protocol_version: axon_protocol::DAEMON_PROTOCOL_VERSION,
                ok: false,
                body: serde_json::json!({"error": error.to_string()}),
                cursor: None,
            }],
        };
        for item in response {
            serde_json::to_writer(reader.get_mut(), &item)?;
            reader.get_mut().write_all(b"\n")?;
        }
        reader.get_mut().flush()?;
        line.clear();
    }
    Ok(())
}

/// 把请求编码为单行 JSON。
pub fn encode_request(request: &DaemonRequest) -> Result<String, serde_json::Error> {
    serde_json::to_string(request)
}

/// 从单行 JSON 解码响应。
pub fn decode_response(line: &str) -> Result<DaemonResponse, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    //! 端点在 Windows 上落在全局命名管道命名空间里，因此每个用例都必须使用自己的唯一端点
    //! （`axon-ipc-<用例>-<uuid>`），既不能复用生产端点 `axon-daemon`，也不能跨用例共享：
    //! 固定名会让测试互相争抢实例，也会与真实 Daemon 或并发测试进程互相影响。

    use super::*;
    use axon_domain::DaemonSessionId;
    use axon_protocol::{DAEMON_PROTOCOL_VERSION, DaemonCommand};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Instant;
    use uuid::Uuid;

    const TEST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
    const TEST_CLIENT_ID: &str = "ipc-streaming-test";
    /// 服务端故意延迟响应的时长，必须显著大于短超时，给超时判定留出余量。
    const TEST_LATE_RESPONSE_DELAY: Duration = Duration::from_millis(1_000);
    /// 短超时：只验证「不无限等待」，不追求精确计时。
    const TEST_SHORT_TIMEOUT: Duration = Duration::from_millis(200);

    #[test]
    fn send_with_timeout_reports_late_response_without_losing_the_connection() {
        let endpoint = format!("axon-ipc-timeout-{}", Uuid::new_v4());
        let server = JsonlIpcServer::bind(&endpoint).expect("绑定测试 IPC");
        let server_thread = thread::spawn(move || {
            // 客户端超时后按契约丢弃连接，服务端可能在写迟到响应时看到管道关闭，
            // 这是本用例的预期结果而不是缺陷。
            let _ = server.serve_next(|request| {
                thread::sleep(TEST_LATE_RESPONSE_DELAY);
                vec![DaemonResponse::success(
                    request.request_id,
                    serde_json::json!({"status":"ok"}),
                )]
            });
        });
        let mut client = JsonlIpcClient::connect(&endpoint).expect("连接测试 IPC");
        let command = || DaemonCommand::GetState {
            session_id: "ipc-timeout-session".to_owned(),
        };

        let started = Instant::now();
        let timeout = client.send_with_timeout(&request("late", command()), TEST_SHORT_TIMEOUT);
        let elapsed = started.elapsed();

        assert_eq!(
            timeout.expect_err("迟到响应必须超时失败").kind(),
            io::ErrorKind::TimedOut
        );
        assert!(
            elapsed < TEST_LATE_RESPONSE_DELAY,
            "超时必须先于服务端响应返回: elapsed={elapsed:?}"
        );
        // 迟到响应仍然在连接上：这正是调用方超时后必须丢弃连接、不得复用的原因。
        let late = client
            .send_with_timeout(&request("recovered", command()), TEST_RESPONSE_TIMEOUT)
            .expect("迟到响应仍可被读取");
        assert_eq!(late.body["status"], "ok");
        drop(client);
        server_thread.join().expect("服务端线程正常退出");
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_descriptor_accepts_user_acl_and_medium_integrity() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert!(windows_pipe_security_descriptor(sid).is_ok());
        let sddl = WINDOWS_PIPE_SDDL_TEMPLATE.replace("{user_sid}", sid);
        assert!(sddl.contains("(A;;GA;;;S-1-5-21-1-2-3-1001)"));
        assert!(sddl.ends_with("S:(ML;;NW;;;ME)"));
        assert!(!sddl.contains(";;;WD)"));
    }

    /// Daemon 接受连接但从不回答订阅握手时，客户端必须失败返回而不是永久阻塞。
    #[test]
    fn subscribe_handshake_times_out_when_the_daemon_never_answers() {
        let endpoint = format!("axon-ipc-handshake-{}", Uuid::new_v4());
        let server = JsonlIpcServer::bind(&endpoint).expect("绑定测试 IPC");
        let (served_sender, served_receiver) = mpsc::sync_channel(1);
        // 客户端未建立连接时服务端会一直阻塞在 accept：这里只做有界等待并让线程自然结束，
        // 不用无界 join，避免测试进程被钉死并长期占用 Windows 管道实例。
        let _server_thread = thread::spawn(move || {
            // 只读请求、不回响应：连接保持打开但没有 subscription_ready。
            let _ = server.serve_next(|_| Vec::new());
            let _ = served_sender.try_send(());
        });
        let request = request(
            "subscribe",
            DaemonCommand::SubscribeEvents {
                session_id: "handshake-session".to_owned(),
                interaction_id: "handshake-interaction".to_owned(),
                resume_cursor: None,
            },
        );

        let started = Instant::now();
        let error = JsonlIpcClient::connect(&endpoint)
            .and_then(|client| client.subscribe_with_timeout(&request, TEST_SHORT_TIMEOUT))
            .err()
            .expect("握手超时必须失败返回");

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < TEST_LATE_RESPONSE_DELAY,
            "握手超时必须先于任何迟到的响应返回: elapsed={:?}",
            started.elapsed()
        );
        served_receiver
            .recv_timeout(TEST_RESPONSE_TIMEOUT)
            .expect("客户端必须建立连接并让服务端读完请求");
    }

    struct ControlledResponseStream {
        wake_receiver: Receiver<()>,
        dropped_sender: mpsc::SyncSender<()>,
    }

    struct StreamingServerFixture {
        endpoint: String,
        wake_sender: mpsc::SyncSender<()>,
        dropped_receiver: Receiver<()>,
        server_thread: thread::JoinHandle<()>,
    }

    impl IpcResponseStream for ControlledResponseStream {
        fn recv_timeout(&mut self, timeout: Duration) -> io::Result<Option<DaemonResponse>> {
            match self.wake_receiver.recv_timeout(timeout) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "测试订阅唤醒通道已关闭",
                )),
            }
        }
    }

    impl Drop for ControlledResponseStream {
        fn drop(&mut self) {
            let _ = self.dropped_sender.try_send(());
        }
    }

    impl StreamingServerFixture {
        fn start() -> Self {
            let endpoint = format!("axon-ipc-streaming-{}", Uuid::new_v4());
            let server = JsonlIpcServer::bind(&endpoint).expect("绑定测试 IPC");
            let (wake_sender, wake_receiver) = mpsc::sync_channel(2);
            let (dropped_sender, dropped_receiver) = mpsc::sync_channel(1);
            let mut wake_receiver = Some(wake_receiver);
            let server_thread = thread::spawn(move || {
                server
                    .serve_streaming_connections(
                        move |request| test_dispatch(request, &mut wake_receiver, &dropped_sender),
                        Some(2),
                    )
                    .expect("处理两个测试连接");
            });
            Self {
                endpoint,
                wake_sender,
                dropped_receiver,
                server_thread,
            }
        }

        fn subscribe(&self) -> JsonlIpcSubscription {
            let request = subscription_request();
            let client = JsonlIpcClient::connect(&self.endpoint).expect("连接测试 IPC");
            client.subscribe(&request).expect("建立测试订阅")
        }

        fn send_command(&self) -> DaemonResponse {
            let endpoint = self.endpoint.clone();
            let (sender, receiver) = mpsc::sync_channel(1);
            thread::spawn(move || {
                let result = JsonlIpcClient::connect(&endpoint).and_then(|mut client| {
                    client.send(&request(
                        "command",
                        DaemonCommand::GetState {
                            session_id: "ipc-stream-session".to_owned(),
                        },
                    ))
                });
                let _ = sender.send(result);
            });
            receiver
                .recv_timeout(TEST_RESPONSE_TIMEOUT)
                .expect("订阅不应阻塞普通命令")
                .expect("普通命令响应有效")
        }

        fn finish(self, subscription: JsonlIpcSubscription) {
            // 不再唤醒服务端流；客户端句柄释放必须取消后台 reader 并关闭 socket。
            drop(subscription);
            self.dropped_receiver
                .recv_timeout(TEST_RESPONSE_TIMEOUT)
                .expect("客户端主动断开应释放服务端订阅流");
            drop(self.wake_sender);
            self.server_thread.join().expect("服务端接受线程正常退出");
        }
    }

    #[test]
    fn streaming_subscription_does_not_block_commands_and_disconnect_releases_stream() {
        let fixture = StreamingServerFixture::start();
        let mut subscription = fixture.subscribe();
        let ready = subscription.recv().expect("订阅 ready 响应有效");
        assert_eq!(ready.body["event"], "subscription_ready");

        let command_response = fixture.send_command();
        assert_eq!(command_response.body["status"], "ok");
        fixture.finish(subscription);
    }

    #[test]
    fn subscription_delivers_server_push_after_subscribe() {
        // 服务端对订阅连接按 SUBSCRIPTION_HEARTBEAT_INTERVAL 持续推送心跳。订阅 reader
        // 进入非阻塞轮询后必须把「0 字节读取」当作未就绪而不是连接关闭，否则句柄只会
        // 交付初始响应，后续推送全部静默丢失。
        let fixture = StreamingServerFixture::start();
        let mut subscription = fixture.subscribe();
        let ready = subscription.recv().expect("订阅 ready 响应有效");
        assert_eq!(ready.body["event"], "subscription_ready");

        let pushed = subscription
            .recv_timeout(SUBSCRIPTION_HEARTBEAT_INTERVAL * 2 + TEST_RESPONSE_TIMEOUT)
            .expect("订阅通道不应关闭")
            .expect("服务端推送必须到达订阅方");

        assert_eq!(pushed.body["event"], "subscription_heartbeat");
        // 订阅连接持续读取期间普通命令仍必须能往返（第 2 条连接）。
        assert_eq!(fixture.send_command().body["status"], "ok");
        fixture.finish(subscription);
    }

    fn test_dispatch(
        request: DaemonRequest,
        wake_receiver: &mut Option<Receiver<()>>,
        dropped_sender: &mpsc::SyncSender<()>,
    ) -> IpcDispatch {
        match request.command {
            DaemonCommand::SubscribeEvents { .. } => IpcDispatch::Subscription {
                initial: vec![DaemonResponse::success(
                    request.request_id,
                    serde_json::json!({"event":"subscription_ready"}),
                )],
                stream: Box::new(ControlledResponseStream {
                    wake_receiver: wake_receiver.take().expect("仅建立一个测试订阅"),
                    dropped_sender: dropped_sender.clone(),
                }),
            },
            _ => IpcDispatch::complete(vec![DaemonResponse::success(
                request.request_id,
                serde_json::json!({"status":"ok"}),
            )]),
        }
    }

    fn subscription_request() -> DaemonRequest {
        request(
            "subscription",
            DaemonCommand::SubscribeEvents {
                session_id: DaemonSessionId::new("ipc-stream-session")
                    .expect("Session ID")
                    .to_string(),
                interaction_id: "ipc-interaction".to_owned(),
                resume_cursor: None,
            },
        )
    }

    fn request(request_id: &str, command: DaemonCommand) -> DaemonRequest {
        DaemonRequest {
            message_type: "command".to_owned(),
            request_id: request_id.to_owned(),
            protocol_version: DAEMON_PROTOCOL_VERSION,
            client_id: TEST_CLIENT_ID.to_owned(),
            session_id: None,
            command,
        }
    }

    #[test]
    fn stop_aware_serve_loop_exits_after_wakeup_connection() {
        use std::sync::atomic::AtomicBool;

        let endpoint = format!("axon-ipc-stop-{}", Uuid::new_v4());
        let server = JsonlIpcServer::bind(&endpoint).expect("绑定测试 IPC");
        let stop_requested = Arc::new(AtomicBool::new(false));
        let serve_stop = Arc::clone(&stop_requested);
        let server_thread = thread::spawn(move || {
            server
                .serve_forever_streaming_until(
                    |request| {
                        IpcDispatch::complete(vec![DaemonResponse::success(
                            request.request_id,
                            serde_json::json!({"status":"ok"}),
                        )])
                    },
                    move || serve_stop.load(Ordering::Acquire),
                )
                .expect("停止请求后退出接受循环");
        });
        stop_requested.store(true, Ordering::Release);
        // 唤醒连接：Windows 命名管道与 Unix domain socket 都靠它放行阻塞中的 accept。
        let _ = JsonlIpcClient::connect(&endpoint);
        server_thread.join().expect("接受循环线程正常退出");
    }
}
