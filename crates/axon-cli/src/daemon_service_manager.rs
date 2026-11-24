use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::process::Stdio;

use axon_process::non_interactive_command;

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "axon-daemon";
#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "com.axon.daemon";
#[cfg(test)]
use windows_definition::WINDOWS_TASK_NAME;
pub(super) const DAEMON_PID_FILE_NAME: &str = "daemon.pid";
const DEFAULT_SOUL_CONTENT: &str = include_str!("../../../assets/defaults/SOUL.md");
#[cfg(any(target_os = "macos", windows))]
const PROCESS_STOP_TIMEOUT_MS: u64 = 10_000;
/// 优雅停止的等待窗口：够守护进程 drain 收口，又不至于让 stop 长时间无响应。
#[cfg(windows)]
const GRACEFUL_STOP_TIMEOUT_MS: u64 = 15_000;
#[cfg(any(target_os = "macos", windows))]
const PROCESS_START_TIMEOUT_MS: u64 = 60_000;
#[cfg(any(target_os = "macos", windows))]
const PROCESS_STOP_POLL_MS: u64 = 100;

#[cfg(windows)]
pub(super) fn current_windows_user_sid() -> Result<String, String> {
    windows_definition::current_user_sid()
}

pub(super) fn installed() -> bool {
    #[cfg(windows)]
    return windows_task::windows_task_exists();
    #[cfg(not(windows))]
    {
        unix_definition::platform_definition_path().is_some_and(|path| path.exists())
    }
}

pub(super) fn install(config_path: &Path, runtime_dir: &Path) -> Result<(), String> {
    ensure_default_soul(runtime_dir)?;
    let exe = stable_daemon_executable()?;
    #[cfg(target_os = "macos")]
    return unix_definition::install_launchd(&exe, config_path, runtime_dir);
    #[cfg(target_os = "linux")]
    return unix_definition::install_systemd(&exe, config_path, runtime_dir);
    #[cfg(windows)]
    return windows_task::install_windows_task(&exe, config_path, runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

/// Initialize the user-level Soul during every install, without overwriting edits.
fn ensure_default_soul(runtime_dir: &Path) -> Result<(), String> {
    let root = runtime_dir
        .parent()
        .ok_or_else(|| format!("无法解析 Axon 用户目录: {}", runtime_dir.display()))?;
    fs::create_dir_all(root).map_err(|error| format!("创建 Axon 用户目录失败: {error}"))?;
    let path = root.join("SOUL.md");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(DEFAULT_SOUL_CONTENT.as_bytes())
                .map_err(|error| format!("写入默认 SOUL.md 失败: {error}"))?;
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("初始化默认 SOUL.md 失败: {error}")),
    }
    Ok(())
}

/// 服务定义必须指向稳定 launcher，不能固化当前版本的绝对路径。
fn stable_daemon_executable() -> Result<PathBuf, String> {
    let current =
        std::env::current_exe().map_err(|error| format!("解析 axon 路径失败: {error}"))?;
    if let Some(launcher) = installed_launcher_path()
        && launcher.is_file()
    {
        return Ok(launcher);
    }
    Ok(stable_daemon_executable_from(&current))
}

fn installed_launcher_path() -> Option<PathBuf> {
    let root = std::env::var_os("AXON_INSTALL_ROOT")
        .map(PathBuf::from)
        .or_else(|| {
            #[cfg(windows)]
            let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
            #[cfg(not(windows))]
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
            home.map(PathBuf::from)
                .map(|home| home.join(".axon").join("bin"))
        })?;
    #[cfg(windows)]
    let launcher = "axon-launcher.exe";
    #[cfg(not(windows))]
    let launcher = "axon-launcher";
    Some(root.join("launcher").join(launcher))
}

fn stable_daemon_executable_from(current: &Path) -> PathBuf {
    let Some(version_dir) = current.parent() else {
        return current.to_path_buf();
    };
    let Some(versions_dir) = version_dir.parent() else {
        return current.to_path_buf();
    };
    if versions_dir.file_name().and_then(|name| name.to_str()) != Some("versions") {
        return current.to_path_buf();
    }
    let Some(install_root) = versions_dir.parent() else {
        return current.to_path_buf();
    };
    let Some(binary_name) = current.file_name() else {
        return current.to_path_buf();
    };
    #[cfg(windows)]
    let launcher_name = "axon-launcher.exe";
    #[cfg(not(windows))]
    let launcher_name = "axon-launcher";
    let _ = binary_name;
    install_root.join("launcher").join(launcher_name)
}

pub(super) fn uninstall(runtime_dir: &Path) -> Result<(), String> {
    if let Err(error) = stop(runtime_dir) {
        // 停不下来不阻断卸载，但必须把原因说出来：否则用户会以为服务已被彻底清理。
        eprintln!("警告：停止 Daemon 服务失败，继续卸载服务定义：{error}");
    }
    cleanup_stale_ipc_endpoint(runtime_dir);
    #[cfg(windows)]
    windows_task::uninstall_windows_task(runtime_dir)?;
    // 先 disable 再删单元文件：只删文件会留下 default.target.wants 下的悬空符号链接，
    // systemd 下次 reconcile 时仍会尝试拉起一个已不存在的单元。
    #[cfg(target_os = "linux")]
    let _ = unix_definition::systemctl(&["disable", SERVICE_NAME]);
    #[cfg(not(windows))]
    let path = unix_definition::platform_definition_path()
        .ok_or_else(|| "无法解析 daemon 服务路径".to_owned())?;
    #[cfg(not(windows))]
    if path.exists() {
        fs::remove_file(path).map_err(|error| format!("删除 daemon 服务定义失败: {error}"))?;
    }
    #[cfg(target_os = "linux")]
    unix_definition::systemctl(&["daemon-reload"])?;
    Ok(())
}

pub(super) fn start(config_path: &Path, runtime_dir: &Path) -> Result<(), String> {
    start_service(config_path, runtime_dir)?;
    // 「进程起来了」不等于「服务可用」：pid 文件只证明进程曾活到写出它，IPC bind、运行态库
    // 打开、网关启动任一步失败都会立刻退出。这里必须再探活一次，失败时把原因带回调用方。
    super::probe_health(super::DEFAULT_ENDPOINT)
        .map(|_| ())
        .map_err(|error| format!("Daemon 进程已启动但健康检查失败：{error}"))
}

fn start_service(_config_path: &Path, _runtime_dir: &Path) -> Result<(), String> {
    cleanup_stale_ipc_endpoint(_runtime_dir);
    // 陈旧的停止请求会让新进程立刻退出：启动前必须清掉。
    super::daemon_stop_request::clear(_runtime_dir);
    #[cfg(target_os = "macos")]
    {
        let path = unix_definition::platform_definition_path()
            .ok_or_else(|| "无法解析 LaunchAgents 目录".to_owned())?;
        let domain = unix_definition::launchd_domain();
        let target = format!("{domain}/{LAUNCHD_LABEL}");
        if !unix_definition::command_success(
            non_interactive_command("launchctl").args(["print", &target]),
        ) {
            run(
                non_interactive_command("launchctl").args([
                    "bootstrap",
                    &domain,
                    path.to_string_lossy().as_ref(),
                ]),
                "加载 launchd daemon 服务",
            )?;
        }
        run(
            non_interactive_command("launchctl").args(["kickstart", &target]),
            "启动 launchd daemon 服务",
        )?;
        unix_definition::wait_for_service_start(_runtime_dir)
    }
    #[cfg(target_os = "linux")]
    return unix_definition::systemctl(&["start", SERVICE_NAME]);
    #[cfg(windows)]
    return windows_task::start_windows_task(_runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

pub(super) fn stop(_runtime_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", unix_definition::launchd_domain(), LAUNCHD_LABEL);
        let status = non_interactive_command("launchctl")
            .args(["bootout", &target])
            .status()
            .map_err(|error| format!("停止 launchd daemon 服务失败: {error}"))?;
        if status.success() || !service_running(_runtime_dir) {
            return Ok(());
        }
        Err("停止 launchd daemon 服务失败".to_owned())
    }
    #[cfg(target_os = "linux")]
    return unix_definition::systemctl(&["stop", SERVICE_NAME]);
    #[cfg(windows)]
    return windows_task::stop_windows_task(_runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

pub(super) fn restart(config_path: &Path, runtime_dir: &Path) -> Result<(), String> {
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (config_path, runtime_dir);
    #[cfg(target_os = "macos")]
    {
        stop(runtime_dir)?;
        unix_definition::wait_for_daemon_exit(runtime_dir)?;
        cleanup_stale_ipc_endpoint(runtime_dir);
        start(config_path, runtime_dir)
    }
    #[cfg(target_os = "linux")]
    {
        // 与 macOS 同构：先停、再清陈旧 IPC 端点、最后起。unit 是 Restart=always，
        // 留着上次崩掉时的 socket 文件会让 systemd 陷入「拉起 → bind 失败 → 退出 → 再拉起」
        // 的循环，只有 daemon start 才救得回来。
        stop(runtime_dir)?;
        cleanup_stale_ipc_endpoint(runtime_dir);
        unix_definition::systemctl(&["start", SERVICE_NAME])
    }
    #[cfg(windows)]
    {
        stop(runtime_dir)?;
        start(config_path, runtime_dir)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

fn cleanup_stale_ipc_endpoint(runtime_dir: &Path) {
    let pid = fs::read_to_string(daemon_pid_path(runtime_dir))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    if pid.is_some_and(daemon_process_alive) {
        return;
    }
    let endpoint = runtime_dir.join("axon-daemon");
    if endpoint.exists() {
        let _ = fs::remove_file(endpoint);
    }
}

#[cfg(unix)]
fn daemon_pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(DAEMON_PID_FILE_NAME)
}

#[cfg(not(any(unix, windows)))]
fn daemon_pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(DAEMON_PID_FILE_NAME)
}

#[cfg(windows)]
fn daemon_pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(DAEMON_PID_FILE_NAME)
}

#[cfg(unix)]
pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(output) = non_interactive_command("ps")
        .args(["-o", "state=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let decoded = decode_command_output(&output.stdout);
    let state = decoded.split_whitespace().next().unwrap_or_default();
    !state.is_empty() && !state.starts_with('Z')
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn process_alive(_pid: u32) -> bool {
    false
}

pub(super) fn service_running(_runtime_dir: &Path) -> bool {
    #[cfg(target_os = "macos")]
    return unix_definition::launchd_service_active();
    #[cfg(target_os = "linux")]
    return unix_definition::command_success(non_interactive_command("systemctl").args([
        "--user",
        "is-active",
        "--quiet",
        SERVICE_NAME,
    ]));
    #[cfg(windows)]
    return windows_task::windows_daemon_process_alive(_runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    false
}

fn run(command: &mut Command, action: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{action}失败，无法启动外部命令: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = command_output_detail(&output.stdout, &output.stderr);
    if detail.is_empty() {
        Err(format!("{action}失败，exit={:?}", output.status.code()))
    } else {
        Err(format!(
            "{action}失败，exit={:?}，系统信息: {detail}",
            output.status.code()
        ))
    }
}

/// 读取目标 PID 的映像名；进程不存在或查询失败时返回 None。
#[cfg(windows)]
pub(super) fn process_image_name(pid: u32) -> Option<String> {
    process_pid_row(pid)
}

/// 读取目标 PID 的映像名；只有「映像名列非空且 PID 列精确相等」才算命中。
///
/// 不使用自由文本包含匹配：`tasklist` 输出里内存用量等列同样可能含有相同数字，
/// 无效 PID 的错误文案也可能带上该数字，两者都会把「不存在」误判成「存活」。
#[cfg(windows)]
fn process_pid_row(pid: u32) -> Option<String> {
    let output = non_interactive_command("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    decode_command_output(&output.stdout)
        .lines()
        .find_map(|line| {
            let mut fields = line.split(',').map(|field| field.trim().trim_matches('"'));
            let image = fields.next()?;
            let row_pid = fields.next()?.parse::<u32>().ok()?;
            (!image.is_empty() && row_pid == pid).then(|| image.to_owned())
        })
}

#[cfg(windows)]
pub(crate) fn process_alive(pid: u32) -> bool {
    // PID 0 是 System Idle Process：`tasklist` 会列出它，但它不是可管理的用户进程。
    if pid == 0 {
        return false;
    }
    process_pid_row(pid).is_some()
}

fn command_output_detail(stdout: &[u8], stderr: &[u8]) -> String {
    [stdout, stderr]
        .iter()
        .map(|bytes| decode_command_output(bytes).trim().to_owned())
        .find(|text| !text.is_empty())
        .unwrap_or_default()
}

#[cfg(windows)]
fn decode_command_output(bytes: &[u8]) -> String {
    encoding_rs::GBK.decode(bytes).0.into_owned()
}

#[cfg(not(windows))]
fn decode_command_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[path = "daemon_service_definitions.rs"]
mod service_definitions;
#[cfg(any(target_os = "macos", test))]
use service_definitions::launchd_plist;
#[cfg(any(target_os = "linux", test))]
use service_definitions::systemd_unit;

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[path = "daemon_service_unix.rs"]
mod unix_definition;

#[path = "daemon_service_windows.rs"]
mod windows_definition;
#[cfg(test)]
use windows_definition::{parse_windows_user_sid, windows_task_xml};

#[cfg(windows)]
#[path = "daemon_service_windows_task.rs"]
mod windows_task;

#[path = "daemon_process_probe.rs"]
mod process_probe;
pub(crate) use process_probe::daemon_process_alive;

#[cfg(test)]
#[path = "daemon_service_manager_tests.rs"]
mod tests;
