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

#[test]
fn fast_path_answers_without_entering_the_handler() {
    // 主处理链被长请求占住时，探活必须仍能被旁路直接答掉。
    let request = DaemonRequest {
        message_type: "request".to_owned(),
        request_id: "health-1".to_owned(),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: TEST_CLIENT_ID.to_owned(),
        session_id: None,
        command: DaemonCommand::GetHealth,
    };
    let line = encode_request(&request).expect("编码请求");
    let handler = Arc::new(Mutex::new(|_request: DaemonRequest| -> IpcDispatch {
        panic!("旁路命中时不得进入主处理链");
    }));
    let fast_path: Option<IpcFastPath> = Some(Arc::new(|request: &DaemonRequest| {
        matches!(request.command, DaemonCommand::GetHealth).then(|| {
            IpcDispatch::complete(vec![DaemonResponse::success(
                request.request_id.clone(),
                serde_json::json!({"status":"ok"}),
            )])
        })
    }));

    let dispatch = parse_and_dispatch(&line, &handler, &fast_path).expect("解析并派发");
    let IpcDispatch::Complete(responses) = dispatch else {
        panic!("旁路只给单次响应");
    };
    assert_eq!(responses.len(), 1);
    assert!(responses[0].ok);
    assert_eq!(responses[0].request_id, "health-1");
    assert_eq!(responses[0].body["status"], "ok");
}

#[test]
fn requests_outside_the_fast_path_still_reach_the_handler() {
    let request = DaemonRequest {
        message_type: "request".to_owned(),
        request_id: "state-1".to_owned(),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: TEST_CLIENT_ID.to_owned(),
        session_id: Some("session-1".to_owned()),
        command: DaemonCommand::GetState {
            session_id: "session-1".to_owned(),
        },
    };
    let line = encode_request(&request).expect("编码请求");
    let handler = Arc::new(Mutex::new(|request: DaemonRequest| -> IpcDispatch {
        IpcDispatch::complete(vec![DaemonResponse::success(
            request.request_id,
            serde_json::json!({"handled":true}),
        )])
    }));
    let fast_path: Option<IpcFastPath> = Some(Arc::new(|request: &DaemonRequest| {
        matches!(request.command, DaemonCommand::GetHealth).then(|| {
            IpcDispatch::complete(vec![DaemonResponse::success(
                request.request_id.clone(),
                serde_json::json!({"status":"ok"}),
            )])
        })
    }));

    let dispatch = parse_and_dispatch(&line, &handler, &fast_path).expect("解析并派发");
    let IpcDispatch::Complete(responses) = dispatch else {
        panic!("主处理链只给单次响应");
    };
    assert_eq!(responses[0].body["handled"], true);
}
