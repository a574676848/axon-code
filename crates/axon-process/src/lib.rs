//! 非交互子进程的平台隔离策略。
//!
//! 后台命令必须通过本 crate 配置，避免 Windows 为短生命周期命令创建可见控制台。

/// 子进程资源围栏（rlimit / Job Object）。
///
/// 本模块是全仓唯一允许 `unsafe` 的模块，原因见模块文档。
#[path = "resource_fence.rs"]
pub mod resource_fence;

pub use resource_fence::{
    decode_console_bytes, try_decode_console_bytes, try_decode_console_code_page,
};

#[cfg(unix)]
use std::thread;
use std::{
    collections::HashMap,
    io,
    process::{Child, Command, Stdio},
    sync::{LazyLock, Mutex},
    time::{Duration, Instant},
};

#[cfg(unix)]
const MANAGED_PROCESS_TERMINATION_GRACE_MS: u64 = 100;
#[cfg(unix)]
const MANAGED_PROCESS_POLL_MS: u64 = 10;

static MANAGED_PROCESSES: LazyLock<Mutex<HashMap<u32, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 定时清理受管工具进程的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ManagedProcessCleanupReport {
    /// 扫描时仍在登记的进程组数量。
    pub tracked: usize,
    /// 本次触发终止的超时进程组数量。
    pub terminated: usize,
}

/// 受管工具进程登记守卫。
pub struct ManagedProcessGuard {
    pid: u32,
}

/// 创建同步非交互子进程。
pub fn non_interactive_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    configure_non_interactive_process(&mut command);
    command
}

/// 创建 Tokio 异步非交互子进程。
pub fn non_interactive_tokio_command(
    program: impl AsRef<std::ffi::OsStr>,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    configure_non_interactive_tokio_process(&mut command);
    command
}

/// 配置同步非交互子进程。
pub fn configure_non_interactive_process(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    let _ = command;
}

/// 配置需要整棵回收的工具进程。
pub fn configure_managed_process_group(command: &mut Command) {
    configure_non_interactive_process(command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

/// 登记受管工具进程及其硬截止时间。
pub fn track_managed_process(pid: u32, timeout: Duration) -> ManagedProcessGuard {
    let deadline = Instant::now() + timeout;
    MANAGED_PROCESSES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(pid, deadline);
    ManagedProcessGuard { pid }
}

/// 终止直接子进程及其进程树。
pub fn terminate_managed_process_tree(child: &mut Child) -> io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if let Err(error) = terminate_managed_process_tree_by_pid(child.id())
        && child.try_wait()?.is_none()
    {
        return Err(error);
    }
    match child.kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => Err(error),
    }
}

/// 按 PID 终止受管进程及其整棵后代树。
pub fn terminate_managed_process_tree_by_pid(pid: u32) -> io::Result<()> {
    terminate_process_tree(pid)
}

/// 扫描并终止超过硬截止时间但尚未完成回收的工具进程组。
pub fn cleanup_expired_managed_processes() -> ManagedProcessCleanupReport {
    let now = Instant::now();
    let expired = {
        let processes = MANAGED_PROCESSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        processes
            .iter()
            .filter_map(|(pid, deadline)| (*deadline <= now).then_some(*pid))
            .collect::<Vec<_>>()
    };
    for pid in &expired {
        let _ = terminate_process_tree(*pid);
    }
    ManagedProcessCleanupReport {
        tracked: MANAGED_PROCESSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        terminated: expired.len(),
    }
}

impl Drop for ManagedProcessGuard {
    fn drop(&mut self) {
        MANAGED_PROCESSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.pid);
    }
}

#[cfg(unix)]
fn terminate_process_tree(pid: u32) -> io::Result<()> {
    let process_group = i32::try_from(pid)
        .ok()
        .and_then(i32::checked_neg)
        .ok_or_else(|| io::Error::other("managed process id is out of range"))?;
    signal_process_group(process_group, 15)?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(MANAGED_PROCESS_TERMINATION_GRACE_MS) {
        if !process_group_alive(process_group) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(MANAGED_PROCESS_POLL_MS));
    }
    signal_process_group(process_group, 9)
}

#[cfg(unix)]
fn process_group_alive(process_group: i32) -> bool {
    process_group_signal_command(process_group, "-0")
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
fn signal_process_group(process_group: i32, signal: i32) -> io::Result<()> {
    let signal = match signal {
        9 => "-KILL",
        15 => "-TERM",
        _ => return Err(io::Error::other("unsupported managed process signal")),
    };
    let _ = process_group_signal_command(process_group, signal).status()?;
    Ok(())
}

#[cfg(unix)]
fn process_group_signal_command(process_group: i32, signal: &str) -> Command {
    let mut command = non_interactive_command("/bin/kill");
    command
        .args([signal, "--", &process_group.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(windows)]
fn terminate_process_tree(pid: u32) -> io::Result<()> {
    let status = non_interactive_command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("taskkill failed for managed process"))
    }
}

/// 配置与宿主控制台完全分离的后台子进程。
pub fn configure_detached_process(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    #[cfg(not(windows))]
    let _ = command;
}

/// 配置 Tokio 异步非交互子进程。
pub fn configure_non_interactive_tokio_process(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_preserves_sync_and_async_arguments() {
        let mut sync = non_interactive_command("axon-process-probe");
        sync.arg("--sync");
        assert_eq!(sync.get_args().collect::<Vec<_>>(), ["--sync"]);

        let mut asynchronous = non_interactive_tokio_command("axon-process-probe");
        asynchronous.arg("--async");
        assert_eq!(
            asynchronous.as_std().get_args().collect::<Vec<_>>(),
            ["--async"]
        );
    }

    #[test]
    fn background_workspace_and_lease_probes_cannot_bypass_process_policy() {
        let probe_sources = [
            include_str!("../../axon-cli/src/run_commands/run_workspace_identity.rs"),
            include_str!("../../axon-session/src/application/workspace_resolver.rs"),
            include_str!(
                "../../axon-session/src/infrastructure/sqlite_runtime_store/host_process.rs"
            ),
        ];
        for source in probe_sources {
            assert!(
                !source.contains("Command::new("),
                "后台探测不得绕过共享的隐藏进程创建策略"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn terminating_already_exited_child_is_idempotent() {
        let mut child = non_interactive_command("cmd.exe")
            .args(["/d", "/c", "exit 0"])
            .spawn()
            .expect("测试子进程应可启动");
        child.wait().expect("测试子进程应可退出");

        terminate_managed_process_tree(&mut child).expect("已退出子进程不应触发 taskkill 误报");
    }

    #[test]
    #[cfg(windows)]
    fn isolated_child_cannot_acquire_console_title_ownership() {
        use std::time::Duration;

        const CHILD_TITLE: &str = "axon-child-title";
        const WRITE_TITLE: &str = "title axon-child-title & ping -n 2 127.0.0.1 >nul";
        let read_title = || {
            let output = Command::new("pwsh.exe")
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "[Console]::Title",
                ])
                .output()
                .expect("应可读取父控制台标题");
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        let mut child = non_interactive_command("cmd.exe")
            .args(["/d", "/s", "/c", WRITE_TITLE])
            .spawn()
            .expect("隔离后的 cmd.exe 应可启动");

        std::thread::sleep(Duration::from_millis(100));
        let observed = read_title();
        let status = child.wait().expect("隔离后的 cmd.exe 应正常退出");

        assert!(status.success());
        assert_ne!(observed, CHILD_TITLE);
    }
}
