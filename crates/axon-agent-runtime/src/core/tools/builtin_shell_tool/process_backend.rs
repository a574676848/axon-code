//! 持久 Shell 会话的 pipe 与 PTY 进程后端。

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};
#[cfg(windows)]
use std::{sync::mpsc, time::Duration};

use axon_process::{
    configure_managed_process_group, decode_console_bytes,
    resource_fence::{
        ProcessFenceHandle, ProcessResourceLimits, configure_process_resource_limits,
        fence_pty_command, fence_spawned_child,
    },
    terminate_managed_process_tree, terminate_managed_process_tree_by_pid,
    try_decode_console_code_page,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use super::{
    SHELL_COMMAND_IO_ERROR, SHELL_COMMAND_OUTPUT_CHUNK_BYTES, ShellCommandOutputStream,
    output_buffer::SharedProcessOutput,
};

const PTY_ROWS: u16 = 24;
const PTY_COLS: u16 = 120;
const ANSI_CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
const ANSI_CURSOR_POSITION_RESPONSE: &[u8] = b"\x1b[1;1R";
#[cfg(windows)]
const PTY_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const PTY_STARTUP_TIMEOUT_ERROR: &str = "PTY 创建超时，当前 Windows ConPTY 不可用";
#[cfg(windows)]
const WINDOWS_EXTENDED_PATH_PREFIX: &str = r"\\?\";
#[cfg(windows)]
const WINDOWS_EXTENDED_UNC_PREFIX: &str = r"\\?\UNC\";
#[cfg(windows)]
const WINDOWS_UNC_PREFIX: &str = r"\\";

pub(crate) enum ProcessBackend {
    Pipe {
        child: Child,
        stdin: ChildStdin,
        /// 资源围栏句柄；必须与子进程同生命周期，否则 Windows Job Object 会被提前释放。
        _fence: Option<ProcessFenceHandle>,
    },
    Pty {
        child: Box<dyn portable_pty::Child + Send + Sync>,
        process_id: Option<u32>,
        writer: PtyWriter,
        _master: Box<dyn portable_pty::MasterPty + Send>,
    },
}

type PtyWriter = Arc<Mutex<Box<dyn Write + Send>>>;

impl ProcessBackend {
    pub(crate) fn spawn(
        command: &[String],
        cwd: &Path,
        tty: bool,
        output: Arc<SharedProcessOutput>,
        env_overrides: &[(String, String)],
    ) -> std::io::Result<Self> {
        if tty {
            spawn_pty(command, cwd, output, env_overrides)
        } else {
            spawn_pipe(command, cwd, output, env_overrides)
        }
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<i32>> {
        match self {
            Self::Pipe { child, .. } => child
                .try_wait()
                .map(|status| status.map(|status| status.code().unwrap_or(1))),
            Self::Pty { child, .. } => child
                .try_wait()
                .map(|status| status.map(|status| i32::try_from(status.exit_code()).unwrap_or(1))),
        }
    }

    pub(crate) fn write_stdin(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Pipe { stdin, .. } => stdin.write_all(bytes).and_then(|_| stdin.flush()),
            Self::Pty { writer, .. } => write_pty(writer, &terminal_input(bytes)),
        }
    }

    pub(crate) fn ctrl_c(&mut self) -> std::io::Result<bool> {
        match self {
            Self::Pipe { child, .. } => {
                terminate_managed_process_tree(child)?;
                Ok(true)
            }
            Self::Pty { writer, .. } => {
                write_pty(writer, &[3])?;
                Ok(false)
            }
        }
    }

    pub(crate) fn terminate(&mut self) -> std::io::Result<()> {
        match self {
            Self::Pipe { child, .. } => terminate_managed_process_tree(child),
            Self::Pty {
                child, process_id, ..
            } => {
                if let Some(process_id) = process_id
                    && let Err(error) = terminate_managed_process_tree_by_pid(*process_id)
                    && child.try_wait()?.is_none()
                {
                    return Err(error);
                }
                child.kill()
            }
        }
    }
}

fn spawn_pipe(
    command: &[String],
    cwd: &Path,
    output: Arc<SharedProcessOutput>,
    env_overrides: &[(String, String)],
) -> std::io::Result<ProcessBackend> {
    let (program, args) = command.split_first().ok_or_else(io_error)?;
    let mut process = Command::new(program);
    configure_managed_process_group(&mut process);
    // 围栏必须在 spawn 之前配置：Unix 走 pre_exec，Windows 走启动后的 Job Object 绑定。
    let limits = ProcessResourceLimits::resolved_default();
    configure_process_resource_limits(&mut process, limits);
    // 注入值覆盖继承环境，确保分页器与色彩输出在子进程内被禁用。
    process.envs(env_overrides.iter().map(|(key, value)| (key, value)));
    let mut child = process
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let fence = fence_spawned_child(&child, limits)?;
    let stdin = child.stdin.take().ok_or_else(io_error)?;
    let stdout = child.stdout.take().ok_or_else(io_error)?;
    let stderr = child.stderr.take().ok_or_else(io_error)?;
    spawn_reader(
        stdout,
        ShellCommandOutputStream::Stdout,
        Arc::clone(&output),
    );
    spawn_reader(stderr, ShellCommandOutputStream::Stderr, output);
    Ok(ProcessBackend::Pipe {
        child,
        stdin,
        _fence: fence,
    })
}

fn spawn_pty(
    command: &[String],
    cwd: &Path,
    output: Arc<SharedProcessOutput>,
    env_overrides: &[(String, String)],
) -> std::io::Result<ProcessBackend> {
    let pair = open_pty(PtySize {
        rows: PTY_ROWS,
        cols: PTY_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let portable_pty::PtyPair { master, slave } = pair;
    // 交互式会话的限额独立于一次性命令：`portable_pty` 不暴露 pre_exec 钩子，
    // 因此改用 `prlimit` 包装命令；系统无该程序时保持原命令、不做包装。
    if command.is_empty() {
        return Err(io_error());
    }
    let mut session_command = command.to_vec();
    fence_pty_command(
        &mut session_command,
        ProcessResourceLimits::interactive_default(),
    );
    let (program, args) = session_command.split_first().ok_or_else(io_error)?;
    let mut builder = CommandBuilder::new(program);
    builder.args(args);
    builder.cwd(pty_working_directory(cwd));
    for (key, value) in env_overrides {
        builder.env(key, value);
    }
    let child = slave.spawn_command(builder).map_err(|_| io_error())?;
    let process_id = child.process_id();
    let reader = master.try_clone_reader().map_err(|_| io_error())?;
    let writer = Arc::new(Mutex::new(master.take_writer().map_err(|_| io_error())?));
    drop(slave);
    spawn_pty_reader(reader, output, Arc::clone(&writer));
    Ok(ProcessBackend::Pty {
        child,
        process_id,
        writer,
        _master: master,
    })
}

#[cfg(windows)]
fn open_pty(size: PtySize) -> std::io::Result<portable_pty::PtyPair> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(native_pty_system().openpty(size));
    });
    match receiver.recv_timeout(PTY_STARTUP_TIMEOUT) {
        Ok(Ok(pair)) => Ok(pair),
        Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => Err(io_error()),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            PTY_STARTUP_TIMEOUT_ERROR,
        )),
    }
}

#[cfg(not(windows))]
fn open_pty(size: PtySize) -> std::io::Result<portable_pty::PtyPair> {
    native_pty_system().openpty(size).map_err(|_| io_error())
}

#[cfg(windows)]
fn pty_working_directory(cwd: &Path) -> PathBuf {
    let value = cwd.to_string_lossy();
    if let Some(path) = value.strip_prefix(WINDOWS_EXTENDED_UNC_PREFIX) {
        return PathBuf::from(format!("{WINDOWS_UNC_PREFIX}{path}"));
    }
    value
        .strip_prefix(WINDOWS_EXTENDED_PATH_PREFIX)
        .map_or_else(|| cwd.to_path_buf(), PathBuf::from)
}

#[cfg(not(windows))]
fn pty_working_directory(cwd: &Path) -> PathBuf {
    cwd.to_path_buf()
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    stream: ShellCommandOutputStream,
    output: Arc<SharedProcessOutput>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; SHELL_COMMAND_OUTPUT_CHUNK_BYTES];
        let mut decoder = Utf8StreamDecoder::default();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    decoder.finish(|bytes| output.append(stream, bytes));
                    break;
                }
                Ok(read) => decoder.push(&buffer[..read], |bytes| output.append(stream, bytes)),
            }
        }
        output.notify_change();
    });
}

fn spawn_pty_reader(
    mut reader: impl Read + Send + 'static,
    output: Arc<SharedProcessOutput>,
    writer: PtyWriter,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; SHELL_COMMAND_OUTPUT_CHUNK_BYTES];
        let mut matched_query_bytes = 0;
        let mut decoder = Utf8StreamDecoder::default();
        let mut sanitizer = TerminalOutputSanitizer::default();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    decoder.finish(|bytes| {
                        let display = sanitizer.push(bytes);
                        output.append_terminal(ShellCommandOutputStream::Stdout, bytes, &display);
                    });
                    break;
                }
                Ok(read) => {
                    for byte in &buffer[..read] {
                        if *byte == ANSI_CURSOR_POSITION_QUERY[matched_query_bytes] {
                            matched_query_bytes += 1;
                            if matched_query_bytes == ANSI_CURSOR_POSITION_QUERY.len() {
                                let _ = write_pty(&writer, ANSI_CURSOR_POSITION_RESPONSE);
                                matched_query_bytes = 0;
                            }
                        } else {
                            matched_query_bytes =
                                usize::from(*byte == ANSI_CURSOR_POSITION_QUERY[0]);
                        }
                    }
                    decoder.push(&buffer[..read], |bytes| {
                        let display = sanitizer.push(bytes);
                        output.append_terminal(ShellCommandOutputStream::Stdout, bytes, &display);
                    });
                }
            }
        }
        output.notify_change();
    });
}

/// Preserve Unix terminal input bytes; normalize model line endings for Windows ConPTY.
#[cfg(not(windows))]
fn terminal_input(bytes: &[u8]) -> Vec<u8> {
    bytes.to_vec()
}

#[cfg(windows)]
fn terminal_input(bytes: &[u8]) -> Vec<u8> {
    let mut terminal_bytes = Vec::with_capacity(bytes.len());
    let mut previous_was_cr = false;
    for byte in bytes {
        match byte {
            b'\n' if previous_was_cr => {}
            b'\n' => terminal_bytes.push(b'\r'),
            _ => terminal_bytes.push(*byte),
        }
        previous_was_cr = *byte == b'\r';
    }
    terminal_bytes
}

#[derive(Default)]
struct TerminalOutputSanitizer {
    state: TerminalOutputState,
}

#[derive(Default)]
enum TerminalOutputState {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl TerminalOutputSanitizer {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut display = Vec::with_capacity(bytes.len());
        for byte in bytes {
            match self.state {
                TerminalOutputState::Text => match byte {
                    0x1b => self.state = TerminalOutputState::Escape,
                    b'\n' | b'\r' | b'\t' | 0x20..=0x7e | 0x80..=0xff => display.push(*byte),
                    _ => {}
                },
                TerminalOutputState::Escape => {
                    self.state = match byte {
                        b'[' => TerminalOutputState::Csi,
                        b']' => TerminalOutputState::Osc,
                        _ => TerminalOutputState::Text,
                    };
                }
                TerminalOutputState::Csi => {
                    if (0x40..=0x7e).contains(byte) {
                        self.state = TerminalOutputState::Text;
                    }
                }
                TerminalOutputState::Osc => match byte {
                    0x07 => self.state = TerminalOutputState::Text,
                    0x1b => self.state = TerminalOutputState::OscEscape,
                    _ => {}
                },
                TerminalOutputState::OscEscape => {
                    self.state = if *byte == b'\\' {
                        TerminalOutputState::Text
                    } else if *byte == 0x1b {
                        TerminalOutputState::OscEscape
                    } else {
                        TerminalOutputState::Osc
                    };
                }
            }
        }
        display
    }
}

/// 子进程输出解码器：**整条流**判定编码：优先 UTF-8，否则按平台控制台代码页解码。
///
/// 为什么不能逐段回退：GBK/CP932 这类双字节代码页里存在**恰好是合法 UTF-8** 的字节对
/// （实测：`CA B9` 是 GBK 的「使」，同时是合法的 UTF-8 `ʹ`）。逐段回退会把这些字节对
/// 当 UTF-8 吐出去，并把它后面的字节对**切断**，于是同一条 GBK 消息出现「前半段正确、
/// 尾部乱码」（R2 验证 §5.4 实测 `“<”运算符保留供将来ʹ�á�`）。
///
/// 因此改为：一旦出现**确定非法**的 UTF-8 字节（`error_len()` 为 `Some`），就把整条流
/// 切换为控制台代码页解码，此后不再按 UTF-8 切分。ASCII 前缀在两套编码下一致，故切换点
/// 之前的输出不受影响；`error_len().is_none()`（尾部不完整）时继续等待后续字节。
#[derive(Default)]
struct Utf8StreamDecoder {
    pending: Vec<u8>,
    /// 已判定整条流不是 UTF-8；此后所有字节按控制台代码页解码。
    legacy: bool,
    /// 跨块残留的半个多字节字符（代码页模式）；避免把双字节字符切在两块之间。
    legacy_carry: Vec<u8>,
}

impl Utf8StreamDecoder {
    fn push(&mut self, bytes: &[u8], mut emit: impl FnMut(&[u8])) {
        if self.legacy {
            self.push_legacy(bytes, &mut emit);
            return;
        }
        self.pending.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(_) => {
                    if !self.pending.is_empty() {
                        emit(&self.pending);
                        self.pending.clear();
                    }
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        // 先吐已确认合法的前缀：ASCII 在两套编码下逐字节一致。
                        emit(&self.pending[..valid_up_to]);
                        self.pending.drain(..valid_up_to);
                        continue;
                    }
                    if error.error_len().is_none() {
                        // 尾部是被截断的多字节序列，等下一块再判。
                        break;
                    }
                    // 首个确定非法的字节 ⇒ 整条流按控制台代码页解码；走同一条路径，
                    // 以便把尾部可能存在的半个多字节字符留到下一块。
                    self.legacy = true;
                    let pending = std::mem::take(&mut self.pending);
                    self.push_legacy(&pending, &mut emit);
                    break;
                }
            }
        }
    }

    fn finish(&mut self, mut emit: impl FnMut(&[u8])) {
        if self.legacy {
            let carry = std::mem::take(&mut self.legacy_carry);
            if !carry.is_empty() {
                emit(decode_console_bytes(&carry).as_bytes());
            }
            return;
        }
        if self.pending.is_empty() {
            return;
        }
        // 尾部残留：截断的多字节序列或未闭合的代码页序列，走同一条回退解码。
        let pending = std::mem::take(&mut self.pending);
        emit(decode_console_bytes(&pending).as_bytes());
    }

    /// 代码页模式：整块解码，末尾若为半个多字节字符则留到下一块。
    ///
    /// 必须用 `try_decode_console_code_page`（**只**走代码页）而不是会先试 UTF-8 的
    /// `try_decode_console_bytes`：代码页里存在恰好是合法 UTF-8 的字节对（`CA B9` 既是
    /// GBK 的「使」也是 UTF-8 的 `ʹ`），再试一次 UTF-8 就等于把已判定过的流重新切开。
    ///
    /// 从尾部逐步回退可找出「能完整解码的最长前缀」，比「末字节 ≥ 0x80 就按住」的字节
    /// 启发式精确：后继字节也可能落在 0x40..=0x7E，靠取值范围猜不出来。
    fn push_legacy(&mut self, bytes: &[u8], emit: &mut impl FnMut(&[u8])) {
        let mut chunk = std::mem::take(&mut self.legacy_carry);
        chunk.extend_from_slice(bytes);
        let mut split = chunk.len();
        loop {
            if let Some(text) = try_decode_console_code_page(&chunk[..split]) {
                emit(text.as_bytes());
                self.legacy_carry = chunk[split..].to_vec();
                return;
            }
            if split == 0 {
                // 整块都无法无损解码：有损输出，避免字节无限滞留。
                emit(decode_console_bytes(&chunk).as_bytes());
                self.legacy_carry.clear();
                return;
            }
            split -= 1;
        }
    }
}

fn write_pty(writer: &PtyWriter, bytes: &[u8]) -> std::io::Result<()> {
    let mut writer = writer.lock().map_err(|_| io_error())?;
    writer.write_all(bytes).and_then(|_| writer.flush())
}

fn io_error() -> std::io::Error {
    std::io::Error::other(SHELL_COMMAND_IO_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_decoder_preserves_characters_split_across_chunks() {
        let source = "Axon 中文输出".as_bytes();
        let mut decoder = Utf8StreamDecoder::default();
        let mut decoded = Vec::new();

        decoder.push(&source[..7], |bytes| decoded.extend_from_slice(bytes));
        decoder.push(&source[7..9], |bytes| decoded.extend_from_slice(bytes));
        decoder.push(&source[9..], |bytes| decoded.extend_from_slice(bytes));
        decoder.finish(|bytes| decoded.extend_from_slice(bytes));

        assert_eq!(String::from_utf8(decoded).unwrap(), "Axon 中文输出");
    }

    /// 整条流必须**一次**决定编码：GBK 里存在恰好是合法 UTF-8 的字节对
    /// （`CA B9` 既是 GBK 的「使」，也是合法 UTF-8 的 `ʹ`）。逐段回退会吐出该字节对
    /// 并切断其后的字节对，导致同一条消息「前半段正确、尾部乱码」：这正是修复前
    /// 实测到的 `“<”运算符保留供将来ʹ�á�`。
    ///
    /// 断言与环境无关：解码结果必须等于「整段按控制台代码页解码」的结果。
    #[test]
    fn legacy_stream_is_decoded_as_one_unit_instead_of_per_segment() {
        // `A1 B0` = GBK 的左双引号；`CA B9` = 「使」（同时是合法 UTF-8）。
        let payload = [0xA1_u8, 0xB0, 0xCA, 0xB9];
        let mut decoder = Utf8StreamDecoder::default();
        let mut decoded = Vec::new();

        decoder.push(&payload, |bytes| decoded.extend_from_slice(bytes));
        decoder.finish(|bytes| decoded.extend_from_slice(bytes));

        assert_eq!(
            String::from_utf8(decoded).expect("解码结果必须是合法 UTF-8"),
            decode_console_bytes(&payload),
            "非 UTF-8 流必须整体按代码页解码，不得逐段混用"
        );
    }

    /// 代码页模式下，跨块被切开的双字节字符必须拼回来（不能留下半个字符）。
    #[test]
    fn legacy_stream_keeps_pairs_split_across_chunks() {
        let payload = [0xA1_u8, 0xB0, 0xCA, 0xB9];
        let mut decoder = Utf8StreamDecoder::default();
        let mut decoded = Vec::new();

        // 三字节处切开：末字节正好落在「使」的首字节上。
        decoder.push(&payload[..3], |bytes| decoded.extend_from_slice(bytes));
        decoder.push(&payload[3..], |bytes| decoded.extend_from_slice(bytes));
        decoder.finish(|bytes| decoded.extend_from_slice(bytes));

        assert_eq!(
            String::from_utf8(decoded).expect("解码结果必须是合法 UTF-8"),
            decode_console_bytes(&payload)
        );
    }

    /// 尾部残缺字节在代码页模式下也应被判为「不完整」，留到下一块再解。
    #[test]
    fn legacy_stream_holds_back_incomplete_tail_until_next_chunk() {
        assert!(
            try_decode_console_code_page(&[0x81]).is_none(),
            "单个首字节必须被判为不完整，否则流式解码会切坏字符"
        );
    }

    #[test]
    fn utf8_decoder_keeps_surrounding_text_when_bytes_are_not_utf8() {
        // 非法字节按控制台代码页回退解码（结果依系统代码页而定），因此这里只断言
        // 「两侧合法文本不被破坏、字节不被丢弃」这两条与环境无关的不变量。
        let mut decoder = Utf8StreamDecoder::default();
        let mut decoded = Vec::new();

        decoder.push(b"before\xFFafter", |bytes| decoded.extend_from_slice(bytes));
        decoder.finish(|bytes| decoded.extend_from_slice(bytes));

        let text = String::from_utf8(decoded).expect("回退解码结果必须是合法 UTF-8");
        assert!(
            text.starts_with("before") && text.ends_with("after"),
            "非法字节段两侧的合法文本必须保留：{text}"
        );
        assert!(
            text.chars().count() > "beforeafter".len(),
            "非法字节不得被静默丢弃：{text}"
        );
    }

    #[test]
    #[cfg(windows)]
    fn terminal_input_turns_model_newlines_into_enter_keys() {
        assert_eq!(
            terminal_input(b"first\nsecond\r\nthird\n"),
            b"first\rsecond\rthird\r"
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn terminal_input_preserves_unix_newlines() {
        assert_eq!(
            terminal_input(b"first\nsecond\r\nthird\n"),
            b"first\nsecond\r\nthird\n"
        );
    }

    #[test]
    fn terminal_output_sanitizer_removes_split_control_sequences() {
        let mut sanitizer = TerminalOutputSanitizer::default();
        let mut display = sanitizer.push(b"\x1b[31mfirst\x1b[");
        display.extend(sanitizer.push(b"0m\n\x1b]0;title\x1b\\second"));

        assert_eq!(String::from_utf8(display).unwrap(), "first\nsecond");
    }
}
