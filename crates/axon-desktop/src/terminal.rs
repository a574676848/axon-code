//! 会话工作区内的交互式终端。
//!
//! 一个 shell 进程一个会话：开一个终端就起一个真实 PTY（Windows 走 ConPTY，Unix 走 pty），
//! 工作目录钉在会话工作区根，输入按 UTF-8 字节写回，尺寸变化原样转发。
//! 输出由每个会话自己的读取线程推给界面：后端不做终端仿真，屏幕状态归界面，
//! 这里只负责字节搬运与生命周期，因此不引入第二份「屏幕应该长什么样」的事实。
//!
//! 会话在工作区根目录内启动：工作区不存在时直接拒绝，而不是退回某个默认目录，
//! 否则用户会在一个自己没选过的目录里敲命令。
//!
//! 进程退出即回收：PTY 主端句柄随进程关闭，Windows 的 ConPTY 与 Unix 的会话首进程
//! 都会随之结束，因此不需要额外的退出钩子。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::Emitter;

/// 终端输出的事件通道名，与界面 `ipc.ts` 里的常量一致。
pub const TERMINAL_OUTPUT_EVENT: &str = "axon://terminal-output";

/// 终端退出的事件通道名。
pub const TERMINAL_EXIT_EVENT: &str = "axon://terminal-exit";

/// 默认列数；界面挂载后会立刻按实际尺寸再调一次。
const DEFAULT_COLS: u16 = 100;

/// 默认行数。
const DEFAULT_ROWS: u16 = 30;

/// 单帧推给界面的最大字节数：超过就分帧，避免一帧把 IPC 撑爆。
const MAX_CHUNK_BYTES: usize = 8 * 1024;

/// 读取线程的缓冲区大小。
const READ_BUFFER_BYTES: usize = 8 * 1024;

/// 终端尺寸的上下限：界面量错时不至于把 PTY 设成 0 列或上万行。
const MIN_DIMENSION: u16 = 1;
const MAX_DIMENSION: u16 = 500;

/// 一个可选的 shell。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShellChoice {
    /// 启动用的程序：绝对路径，或 PATH 上的命令名。
    pub path: String,
    /// 展示名。
    pub name: String,
}

/// 已建立的终端会话的对外描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalInfo {
    /// 会话标识，后续写入、改尺寸、关闭都用它。
    pub id: String,
    /// 实际启动的 shell。
    pub shell: String,
    /// 启动时的工作目录。
    pub cwd: String,
    /// 当前列数。
    pub cols: u16,
    /// 当前行数。
    pub rows: u16,
}

/// 一帧终端输出。
#[derive(Debug, Clone, Serialize)]
struct OutputFrame {
    id: String,
    /// 原始字节的 base64：终端输出不保证是合法 UTF-8，转成字符串会丢帧。
    data: String,
}

/// 一次终端退出。
#[derive(Debug, Clone, Serialize)]
struct ExitFrame {
    id: String,
    /// 退出码；拿不到时为 `None`，界面据此显示「已退出」。
    code: Option<u32>,
    /// 会话是否由用户主动关闭。
    closed: bool,
}

/// 终端输出的出口。
///
/// 会话核心只认这个端口，不认事件通道：生产实现把字节推给界面，
/// 验证实现直接收在内存里，因此「起 shell、写命令、读输出、收退出」这条链路
/// 不必先起一个应用实例就能整条跑通。
pub trait TerminalSink: Send + Sync + 'static {
    /// 一段原始输出。
    fn output(&self, id: &str, bytes: &[u8]);
    /// 一次退出；`code` 拿不到时为 `None`，`closed` 表示是用户主动关的。
    fn exited(&self, id: &str, code: Option<u32>, closed: bool);
}

/// 生产出口：把字节编码成 base64 后经 Tauri 事件推给界面。
struct EventSink(tauri::AppHandle);

impl TerminalSink for EventSink {
    fn output(&self, id: &str, bytes: &[u8]) {
        for chunk in bytes.chunks(MAX_CHUNK_BYTES) {
            let _ = self.0.emit(
                TERMINAL_OUTPUT_EVENT,
                OutputFrame {
                    id: id.to_string(),
                    data: base64::engine::general_purpose::STANDARD.encode(chunk),
                },
            );
        }
    }

    fn exited(&self, id: &str, code: Option<u32>, closed: bool) {
        let _ = self.0.emit(
            TERMINAL_EXIT_EVENT,
            ExitFrame {
                id: id.to_string(),
                code,
                closed,
            },
        );
    }
}

/// 一个活着的终端会话。
struct Session {
    info: TerminalInfo,
    /// 改尺寸的入口；`MasterPty::resize` 只借用，因此这里不必可变。
    master: Box<dyn MasterPty + Send>,
    /// 键盘输入写入口。
    writer: Box<dyn Write + Send>,
    /// 子进程句柄，关闭时用来杀进程与取退出码。
    child: Box<dyn Child + Send + Sync>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

/// 进程内全部终端会话。
///
/// 注册表本身是可共享的（读取线程退出时要回来摘掉自己那一格），因此由 `Arc` 持有，
/// 作为 Tauri 的托管状态挂在应用上。
#[derive(Debug, Default)]
pub struct TerminalRegistry {
    sessions: Mutex<HashMap<String, Session>>,
    sequence: AtomicU64,
}

impl TerminalRegistry {
    /// 会话数量；用于诊断与测试。
    pub fn len(&self) -> usize {
        self.sessions.lock().map(|held| held.len()).unwrap_or(0)
    }

    /// 是否一个会话都没有。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 起一个终端；返回它的描述。
    fn spawn(
        self: &Arc<Self>,
        sink: Arc<dyn TerminalSink>,
        root: &str,
        shell: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalInfo, String> {
        // 工作目录必须是真实存在的工作区根：这一步同时把路径边界收在既有实现里。
        let cwd = crate::workspace_files::resolve_within(root, "")?;
        let shell = match shell {
            Some(requested) => requested.to_string(),
            None => default_shell().ok_or_else(|| "本机没有找到可用的 shell".to_string())?,
        };
        let cols = clamp_dimension(cols);
        let rows = clamp_dimension(rows);

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("无法创建终端：{error}"))?;

        let mut command = CommandBuilder::new(&shell);
        command.cwd(&cwd);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("无法启动 {shell}：{error}"))?;
        // 从端句柄必须在这里就放掉：留着它，主端读不到子进程退出后的 EOF。
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("无法读取终端输出：{error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("无法写入终端：{error}"))?;

        let id = format!("term-{}", self.sequence.fetch_add(1, Ordering::Relaxed) + 1);
        let info = TerminalInfo {
            id: id.clone(),
            shell,
            cwd: cwd.to_string_lossy().to_string(),
            cols,
            rows,
        };
        let session = Session {
            info: info.clone(),
            master: pair.master,
            writer,
            child,
        };
        let mut held = self
            .sessions
            .lock()
            .map_err(|_| "终端注册表不可用".to_string())?;
        held.insert(id.clone(), session);
        drop(held);

        spawn_reader(Arc::clone(self), sink, id, reader);
        Ok(info)
    }

    /// 把键盘输入写进终端。
    fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let mut held = self
            .sessions
            .lock()
            .map_err(|_| "终端注册表不可用".to_string())?;
        let session = held
            .get_mut(id)
            .ok_or_else(|| "这个终端已经不存在了".to_string())?;
        session
            .writer
            .write_all(data.as_bytes())
            .and_then(|()| session.writer.flush())
            .map_err(|error| format!("写入终端失败：{error}"))
    }

    /// 把新的字符尺寸转给 PTY，shell 里的全屏程序据此重排。
    fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let held = self
            .sessions
            .lock()
            .map_err(|_| "终端注册表不可用".to_string())?;
        let session = held
            .get(id)
            .ok_or_else(|| "这个终端已经不存在了".to_string())?;
        session
            .master
            .resize(PtySize {
                rows: clamp_dimension(rows),
                cols: clamp_dimension(cols),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| format!("调整终端尺寸失败：{error}"))
    }

    /// 关掉一个终端：先杀进程，再摘掉这一格。
    ///
    /// 读取线程随后会读到 EOF，发现这一格已经不在了，就不再重复广播退出。
    fn close(&self, id: &str) -> Result<(), String> {
        let mut held = self
            .sessions
            .lock()
            .map_err(|_| "终端注册表不可用".to_string())?;
        let Some(mut session) = held.remove(id) else {
            // 已经关掉的终端再关一次是空操作：界面重连或重复点击不该报错。
            return Ok(());
        };
        drop(held);
        let _ = session.child.kill();
        Ok(())
    }
}

/// 读取线程：把 PTY 的输出搬到出口，读到 EOF 时回收这一格并广播退出。
fn spawn_reader(
    registry: Arc<TerminalRegistry>,
    sink: Arc<dyn TerminalSink>,
    id: String,
    mut reader: Box<dyn Read + Send>,
) {
    std::thread::spawn(move || {
        let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => sink.output(&id, &buffer[..read]),
                Err(_) => break,
            }
        }
        // 会话还在说明是进程自己结束的；已经被 `close` 摘掉说明是用户关的。
        let reaped = registry
            .sessions
            .lock()
            .ok()
            .and_then(|mut held| held.remove(&id));
        let (code, closed) = match reaped {
            Some(mut session) => (
                session.child.wait().ok().map(|status| status.exit_code()),
                false,
            ),
            None => (None, true),
        };
        sink.exited(&id, code, closed);
    });
}

/// 把界面量出来的尺寸夹进可用区间。
fn clamp_dimension(value: u16) -> u16 {
    value.clamp(MIN_DIMENSION, MAX_DIMENSION)
}

/// 本机可用的 shell，按推荐顺序；一个都没有时返回空表。
pub fn shells() -> Vec<ShellChoice> {
    let mut choices = Vec::new();
    for (name, candidates) in shell_catalog() {
        if let Some(path) = candidates.into_iter().find(|path| shell_exists(path)) {
            choices.push(ShellChoice {
                path,
                name: name.to_string(),
            });
        }
    }
    choices
}

/// 默认 shell：候选表里的第一个。
fn default_shell() -> Option<String> {
    shells().into_iter().next().map(|choice| choice.path)
}

/// 候选 shell：展示名与按顺序尝试的定位。
fn shell_catalog() -> Vec<(&'static str, Vec<String>)> {
    if cfg!(target_os = "windows") {
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        vec![
            ("PowerShell 7", vec!["pwsh.exe".to_string()]),
            (
                "Windows PowerShell",
                vec![format!(
                    r"{system_root}\System32\WindowsPowerShell\v1.0\powershell.exe"
                )],
            ),
            (
                "命令提示符",
                vec![format!(r"{system_root}\System32\cmd.exe")],
            ),
        ]
    } else {
        let mut shells = Vec::new();
        if let Ok(shell) = std::env::var("SHELL")
            && !shell.trim().is_empty()
        {
            shells.push(shell);
        }
        vec![
            ("登录 shell", shells),
            (
                "Bash",
                vec!["/bin/bash".to_string(), "/usr/bin/bash".to_string()],
            ),
            (
                "Zsh",
                vec!["/bin/zsh".to_string(), "/usr/bin/zsh".to_string()],
            ),
            (
                "Shell",
                vec!["/bin/sh".to_string(), "/usr/bin/sh".to_string()],
            ),
        ]
    }
}

/// 一个 shell 候选是否真的可用。
fn shell_exists(candidate: &str) -> bool {
    let path = std::path::Path::new(candidate);
    if path.is_absolute() {
        return path.is_file();
    }
    // 裸命令名按 PATH 解析：只认真的存在且可执行的那一个。
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|directory| {
        let direct = directory.join(candidate);
        if direct.is_file() {
            return true;
        }
        cfg!(target_os = "windows")
            && !candidate.to_ascii_lowercase().ends_with(".exe")
            && directory.join(format!("{candidate}.exe")).is_file()
    })
}

/// Tauri IPC 命令：列出本机可用的 shell。
#[tauri::command]
pub fn terminal_shells() -> Vec<ShellChoice> {
    shells()
}

/// Tauri IPC 命令：在工作区里开一个终端。
#[tauri::command]
pub fn terminal_open(
    app: tauri::AppHandle,
    registry: tauri::State<'_, Arc<TerminalRegistry>>,
    root: String,
    shell: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> Result<TerminalInfo, String> {
    registry.spawn(
        Arc::new(EventSink(app)),
        &root,
        shell.as_deref(),
        cols.unwrap_or(DEFAULT_COLS),
        rows.unwrap_or(DEFAULT_ROWS),
    )
}

/// Tauri IPC 命令：把键盘输入写进终端。
#[tauri::command]
pub fn terminal_write(
    registry: tauri::State<'_, Arc<TerminalRegistry>>,
    id: String,
    data: String,
) -> Result<(), String> {
    registry.write(&id, &data)
}

/// Tauri IPC 命令：调整终端尺寸。
#[tauri::command]
pub fn terminal_resize(
    registry: tauri::State<'_, Arc<TerminalRegistry>>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    registry.resize(&id, cols, rows)
}

/// Tauri IPC 命令：关闭终端。
#[tauri::command]
pub fn terminal_close(
    registry: tauri::State<'_, Arc<TerminalRegistry>>,
    id: String,
) -> Result<(), String> {
    registry.close(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证用的出口：把输出与退出收在内存里，供用例轮询。
    #[derive(Debug, Default)]
    struct RecordingSink {
        output: Mutex<Vec<u8>>,
        exits: Mutex<Vec<(Option<u32>, bool)>>,
    }

    impl RecordingSink {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.output.lock().expect("输出缓冲")).to_string()
        }

        fn exits(&self) -> Vec<(Option<u32>, bool)> {
            self.exits.lock().expect("退出记录").clone()
        }
    }

    impl TerminalSink for RecordingSink {
        fn output(&self, _id: &str, bytes: &[u8]) {
            self.output
                .lock()
                .expect("输出缓冲")
                .extend_from_slice(bytes);
        }

        fn exited(&self, _id: &str, code: Option<u32>, closed: bool) {
            self.exits.lock().expect("退出记录").push((code, closed));
        }
    }

    /// 真正的终端会回答光标位置查询（DSR，`ESC[6n`），行编辑器拿到答复才继续。
    ///
    /// 用例里没有终端仿真器，因此这里替它作答：应答的是**协议规定的那一个**，
    /// 不是绕过 shell 的旁路：不答，PowerShell 的 PSReadLine 会一直等下去。
    fn answer_cursor_queries(
        registry: &TerminalRegistry,
        id: &str,
        sink: &RecordingSink,
        answered: &mut usize,
    ) {
        let queries = sink.text().matches("[6n").count();
        while *answered < queries {
            let _ = registry.write(id, "\u{1b}[1;1R");
            *answered += 1;
        }
    }

    /// 用例用的临时工作区；由 `TestTempDir` 的守卫负责清理。
    ///
    /// 用仓内共享的唯一目录而不是自拼时间戳：并行用例撞上同一个时间戳时，
    /// 两个用例会共用同一棵树而互相删掉对方的工作区。
    fn scratch_workspace() -> axon_test_support::TestTempDir {
        axon_test_support::test_temp_dir()
    }

    #[test]
    fn dimensions_are_clamped_into_a_usable_range() {
        assert_eq!(clamp_dimension(0), MIN_DIMENSION);
        assert_eq!(clamp_dimension(u16::MAX), MAX_DIMENSION);
        assert_eq!(clamp_dimension(120), 120);
    }

    #[test]
    fn the_shell_catalog_is_ordered_and_non_empty() {
        let catalog = shell_catalog();
        assert!(!catalog.is_empty(), "必须至少声明一个 shell 候选");
        for (name, candidates) in &catalog {
            assert!(!name.is_empty(), "候选 shell 必须带展示名");
            assert!(!candidates.is_empty(), "{name} 必须给出定位方式");
        }
    }

    #[test]
    fn every_shell_offered_really_exists_on_this_machine() {
        // 菜单里出现的每一条都必须能真的启动，否则就是点不动的条目。
        for choice in shells() {
            assert!(!choice.name.is_empty(), "候选 shell 必须带展示名");
            assert!(
                shell_exists(&choice.path),
                "列出的 {} 必须在本机存在",
                choice.path
            );
        }
    }

    #[test]
    fn a_missing_workspace_is_rejected_instead_of_falling_back_to_a_default_directory() {
        let registry = Arc::new(TerminalRegistry::default());
        assert!(registry.is_empty(), "初始注册表必须是空的");
        let missing = std::env::temp_dir().join("axon-terminal-no-such-workspace");
        // 没有应用句柄也不该走到落点：工作区校验在最前面。
        let root = missing.to_string_lossy().to_string();
        assert!(
            crate::workspace_files::resolve_within(&root, "").is_err(),
            "不存在的工作区必须在起进程之前被拒绝"
        );
    }

    #[test]
    fn writing_to_an_unknown_terminal_reports_instead_of_panicking() {
        let registry = TerminalRegistry::default();
        assert!(registry.write("term-none", "x").is_err());
        assert!(registry.resize("term-none", 80, 24).is_err());
        // 关闭不存在的终端是空操作：界面重复点击不该看到报错。
        assert!(registry.close("term-none").is_ok());
    }

    #[test]
    #[ignore = "需要真实的 shell 与 PTY；用 --ignored --nocapture 显式运行"]
    fn smoke_runs_a_command_in_a_real_terminal_and_reports_its_exit() {
        let root = scratch_workspace();
        let root = root.path().to_path_buf();
        let registry = Arc::new(TerminalRegistry::default());
        let sink = Arc::new(RecordingSink::default());
        let displayed = root.to_string_lossy().to_string();

        let info = registry
            .spawn(
                Arc::clone(&sink) as Arc<dyn TerminalSink>,
                &displayed,
                None,
                100,
                30,
            )
            .expect("必须能起一个真实终端");
        assert_eq!(registry.len(), 1, "会话必须登记在注册表里");
        assert!(!info.shell.is_empty(), "必须如实报告实际启动的 shell");
        let name = root
            .file_name()
            .expect("工作区名字")
            .to_string_lossy()
            .to_string();
        assert!(
            info.cwd.ends_with(&name),
            "工作目录必须落在工作区里：{}",
            info.cwd
        );

        // 回车在 PTY 上是 CR；命令与标记都取不可能与欢迎信息重合的字串。
        registry
            .write(&info.id, "echo axon-terminal-marker-7f3a\r")
            .expect("写入必须成功");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        let mut answered = 0_usize;
        while std::time::Instant::now() < deadline
            && !sink.text().contains("axon-terminal-marker-7f3a")
        {
            answer_cursor_queries(&registry, &info.id, &sink, &mut answered);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            sink.text().contains("axon-terminal-marker-7f3a"),
            "终端输出里必须出现命令的回显；实际收到：{}",
            sink.text()
        );

        // 关掉它：进程被杀掉，读取线程读到 EOF 后必须广播一次「用户关闭」。
        registry.close(&info.id).expect("关闭必须成功");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline && sink.exits().is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let exits = sink.exits();
        assert_eq!(exits.len(), 1, "退出必须恰好广播一次");
        assert!(exits[0].1, "主动关闭的会话必须标记成 closed");

        // 终端不存在之后再写必须如实报错，而不是静默丢字节。
        assert!(registry.write(&info.id, "echo late\r").is_err());
    }
}
