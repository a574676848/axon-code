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
#[cfg(unix)]
use std::time::{Duration, Instant};
use std::{
    io,
    process::{Child, Command, Stdio},
};

#[cfg(unix)]
const MANAGED_PROCESS_TERMINATION_GRACE_MS: u64 = 100;
#[cfg(unix)]
const MANAGED_PROCESS_POLL_MS: u64 = 10;

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

/// 受管子进程的整树回收句柄。
///
/// 为什么需要它：Windows 上 [`terminate_managed_process_tree`] 必须 spawn 一个
/// `taskkill /T /F` 才拿得到整棵树，而那次外部进程创建的成本由杀毒扫描与系统负载决定、
/// 没有上界（本机实测单次 0.45~0.9s，并发跑测试时可达数秒）。硬超时的收敛时间因此被一个
/// 与超时无关的外部工具拖走。Job Object 把「整棵树」变成内核对象：派生后绑定一次，
/// 回收只需关闭句柄，`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 由内核一次性终止作业内全部进程，
/// 不 spawn 任何外部程序，后代在创建时自动入作业、不会漏。
///
/// 其余平台没有等价内核对象，回收沿用进程组信号（同样是 `kill(2)` 语义，不受
/// 进程创建开销影响），因此调用方只认本类型、不写平台分支。
pub struct ManagedProcessTree {
    /// Windows 侧的 Job 句柄；关闭它即命中 `KILL_ON_JOB_CLOSE`。
    #[cfg(windows)]
    fence: Option<resource_fence::ProcessFenceHandle>,
}

impl ManagedProcessTree {
    /// 在 `spawn` 成功后绑定子进程及其后续派生出的全部后代。
    ///
    /// 绑定失败不阻断执行：少一层内核围栏只是回收退回 `terminate_managed_process_tree`
    /// 的 `taskkill` 扫描，而拒绝启动会直接让调用方跑不起来：这与资源围栏
    /// 「Windows 侧尽力而为、失败不阻断」同一口径。
    #[must_use]
    pub fn adopt(child: &Child) -> Self {
        #[cfg(windows)]
        {
            // 只借用资源围栏的绑定路径，不施加任何限额：provider 脚本没有限额策略。
            Self {
                fence: resource_fence::fence_spawned_child(
                    child,
                    resource_fence::ProcessResourceLimits::off(),
                )
                .ok()
                .flatten(),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Self {}
        }
    }

    /// 立即回收整棵进程树；返回时该树已被终止。
    pub fn terminate(&mut self, child: &mut Child) -> io::Result<()> {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        #[cfg(windows)]
        if let Some(fence) = self.fence.take() {
            // 关闭句柄即关闭 Job：内核按 `KILL_ON_JOB_CLOSE` 终止作业内全部进程。
            drop(fence);
            return Ok(());
        }
        terminate_managed_process_tree(child)
    }
}

/// 以进程组为单位终止整棵树：先 `SIGTERM` 给宽限期，仍在则 `SIGKILL`。
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

/// 进程启动标识：判「同一个 PID 是否仍是同一个进程」的第二身份。
///
/// PID 会被系统复用，单凭 PID 判活会把无关进程当成宿主（租约因此永远不被回收，新宿主
/// 也认领不到）。这里取平台原生的启动时刻做标识：Linux 读 `/proc/<pid>/stat` 的
/// starttime，其余 Unix 用 `ps -o lstart=`，Windows 用 `Get-Process` 的启动时刻。
///
/// 取不到时返回 `None`，调用方必须退回「只看 PID 存活」的弱判定，不得据此认为进程已死。
pub fn process_start_id(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // comm 字段可能含空格与括号：从最后一个 ')' 之后开始切分，字段偏移才对得上。
        // ')' 之后第 1 个字段是 state（整体第 3 字段），starttime 是整体第 22 字段。
        let tail = stat.rsplit_once(')')?.1;
        let start_time = tail.split_whitespace().nth(19)?;
        Some(format!("linux-starttime:{start_time}"))
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        let output = non_interactive_command("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!text.is_empty()).then(|| format!("ps-lstart:{text}"))
    }
    #[cfg(windows)]
    {
        let output = non_interactive_command("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-Process -Id {pid}).StartTime.ToFileTimeUtc()"),
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        (!text.is_empty()).then(|| format!("win-starttime:{text}"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
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
        const WRITE_TITLE: &str = "title axon-child-title & ping -n 2 127.0.0.1 >nul & exit 0";
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
