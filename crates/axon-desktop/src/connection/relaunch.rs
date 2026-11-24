//! 切档重启：launcher 专用解析、中继进程与失败回滚。
//!
//! 切换档案等于整链重建（事件流是进程级单例），因此切档就是重启应用。三条看似
//! 可行、实际都不成立的做法必须先排除：
//!
//! 1. 指望「新进程唤回旧进程」：单实例插件的第二实例是**交参数后立即退出**，
//!    会得到「新进程秒退、旧进程仍在、`lastProfileId` 已写盘」。
//! 2. 直接调 `process::restart`：它先 spawn 新进程再 `exit(0)`，新进程启动时旧进程
//!    仍持锁；且运行中 `current_exe()` 就是版本目录二进制，违反「经稳定入口启动」。
//! 3. 关窗再启动：`CloseRequested` 被托盘改写为 `hide()`，进程根本不退出。
//!
//! 因此这里引入一个**中继进程**：它用 `current_exe()` 启动，但**不建窗口、不注册任何
//! 插件、不装载单实例锁**，所以不会被判成第二实例；它只等旧进程消失，然后经稳定
//! 入口拉起 launcher。中继不是一个界面进程，这一点与「禁止直接拉起版本目录二进制
//! 去当界面」并不冲突。

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 中继参数前缀；必须隐藏、必须从 argv 消费掉、不得透传。
pub const RELAUNCH_ARG_PREFIX: &str = "--relaunch-after=";
/// 拉起失败时留证的文件名。
pub const FAILURE_FILE: &str = "relaunch-failed.json";
/// 已消费的失败记录文件名（改名而不是删除，留证但不再消费）。
pub const HANDLED_FILE: &str = "relaunch-failed.handled.json";
/// launcher 所在子目录。
const LAUNCHER_DIR: &str = "launcher";
/// 等旧进程退出的上限。
const OLD_PID_WAIT: Duration = Duration::from_secs(30);
/// 旧进程超时后额外再等的时间。
const OLD_PID_EXTRA_WAIT: Duration = Duration::from_secs(10);
/// 轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// 拉起失败的重试退避。
const LAUNCH_BACKOFF_MS: [u64; 3] = [200, 500, 1000];
/// launcher 的桌面端子命令。
const DESKTOP_SUBCOMMAND: &str = "desktop";
/// Windows 侧脱离当前控制台：新进程不继承控制台，也不弹窗。
#[cfg(windows)]
const DETACHED_AND_NO_WINDOW: u32 = 0x0000_0008 | 0x0000_0200 | 0x0800_0000;

/// 拉起失败时写下的事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureRecord {
    /// 本次想切到的档案 id。
    pub target_profile_id: String,
    /// 切换前的档案 id，供下次启动回滚。
    pub previous_profile_id: Option<String>,
    /// 中文原因。
    pub reason: String,
    /// 发生时间（毫秒）。
    pub at_ms: u64,
}

/// 从 argv 中取出并**消费** `--relaunch-after=<pid>`。
///
/// 识别必须发生在 Tauri 初始化与单实例注册之前，因此调用方在 `main` 的最前面
/// 调用本函数，并把返回的参数表继续交给 Tauri。
#[must_use]
pub fn take_relaunch_argument(args: Vec<OsString>) -> (Option<u32>, Vec<OsString>) {
    let mut old_pid = None;
    let mut kept = Vec::with_capacity(args.len());
    for arg in args {
        let text = arg.to_string_lossy();
        match text
            .strip_prefix(RELAUNCH_ARG_PREFIX)
            .and_then(|value| value.parse::<u32>().ok())
        {
            Some(pid) => old_pid = Some(pid),
            None => kept.push(arg),
        }
    }
    (old_pid, kept)
}

/// 安装根：`AXON_INSTALL_ROOT` 优先，否则用户主目录下的 `.axon/bin`。
///
/// 与「版本目录」无关：这里要的是**稳定入口**所在的那一层。
#[must_use]
pub fn install_root() -> PathBuf {
    if let Some(custom) = std::env::var_os("AXON_INSTALL_ROOT").filter(|value| !value.is_empty()) {
        return PathBuf::from(custom);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".axon").join("bin")
}

/// launcher 的相对路径。
#[must_use]
pub fn launcher_relative_path() -> PathBuf {
    let name = if cfg!(windows) {
        "axon-launcher.exe"
    } else {
        "axon-launcher"
    };
    PathBuf::from(LAUNCHER_DIR).join(name)
}

/// 目标是否可作为 launcher 启动。
///
/// **必须是普通文件且可执行**：既有安装态 CLI 解析是混合候选表（launcher、
/// `axon.exe`、`axon.cmd`、cargo bin），只按 `is_file()` 命中且不校验可执行位。
/// 复用它会让「launcher 被删掉」的前置检查照样通过，随后中继去执行一个 `.cmd`
/// 而失败，因此这里必须有专用解析。
#[must_use]
pub fn is_launchable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 在给定安装根下解析 launcher。
///
/// # Errors
///
/// launcher 不存在、不是普通文件或不可执行。
pub fn resolve_launcher_at(install_root: &Path) -> Result<PathBuf, String> {
    let candidate = install_root.join(launcher_relative_path());
    if is_launchable(&candidate) {
        return Ok(candidate);
    }
    Err(format!(
        "安装目录里的稳定入口不可用：{}",
        candidate.display()
    ))
}

/// 用当前安装根解析 launcher。
///
/// # Errors
///
/// 同 [`resolve_launcher_at`]。
pub fn resolve_launcher() -> Result<PathBuf, String> {
    resolve_launcher_at(&install_root())
}

/// 以脱离方式启动中继进程。
///
/// 脱离的含义：Windows 用 `DETACHED_PROCESS` 加不建控制台，Unix 用独立进程组，
/// 这样原终端关闭或父进程退出都不会把整条链带走。
///
/// # Errors
///
/// 取不到自身路径或 spawn 失败。
pub fn spawn_relay(old_pid: u32) -> Result<(), String> {
    let exe =
        std::env::current_exe().map_err(|error| format!("无法定位桌面端自身路径：{error}"))?;
    let mut command = std::process::Command::new(exe);
    command
        .arg(format!("{RELAUNCH_ARG_PREFIX}{old_pid}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("启动中继进程失败：{error}"))
}

/// 让子进程脱离当前会话。
#[cfg(windows)]
fn detach(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(DETACHED_AND_NO_WINDOW);
}

/// 让子进程脱离当前会话。
#[cfg(unix)]
fn detach(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

/// 中继进程主体：等旧进程消失，再经稳定入口拉起桌面端。
///
/// 返回进程退出码：0 表示已拉起，非 0 表示拉起失败并已写下失败记录。
#[must_use]
pub fn run_relay(old_pid: u32, launcher: &Path) -> i32 {
    if wait_for_exit(old_pid, OLD_PID_WAIT) {
        return launch_desktop(launcher);
    }
    // 旧进程 30 秒仍未消失：再等 10 秒后强行尝试拉起一次。
    if wait_for_exit(old_pid, OLD_PID_EXTRA_WAIT) {
        return launch_desktop(launcher);
    }
    launch_desktop(launcher)
}

/// 轮询等待目标进程消失。
fn wait_for_exit(pid: u32, budget: Duration) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < budget {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    !process_alive(pid)
}

/// 进程是否仍在。
fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let target = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&[target]), true);
    system.process(target).is_some()
}

/// 经 launcher 拉起桌面端，失败按 200 / 500 / 1000 毫秒退避重试三次。
fn launch_desktop(launcher: &Path) -> i32 {
    let mut last_error = String::new();
    for backoff in LAUNCH_BACKOFF_MS {
        match spawn_launcher(launcher) {
            Ok(()) => return 0,
            Err(error) => {
                last_error = error;
                std::thread::sleep(Duration::from_millis(backoff));
            }
        }
    }
    let record = FailureRecord {
        target_profile_id: current_target_profile(),
        previous_profile_id: current_previous_profile(),
        reason: last_error,
        at_ms: now_ms(),
    };
    if let Err(error) = write_failure_record(&record) {
        eprintln!("ERROR: 拉起失败记录写入失败：{error}");
    }
    1
}

/// 经稳定入口启动桌面端。
fn spawn_launcher(launcher: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new(launcher);
    command
        .arg(DESKTOP_SUBCOMMAND)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("拉起桌面端失败：{error}"))
}

/// 从状态文件读本次想切到的档案 id；读不到时给空串（记录里仍然是事实）。
fn current_target_profile() -> String {
    crate::connection::store::load_document()
        .document
        .client
        .last_profile_id
        .unwrap_or_default()
}

/// 从状态文件读切换前的档案 id。
fn current_previous_profile() -> Option<String> {
    crate::connection::store::load_document()
        .document
        .client
        .previous_profile_id
}

/// 写下失败记录。
///
/// # Errors
///
/// 序列化或写盘失败。
pub fn write_failure_record(record: &FailureRecord) -> Result<PathBuf, String> {
    let path = crate::connection::store::client_root().join(FAILURE_FILE);
    let text = serde_json::to_string_pretty(record)
        .map_err(|error| format!("序列化拉起失败记录失败：{error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建客户端数据目录失败：{error}"))?;
    }
    std::fs::write(&path, text).map_err(|error| format!("写入拉起失败记录失败：{error}"))?;
    Ok(path)
}

/// 消费一次失败记录。
///
/// **必须消费一次即改名**：否则每次启动都会回退到 `previousProfileId`，用户后续
/// 切档永不生效。改名而不是删除，是为了留证。
#[must_use]
pub fn consume_pending_failure() -> Option<FailureRecord> {
    let root = crate::connection::store::client_root();
    let path = root.join(FAILURE_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let record = serde_json::from_str::<FailureRecord>(&text).ok()?;
    let handled = root.join(HANDLED_FILE);
    if let Err(error) = std::fs::rename(&path, &handled) {
        eprintln!("WARNING: 拉起失败记录改名失败（下次启动会重复回退）：{error}");
    }
    Some(record)
}

/// 当前时间（毫秒）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "relaunch_tests.rs"]
mod relaunch_tests;
