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

/// 崩溃遗留的 POSIX 端点文件回收；Windows 命名管道不留文件，不参与编译。
#[cfg(unix)]
#[path = "ipc/stale_endpoint.rs"]
mod stale_endpoint;

/// JSONL 本地 IPC 服务端。
pub struct JsonlIpcServer {
    listener: Listener,
    /// 不取主处理链锁的应答旁路：只用于「读一份已经备好的快照」这类请求。
    ///
    /// 主处理链被一个长请求占住时，探活仍要继续可用；没有旁路时健康检查会被迫排队，
    /// 上游就会把「慢」误判成「挂了」。
    fast_path: Option<IpcFastPath>,
}

/// IPC 快速旁路：命中时直接给出应答，不参与主处理链的互斥。
pub type IpcFastPath = Arc<dyn Fn(&DaemonRequest) -> Option<IpcDispatch> + Send + Sync>;

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
        // POSIX 上崩溃会留下 socket 文件，bind 前先回收无人监听的那份。
        #[cfg(unix)]
        stale_endpoint::reclaim(endpoint);
        let listener = ListenerOptions::new().name(name).create_sync()?;
        Ok(Self {
            listener,
            fast_path: None,
        })
    }

    /// 绑定 endpoint，并在 Windows 上授予指定用户 SID 访问命名管道的权限。
    ///
    /// Windows 最高权限任务与普通权限 CLI/TUI 的令牌不同；显式授权同一用户 SID，
    /// 并把管道完整性标签设为中级，避免默认权限导致 `Access Denied`。
    /// `user_sid` 为空或非 Windows 平台时与 [`JsonlIpcServer::bind`] 等价。
    pub fn bind_with_user_sid(endpoint: &str, user_sid: Option<&str>) -> io::Result<Self> {
        let name = local_socket_name(endpoint)?;
        // 与 bind 同一口径：Windows 走命名管道不涉及文件，POSIX 先回收残留端点。
        #[cfg(unix)]
        stale_endpoint::reclaim(endpoint);
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
            fast_path: None,
        })
    }

    /// 挂上应答旁路：命中的请求不进主处理链的互斥。
    #[must_use]
    pub fn with_fast_path(mut self, fast_path: IpcFastPath) -> Self {
        self.fast_path = Some(fast_path);
        self
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
        let fast_path = self.fast_path.clone();
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
            let fast_path = fast_path.clone();
            thread::Builder::new()
                .name(CONNECTION_THREAD_NAME.to_owned())
                .spawn(move || {
                    let _active = active;
                    let _ = serve_streaming_connection(connection, &handler, &fast_path);
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

fn serve_streaming_connection<F>(
    connection: Stream,
    handler: &Arc<Mutex<F>>,
    fast_path: &Option<IpcFastPath>,
) -> io::Result<()>
where
    F: FnMut(DaemonRequest) -> IpcDispatch,
{
    let mut reader = BufReader::new(connection);
    let mut line = String::new();
    while reader.read_line(&mut line)? > 0 {
        let dispatch = parse_and_dispatch(line.trim_end(), handler, fast_path)?;
        write_dispatch(reader.get_mut(), dispatch)?;
        line.clear();
    }
    Ok(())
}

fn parse_and_dispatch<F>(
    line: &str,
    handler: &Arc<Mutex<F>>,
    fast_path: &Option<IpcFastPath>,
) -> io::Result<IpcDispatch>
where
    F: FnMut(DaemonRequest) -> IpcDispatch,
{
    let request = match serde_json::from_str::<DaemonRequest>(line) {
        Ok(request) => request,
        Err(error) => {
            return Ok(IpcDispatch::complete(vec![invalid_request_response(error)]));
        }
    };
    // 旁路先答：健康检查这类只读快照不该等主处理链上的长请求收尾。
    if let Some(dispatch) = fast_path.as_ref().and_then(|fast_path| fast_path(&request)) {
        return Ok(dispatch);
    }
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
#[path = "ipc_tests.rs"]
mod tests;
