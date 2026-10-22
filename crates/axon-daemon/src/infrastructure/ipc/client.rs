//! Daemon IPC 的 JSONL 客户端与订阅句柄。
//!
//! 客户端只用「阻塞写 + 有界轮询读」完成一次请求/响应往返，并为订阅连接维护
//! 有界推送通道；服务端的 accept、派发与响应写出留在父模块 `ipc`。

use axon_protocol::{DaemonRequest, DaemonResponse};
use interprocess::local_socket::{Stream, prelude::*};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use super::{CONNECTION_THREAD_NAME, local_socket_name};

const SUBSCRIPTION_CLIENT_QUEUE_LIMIT: usize = 64;
const SUBSCRIPTION_READER_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 订阅握手（写出 `SubscribeEvents` → 收到 `subscription_ready`）的上限。
///
/// Daemon 卡死时 `subscribe` 必须失败返回而不是永久阻塞：无界等待会把调用方
/// 永久钉在 Working 状态，且「连接已建立」并不能证明 Daemon 会回话。
const SUBSCRIPTION_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 订阅 reader 的单次等待上限。
///
/// 它同时决定「句柄释放后连接多久关闭」：用服务端心跳周期（1s）等待会让取消响应与
/// 服务端下一次写入叠加成秒级延迟，因此使用更短的独立粒度。
const SUBSCRIPTION_READER_WAIT: Duration = Duration::from_millis(250);

/// Windows `ERROR_PIPE_BUSY`：管道实例全部繁忙；管道尚未完全建立时也会出现。
#[cfg(windows)]
const WINDOWS_ERROR_PIPE_BUSY: i32 = 231;
/// Windows `ERROR_NO_DATA`：管道正在关闭。
#[cfg(windows)]
const WINDOWS_ERROR_NO_DATA: i32 = 232;
/// Windows `ERROR_PIPE_NOT_CONNECTED`：对端尚未完成连接握手。
#[cfg(windows)]
const WINDOWS_ERROR_PIPE_NOT_CONNECTED: i32 = 233;

fn is_reader_poll_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// 管道状态类瞬时错误：重试即可恢复，不代表连接已经失效。
#[cfg(windows)]
fn is_transient_pipe_state(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(WINDOWS_ERROR_PIPE_BUSY | WINDOWS_ERROR_NO_DATA | WINDOWS_ERROR_PIPE_NOT_CONNECTED)
    )
}

#[cfg(not(windows))]
const fn is_transient_pipe_state(_error: &io::Error) -> bool {
    false
}

fn ipc_timeout_error() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "Daemon IPC 请求超时")
}

/// 非阻塞读返回 0 字节时是否代表「暂无数据」而非对端关闭。
///
/// Windows 命名管道在 `PIPE_NOWAIT` 下用「0 字节读取成功」表达缓冲区为空，与真实 EOF
/// 在标准 `Read` 契约下无法区分（对端关闭走 `BrokenPipe`）；阻塞模式与 Unix domain
/// socket 都不会这样表达未就绪（后者用 `EAGAIN`），0 字节即对端关闭。
///
/// 该判定必须同时看平台与读模式：只看平台会让阻塞读把关闭误判成未就绪而永久等待，
/// 只看模式会让 Unix 非阻塞读把关闭误判成未就绪而空转。
#[cfg(windows)]
const fn platform_zero_byte_means_pending() -> bool {
    true
}

#[cfg(not(windows))]
const fn platform_zero_byte_means_pending() -> bool {
    false
}

/// 单次响应读取的字节上限。
const RESPONSE_READ_CHUNK_BYTES: usize = 8 * 1024;

/// JSONL 本地 IPC 客户端。
pub struct JsonlIpcClient {
    stream: Stream,
    /// 尚未构成完整响应行的字节。
    ///
    /// 必须自己累积而不用 `BufRead::read_line`：后者在内部把 `Ok(0)` 当作 EOF 并返回
    /// **半行**数据，而 Windows 非阻塞管道用 `Ok(0)` 表达「暂无数据」，于是半行 JSON 会被
    /// 当成完整响应解析（实测：`EOF while parsing a string at line 1 column 529`）。
    pending: Vec<u8>,
    /// 读端是否处于非阻塞模式；决定 0 字节读取的含义。
    nonblocking: bool,
}

/// 有界异步读取 Daemon 推送的客户端订阅句柄。
pub struct JsonlIpcSubscription {
    responses: mpsc::Receiver<DaemonResponse>,
    reader_cancel: Arc<AtomicBool>,
}

impl Drop for JsonlIpcSubscription {
    fn drop(&mut self) {
        self.reader_cancel.store(true, Ordering::Release);
    }
}

impl JsonlIpcSubscription {
    /// 阻塞等待下一条 Daemon 推送。
    pub fn recv(&mut self) -> io::Result<DaemonResponse> {
        self.responses
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "Daemon IPC 订阅连接已关闭"))
    }

    /// 在时限内等待下一条 Daemon 推送；`Ok(None)` 表示超时。
    pub fn recv_timeout(&mut self, timeout: Duration) -> io::Result<Option<DaemonResponse>> {
        self.responses.recv_timeout(timeout).map_or_else(
            |error| {
                if error == mpsc::RecvTimeoutError::Timeout {
                    Ok(None)
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Daemon IPC 订阅连接已关闭",
                    ))
                }
            },
            |response| Ok(Some(response)),
        )
    }
}

impl JsonlIpcClient {
    /// 连接到 namespaced endpoint。
    pub fn connect(endpoint: &str) -> io::Result<Self> {
        let name = local_socket_name(endpoint)?;
        Ok(Self {
            stream: Stream::connect(name)?,
            pending: Vec::new(),
            nonblocking: false,
        })
    }

    /// 写入一个请求并读取一条响应。
    pub fn send(&mut self, request: &DaemonRequest) -> io::Result<DaemonResponse> {
        self.write_request(request)?;
        self.receive()
    }

    /// 在时限内完成一次请求/响应往返。
    ///
    /// Daemon 卡死时调用方必须能失败返回而不是永久阻塞：读写都切到非阻塞轮询，
    /// 复用订阅连接的 `is_reader_poll_timeout` 看门狗语义，结束后恢复阻塞模式。
    ///
    /// 超时是「结果未知」的安全失败：调用方应按未确认重试（幂等由请求方保证），
    /// 并且**必须丢弃这条连接**：迟到的响应仍会到达，复用连接会把它误配给下一次请求。
    pub fn send_with_timeout(
        &mut self,
        request: &DaemonRequest,
        timeout: Duration,
    ) -> io::Result<DaemonResponse> {
        let deadline = Instant::now() + timeout;
        self.set_nonblocking_mode_until(true, deadline)?;
        let result = self
            .write_request_until(request, deadline)
            .and_then(|()| self.receive_until(deadline));
        // 恢复失败时保留 nonblocking 标记，让它与实际读模式保持一致。
        let _ = self.set_nonblocking_mode(false);
        result
    }

    /// 切换读端阻塞模式，并同步「0 字节读取」的语义判定依据。
    fn set_nonblocking_mode(&mut self, nonblocking: bool) -> io::Result<()> {
        self.stream.set_nonblocking(nonblocking)?;
        self.nonblocking = nonblocking;
        Ok(())
    }

    /// 在截止时间内重试管道状态类瞬时错误后再切换读模式。
    ///
    /// Windows 命名管道的 `PIPE_NOWAIT` 切换在管道尚未完全建立或处于繁忙状态时会瞬时失败
    /// （`ERROR_PIPE_BUSY` 等）。把它当成硬失败会让订阅在 Daemon 正常工作时随机失败，而
    /// 调用方本来就带截止时间：在截止时间内重试，超期仍失败则返回最后一次真实错误，
    /// 既不吞掉错误也不突破超时契约。
    fn set_nonblocking_mode_until(
        &mut self,
        nonblocking: bool,
        deadline: Instant,
    ) -> io::Result<()> {
        loop {
            match self.set_nonblocking_mode(nonblocking) {
                Ok(()) => return Ok(()),
                Err(error) if is_transient_pipe_state(&error) && Instant::now() < deadline => {
                    thread::sleep(SUBSCRIPTION_READER_POLL_INTERVAL);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// 当前读模式下 0 字节读取是否代表「暂无数据」。
    fn zero_byte_read_means_pending(&self) -> bool {
        self.nonblocking && platform_zero_byte_means_pending()
    }

    fn write_request_until(
        &mut self,
        request: &DaemonRequest,
        deadline: Instant,
    ) -> io::Result<()> {
        let mut payload = serde_json::to_vec(request)?;
        payload.push(b'\n');
        let mut written = 0usize;
        while written < payload.len() {
            if Instant::now() >= deadline {
                return Err(ipc_timeout_error());
            }
            match self.stream.write(&payload[written..]) {
                Ok(0) => {
                    // 非阻塞写入在缓冲区无空间时返回 0 字节：这是「暂时写不进去」，
                    // 不是请求被截断（对端关闭会以 `Err` 呈现，见 `read_step` 的说明）。
                    // 按截止时间重试，超期由循环开头的 deadline 检查统一报超时。
                    thread::sleep(SUBSCRIPTION_READER_POLL_INTERVAL);
                }
                Ok(count) => written += count,
                Err(error) if is_reader_poll_timeout(&error) => {
                    thread::sleep(SUBSCRIPTION_READER_POLL_INTERVAL);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn receive_until(&mut self, deadline: Instant) -> io::Result<DaemonResponse> {
        self.receive_until_deadline(deadline)?
            .ok_or_else(ipc_timeout_error)
    }

    /// 在时限内等待一条推送；`Ok(None)` 表示期限内没有数据。
    ///
    /// 推送消费者必须用它而不是无界 `receive()`：无界等待期间无法观察取消标志，
    /// 句柄释放后连接会迟迟不关闭。调用方未处于非阻塞模式时临时切换并在结束后恢复。
    fn receive_timeout(&mut self, timeout: Duration) -> io::Result<Option<DaemonResponse>> {
        let deadline = Instant::now() + timeout;
        let restore_blocking = !self.nonblocking;
        if restore_blocking {
            self.set_nonblocking_mode_until(true, deadline)?;
        }
        let result = self.receive_until_deadline(deadline);
        if restore_blocking {
            let _ = self.set_nonblocking_mode(false);
        }
        result
    }

    fn receive_until_deadline(&mut self, deadline: Instant) -> io::Result<Option<DaemonResponse>> {
        loop {
            if let Some(line) = self.take_complete_line() {
                return serde_json::from_slice(&line)
                    .map(Some)
                    .map_err(io::Error::other);
            }
            if Instant::now() >= deadline {
                self.pending.clear();
                return Ok(None);
            }
            match self.read_step() {
                Ok(true) => {}
                Ok(false) => thread::sleep(SUBSCRIPTION_READER_POLL_INTERVAL),
                Err(error) => {
                    self.pending.clear();
                    return Err(error);
                }
            }
        }
    }

    /// 读入一段可用字节并追加到待解析缓冲。
    ///
    /// `Ok(false)` 表示「暂无数据」（仅 Windows 非阻塞管道用 0 字节读取表达，见
    /// [`platform_zero_byte_means_pending`]）；对端关闭返回 `Err(UnexpectedEof)`。
    fn read_step(&mut self) -> io::Result<bool> {
        let mut scratch = [0_u8; RESPONSE_READ_CHUNK_BYTES];
        match self.stream.read(&mut scratch) {
            Ok(0) if self.zero_byte_read_means_pending() => Ok(false),
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Daemon IPC 订阅连接已关闭",
            )),
            Ok(count) => {
                self.pending.extend_from_slice(&scratch[..count]);
                Ok(true)
            }
            Err(error) if is_reader_poll_timeout(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// 取出缓冲中第一条完整的响应行（去掉行尾 `\n` 或 `\r\n`）；不足一行返回 `None`。
    fn take_complete_line(&mut self) -> Option<Vec<u8>> {
        let newline = self.pending.iter().position(|byte| *byte == b'\n')?;
        let mut line: Vec<u8> = self.pending.drain(..=newline).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(line)
    }

    /// 发送订阅请求并返回有界异步读取句柄；句柄释放会取消后台 reader 并关闭旧连接。
    pub fn subscribe(self, request: &DaemonRequest) -> io::Result<JsonlIpcSubscription> {
        self.subscribe_with_timeout(request, SUBSCRIPTION_HANDSHAKE_TIMEOUT)
    }

    /// 带握手上限的订阅：`timeout` 只约束 `subscription_ready` 的到达时间。
    pub(super) fn subscribe_with_timeout(
        self,
        request: &DaemonRequest,
        timeout: Duration,
    ) -> io::Result<JsonlIpcSubscription> {
        let mut client = self;
        let deadline = Instant::now() + timeout;
        client.write_request(request)?;
        let reader_cancel = Arc::new(AtomicBool::new(false));
        let reader_flag = Arc::clone(&reader_cancel);
        let (sender, responses) = mpsc::sync_channel(SUBSCRIPTION_CLIENT_QUEUE_LIMIT);
        // 握手期就切到非阻塞轮询：句柄最终本来就要保持非阻塞，多切一次读模式
        // 在 Windows 命名管道上会额外引入状态往返，没有收益。
        client.set_nonblocking_mode_until(true, deadline)?;
        let ready = client.receive_until_deadline(deadline)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "Daemon IPC 订阅握手超时：未收到 subscription_ready",
            )
        })?;
        sender
            .send(ready)
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        // 后台 reader 进入非阻塞轮询：必须同步标记，否则 0 字节读取会被误判成对端关闭。
        thread::Builder::new()
            .name(CONNECTION_THREAD_NAME.to_owned())
            .spawn(move || {
                while !reader_flag.load(Ordering::Acquire) {
                    // 有界等待：句柄释放后一个等待周期内即可退出并关闭连接。
                    match client.receive_timeout(SUBSCRIPTION_READER_WAIT) {
                        Ok(Some(response)) => {
                            if sender.send(response).is_err() {
                                break;
                            }
                        }
                        Ok(None) => continue,
                        Err(error) if is_reader_poll_timeout(&error) => continue,
                        Err(_) => break,
                    }
                }
            })
            .map_err(io::Error::other)?;
        Ok(JsonlIpcSubscription {
            responses,
            reader_cancel,
        })
    }

    fn write_request(&mut self, request: &DaemonRequest) -> io::Result<()> {
        serde_json::to_writer(&mut self.stream, request)?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()
    }

    fn receive(&mut self) -> io::Result<DaemonResponse> {
        loop {
            if let Some(line) = self.take_complete_line() {
                return serde_json::from_slice(&line).map_err(io::Error::other);
            }
            if let Err(error) = self.read_step() {
                self.pending.clear();
                return Err(error);
            }
        }
    }
}
