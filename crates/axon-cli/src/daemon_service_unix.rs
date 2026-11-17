//! POSIX 平台的 daemon 用户服务定义与启停。
//!
//! macOS 用 launchd 的 LaunchAgent、Linux 用 systemd 的用户单元：两者都先把服务定义
//! 写入平台约定的用户目录，再按各自的命令加载、启用与启停，并在启动或重启后等待
//! 进程真正进入预期状态。Windows 的计划任务实现见 `daemon_service_windows_task` 子模块。

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use axon_process::non_interactive_command;

#[cfg(target_os = "linux")]
use super::run;
#[cfg(target_os = "macos")]
use super::{PROCESS_START_TIMEOUT_MS, PROCESS_STOP_POLL_MS, PROCESS_STOP_TIMEOUT_MS};
#[cfg(target_os = "macos")]
use super::{daemon_pid_path, process_alive};
use super::{restart, service_running, start};

/// 启动后等待服务进入 running 状态。
///
/// 三个条件必须同时成立：launchd 认为服务在跑、`daemon.pid` 指向的进程仍存活、
/// IPC 端点文件已落盘。只看 launchd 状态会在进程刚拉起就崩时误判为成功。
#[cfg(target_os = "macos")]
pub(super) fn wait_for_service_start(runtime_dir: &Path) -> Result<(), String> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(PROCESS_START_TIMEOUT_MS);
    while std::time::Instant::now() < deadline {
        let pid_ready = fs::read_to_string(daemon_pid_path(runtime_dir))
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_some_and(process_alive);
        if launchd_service_active() && pid_ready && runtime_dir.join("axon-daemon").exists() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(PROCESS_STOP_POLL_MS));
    }
    Err("Daemon 服务未在启动截止时间内进入 running 状态，请检查 data/logs 日志".to_owned())
}

/// 重启前等待旧进程退出，避免新旧实例同时持有 IPC 端点。
#[cfg(target_os = "macos")]
pub(super) fn wait_for_daemon_exit(runtime_dir: &Path) -> Result<(), String> {
    let Some(pid) = fs::read_to_string(daemon_pid_path(runtime_dir))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
    else {
        return Ok(());
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(PROCESS_STOP_TIMEOUT_MS);
    while process_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(PROCESS_STOP_POLL_MS));
    }
    if process_alive(pid) {
        return Err(format!("Daemon 旧进程未在重启截止时间内退出: pid={pid}"));
    }
    Ok(())
}

/// launchd 是否认为服务正在运行。
///
/// `launchctl print` 的输出里，`state = running` 与活动计数大于 0 都算运行中：
/// 只认前者会在 KeepAlive 刚拉起、状态行尚未刷新时误判为未运行。
#[cfg(target_os = "macos")]
pub(super) fn launchd_service_active() -> bool {
    let target = format!("{}/{}", launchd_domain(), super::LAUNCHD_LABEL);
    let Ok(output) = non_interactive_command("launchctl")
        .args(["print", &target])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let detail = super::command_output_detail(&output.stdout, &output.stderr);
    detail.lines().any(|line| {
        let line = line.trim();
        line == "state = running" || line.starts_with("active count = ") && !line.ends_with("= 0")
    })
}

#[cfg(target_os = "macos")]
pub(super) fn install_launchd(exe: &Path, config: &Path, runtime: &Path) -> Result<(), String> {
    let path = platform_definition_path().ok_or_else(|| "无法解析 LaunchAgents 目录".to_owned())?;
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|error| format!("创建 LaunchAgents 目录失败: {error}"))?;
    fs::create_dir_all(runtime.join("logs"))
        .map_err(|error| format!("创建 daemon 日志目录失败: {error}"))?;
    fs::write(&path, super::launchd_plist(exe, config, runtime))
        .map_err(|error| format!("写入 launchd plist 失败: {error}"))?;
    let target = format!("{}/{}", launchd_domain(), super::LAUNCHD_LABEL);
    if service_running(runtime) {
        return restart(config, runtime);
    }
    let _ = non_interactive_command("launchctl")
        .args(["bootout", &target])
        .status();
    if service_running(runtime) {
        Ok(())
    } else {
        start(config, runtime)
    }
}

#[cfg(target_os = "linux")]
pub(super) fn install_systemd(exe: &Path, config: &Path, runtime: &Path) -> Result<(), String> {
    let path = platform_definition_path().ok_or_else(|| "无法解析 systemd 用户目录".to_owned())?;
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|error| format!("创建 systemd 用户目录失败: {error}"))?;
    fs::write(
        &path,
        super::systemd_unit(exe, config, runtime, &service_environment()),
    )
    .map_err(|error| format!("写入 systemd unit 失败: {error}"))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", super::SERVICE_NAME])?;
    if service_running(runtime) {
        restart(config, runtime)
    } else {
        start(config, runtime)
    }
}

/// 用户单元必须显式带上安装现场的 `PATH` 与 `SHELL`。
///
/// systemd 用户实例只给一份最小环境：交互 shell 里的 `PATH` 扩展（`~/.cargo/bin`、
/// `~/.local/bin`、版本管理器 shim）与 `SHELL` 都不会被继承。守护进程之下再启动 shell
/// 工具时会找不到这些命令，缺 `SHELL` 还会退化成 `/bin/sh` 非登录 shell。
/// 取不到或为空的值一律不写，避免在单元里落一个空变量。
#[cfg(target_os = "linux")]
fn service_environment() -> Vec<(String, String)> {
    ["PATH", "SHELL"]
        .into_iter()
        .filter_map(|key| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (key.to_owned(), value))
        })
        .collect()
}

/// 服务定义文件在用户目录下的位置。
pub(super) fn platform_definition_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME").map(PathBuf::from).map(|home| {
        home.join("Library/LaunchAgents")
            .join(format!("{}.plist", super::LAUNCHD_LABEL))
    });
    #[cfg(target_os = "linux")]
    return systemd_user_unit_dir().map(|dir| dir.join(format!("{}.service", super::SERVICE_NAME)));
}

/// systemd 用户单元目录。
///
/// systemd 只按 `$XDG_CONFIG_HOME/systemd/user` 查找用户单元，未设置该变量时才是
/// `~/.config/systemd/user`。写死 `~/.config` 会让设置了 `XDG_CONFIG_HOME` 的用户
/// 装出一份 systemd 永远读不到的服务定义，`systemctl --user start` 报 unit 不存在。
#[cfg(target_os = "linux")]
fn systemd_user_unit_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("systemd/user"))
}

/// launchd 的用户域标识：`gui/<uid>`。
#[cfg(target_os = "macos")]
pub(super) fn launchd_domain() -> String {
    let output = non_interactive_command("id").arg("-u").output().ok();
    let uid = output
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    format!("gui/{uid}")
}

#[cfg(target_os = "linux")]
pub(super) fn systemctl(args: &[&str]) -> Result<(), String> {
    run(
        non_interactive_command("systemctl")
            .arg("--user")
            .args(args),
        "执行 systemd daemon 操作",
    )
}

/// 只关心退出状态、不关心输出的命令探测。
pub(super) fn command_success(command: &mut Command) -> bool {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
