use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use axon_process::non_interactive_command;

#[cfg(target_os = "linux")]
const SERVICE_NAME: &str = "axon-daemon";
#[cfg(target_os = "macos")]
const LAUNCHD_LABEL: &str = "com.axon.daemon";
#[cfg(any(windows, test))]
use windows_definition::WINDOWS_TASK_NAME;
pub(super) const DAEMON_PID_FILE_NAME: &str = "daemon.pid";
const DEFAULT_SOUL_CONTENT: &str = include_str!("../../../assets/defaults/SOUL.md");
#[cfg(any(unix, windows))]
const PROCESS_STOP_TIMEOUT_MS: u64 = 10_000;
/// 优雅停止的等待窗口：够守护进程 drain 收口，又不至于让 stop 长时间无响应。
#[cfg(windows)]
const GRACEFUL_STOP_TIMEOUT_MS: u64 = 15_000;
#[cfg(any(unix, windows))]
const PROCESS_START_TIMEOUT_MS: u64 = 60_000;
#[cfg(any(unix, windows))]
const PROCESS_STOP_POLL_MS: u64 = 100;

#[cfg(windows)]
pub(super) fn current_windows_user_sid() -> Result<String, String> {
    windows_definition::current_user_sid()
}

pub(super) fn installed() -> bool {
    #[cfg(windows)]
    return windows_task_exists();
    #[cfg(not(windows))]
    {
        platform_definition_path().is_some_and(|path| path.exists())
    }
}

pub(super) fn install(config_path: &Path, runtime_dir: &Path) -> Result<(), String> {
    ensure_default_soul(runtime_dir)?;
    let exe = stable_daemon_executable()?;
    #[cfg(target_os = "macos")]
    return install_launchd(&exe, config_path, runtime_dir);
    #[cfg(target_os = "linux")]
    return install_systemd(&exe, config_path, runtime_dir);
    #[cfg(windows)]
    return install_windows_task(&exe, config_path, runtime_dir);
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
    let _ = stop(runtime_dir);
    cleanup_stale_ipc_endpoint(runtime_dir);
    #[cfg(windows)]
    if windows_task_exists() {
        stop_windows_task(runtime_dir)?;
        run(
            non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
                "/delete",
                "/tn",
                WINDOWS_TASK_NAME,
                "/f",
            ]),
            "删除 Windows daemon 计划任务",
        )?;
    }
    #[cfg(not(windows))]
    let path = platform_definition_path().ok_or_else(|| "无法解析 daemon 服务路径".to_owned())?;
    #[cfg(not(windows))]
    if path.exists() {
        fs::remove_file(path).map_err(|error| format!("删除 daemon 服务定义失败: {error}"))?;
    }
    #[cfg(target_os = "linux")]
    systemctl(&["daemon-reload"])?;
    Ok(())
}

pub(super) fn start(_config_path: &Path, _runtime_dir: &Path) -> Result<(), String> {
    cleanup_stale_ipc_endpoint(_runtime_dir);
    // 陈旧的停止请求会让新进程立刻退出：启动前必须清掉。
    super::daemon_stop_request::clear(_runtime_dir);
    #[cfg(target_os = "macos")]
    {
        let path =
            platform_definition_path().ok_or_else(|| "无法解析 LaunchAgents 目录".to_owned())?;
        let domain = launchd_domain();
        let target = format!("{domain}/{LAUNCHD_LABEL}");
        if !command_success(non_interactive_command("launchctl").args(["print", &target])) {
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
        wait_for_service_start(_runtime_dir)
    }
    #[cfg(target_os = "linux")]
    return systemctl(&["start", SERVICE_NAME]);
    #[cfg(windows)]
    return start_windows_task(_runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

#[cfg(target_os = "macos")]
fn wait_for_service_start(runtime_dir: &Path) -> Result<(), String> {
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

pub(super) fn stop(_runtime_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let target = format!("{}/{}", launchd_domain(), LAUNCHD_LABEL);
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
    return systemctl(&["stop", SERVICE_NAME]);
    #[cfg(windows)]
    return stop_windows_task(_runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

pub(super) fn restart(config_path: &Path, runtime_dir: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        stop(runtime_dir)?;
        wait_for_daemon_exit(runtime_dir)?;
        cleanup_stale_ipc_endpoint(runtime_dir);
        start(config_path, runtime_dir)
    }
    #[cfg(target_os = "linux")]
    return systemctl(&["restart", SERVICE_NAME]);
    #[cfg(windows)]
    {
        stop(runtime_dir)?;
        start(config_path, runtime_dir)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("当前平台暂不支持 daemon 用户服务".to_owned())
}

#[cfg(unix)]
fn wait_for_daemon_exit(runtime_dir: &Path) -> Result<(), String> {
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

fn cleanup_stale_ipc_endpoint(runtime_dir: &Path) {
    let pid = fs::read_to_string(daemon_pid_path(runtime_dir))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok());
    if pid.is_some_and(process_alive) {
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
    return launchd_service_active();
    #[cfg(target_os = "linux")]
    return command_success(non_interactive_command("systemctl").args([
        "--user",
        "is-active",
        "--quiet",
        SERVICE_NAME,
    ]));
    #[cfg(windows)]
    return windows_daemon_process_alive(_runtime_dir);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    false
}

#[cfg(target_os = "macos")]
fn launchd_service_active() -> bool {
    let target = format!("{}/{}", launchd_domain(), LAUNCHD_LABEL);
    let Ok(output) = non_interactive_command("launchctl")
        .args(["print", &target])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let detail = command_output_detail(&output.stdout, &output.stderr);
    detail.lines().any(|line| {
        let line = line.trim();
        line == "state = running" || line.starts_with("active count = ") && !line.ends_with("= 0")
    })
}

#[cfg(target_os = "macos")]
fn install_launchd(exe: &Path, config: &Path, runtime: &Path) -> Result<(), String> {
    let path = platform_definition_path().ok_or_else(|| "无法解析 LaunchAgents 目录".to_owned())?;
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|error| format!("创建 LaunchAgents 目录失败: {error}"))?;
    fs::create_dir_all(runtime.join("logs"))
        .map_err(|error| format!("创建 daemon 日志目录失败: {error}"))?;
    fs::write(&path, launchd_plist(exe, config, runtime))
        .map_err(|error| format!("写入 launchd plist 失败: {error}"))?;
    let target = format!("{}/{}", launchd_domain(), LAUNCHD_LABEL);
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
fn install_systemd(exe: &Path, config: &Path, runtime: &Path) -> Result<(), String> {
    let path = platform_definition_path().ok_or_else(|| "无法解析 systemd 用户目录".to_owned())?;
    fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|error| format!("创建 systemd 用户目录失败: {error}"))?;
    fs::write(&path, systemd_unit(exe, config, runtime))
        .map_err(|error| format!("写入 systemd unit 失败: {error}"))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", SERVICE_NAME])?;
    if service_running(runtime) {
        restart(config, runtime)
    } else {
        start(config, runtime)
    }
}

#[cfg(windows)]
fn install_windows_task(exe: &Path, config: &Path, runtime: &Path) -> Result<(), String> {
    fs::create_dir_all(runtime).map_err(|error| format!("创建 daemon 运行目录失败: {error}"))?;
    let powershell = windows_definition::powershell_executable()?;
    if windows_task_exists() && windows_daemon_process_alive(runtime) {
        stop_windows_task(runtime)?;
    }
    let user_sid = windows_definition::current_user_sid()?;
    let user_name = windows_definition::current_user_name()?;
    let xml_path = runtime.join("axon-daemon.task.xml");
    fs::write(
        &xml_path,
        windows_definition::windows_task_xml_bytes(&powershell, exe, config, runtime, &user_sid),
    )
    .map_err(|error| format!("写入 Windows daemon 任务定义失败: {error}"))?;
    let result = run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/create",
            "/tn",
            WINDOWS_TASK_NAME,
            "/xml",
            xml_path.to_string_lossy().as_ref(),
            "/f",
            "/ru",
            &user_name,
            "/it",
        ]),
        "注册 Windows daemon 用户计划任务",
    );
    let _ = fs::remove_file(&xml_path);
    result?;
    windows_definition::grant_current_user_task_access(&powershell, &user_sid)?;
    start_windows_task(runtime)
}

#[cfg(windows)]
fn windows_task_exists() -> bool {
    non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM)
        .args(["/query", "/tn", WINDOWS_TASK_NAME])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// 守护进程是否仍在运行。
///
/// 判据是「`daemon.pid` 指向的 PID 存活且映像名是 Axon」，而不是计划任务状态：
/// 计划任务实例可能在守护进程仍存活时就已结束（提权进程不随外壳退出），只看任务会得出相反结论。
#[cfg(windows)]
fn windows_daemon_process_alive(runtime_dir: &Path) -> bool {
    daemon_pid(runtime_dir)
        .is_some_and(|pid| process_alive(pid) && windows_definition::process_image_is_axon(pid))
}

#[cfg(windows)]
fn start_windows_task(runtime_dir: &Path) -> Result<(), String> {
    if windows_daemon_process_alive(runtime_dir) {
        return Ok(());
    }
    run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/change",
            "/tn",
            WINDOWS_TASK_NAME,
            "/enable",
        ]),
        "启用 Windows daemon 用户计划任务",
    )?;
    run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/run",
            "/tn",
            WINDOWS_TASK_NAME,
        ]),
        "启动 Windows daemon 用户计划任务",
    )?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(PROCESS_START_TIMEOUT_MS);
    while !windows_daemon_process_alive(runtime_dir) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(PROCESS_STOP_POLL_MS));
    }
    if windows_daemon_process_alive(runtime_dir) {
        Ok(())
    } else {
        Err(
            "Windows daemon 用户计划任务未在启动截止时间内运行，请检查用户任务历史与 daemon 日志"
                .to_owned(),
        )
    }
}

#[cfg(windows)]
fn stop_windows_task(runtime_dir: &Path) -> Result<(), String> {
    let Some(pid) = daemon_pid(runtime_dir) else {
        super::daemon_stop_request::clear(runtime_dir);
        return Ok(());
    };
    // 1) 先停自动恢复：否则任务可能在停止过程中把守护进程重新拉起。
    run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/change",
            "/tn",
            WINDOWS_TASK_NAME,
            "/disable",
        ]),
        "暂停 Windows daemon 用户计划任务自动恢复",
    )?;
    // 2) 首选「请求退出」：守护进程以 HighestAvailable 运行，普通用户结束不了它的进程，
    //    但它可以自己 drain 收口退出，这条路径与调用方权限无关。
    super::daemon_stop_request::request(runtime_dir)?;
    if windows_definition::wait_for_process_exit(pid, GRACEFUL_STOP_TIMEOUT_MS) {
        super::daemon_stop_request::clear(runtime_dir);
        return Ok(());
    }
    // 3) 兜底一：结束计划任务实例。它只结束 pwsh 外壳，未必带走提权后的守护进程。
    let task_end_error = run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/end",
            "/tn",
            WINDOWS_TASK_NAME,
        ]),
        "停止 Windows daemon 用户计划任务",
    )
    .err();
    if windows_definition::wait_for_process_exit(pid, PROCESS_STOP_TIMEOUT_MS) {
        super::daemon_stop_request::clear(runtime_dir);
        return Ok(());
    }
    // 4) 兜底二：强制结束。先核对映像名：PID 可能已被回收给无关进程，
    //    而 taskkill /T 会连它的整棵子进程树一起带走。
    if !windows_definition::process_image_is_axon(pid) {
        super::daemon_stop_request::clear(runtime_dir);
        return Err(format!(
            "守护进程未响应停止请求，且 PID {pid} 已不再是 Axon 进程，未执行强制结束；请检查 data/logs 日志"
        ));
    }
    let force_error = run(
        non_interactive_command("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]),
        "强制结束 Windows daemon 用户进程",
    )
    .err();
    if windows_definition::wait_for_process_exit(pid, PROCESS_STOP_TIMEOUT_MS) {
        super::daemon_stop_request::clear(runtime_dir);
        return Ok(());
    }
    super::daemon_stop_request::clear(runtime_dir);
    let detail = force_error
        .or(task_end_error)
        .unwrap_or_else(|| "未返回失败原因".to_owned());
    Err(format!(
        "守护进程 {pid} 未在截止时间内停止；最后一次失败原因：{detail}。若该进程以管理员权限运行，请在管理员终端重试"
    ))
}

#[cfg(windows)]
fn daemon_pid(runtime_dir: &Path) -> Option<u32> {
    fs::read_to_string(daemon_pid_path(runtime_dir))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn platform_definition_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME").map(PathBuf::from).map(|home| {
        home.join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist"))
    });
    #[cfg(target_os = "linux")]
    return std::env::var_os("HOME").map(PathBuf::from).map(|home| {
        home.join(".config/systemd/user")
            .join(format!("{SERVICE_NAME}.service"))
    });
}

#[cfg(target_os = "macos")]
fn launchd_domain() -> String {
    let output = non_interactive_command("id").arg("-u").output().ok();
    let uid = output
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    format!("gui/{uid}")
}

#[cfg(target_os = "linux")]
fn systemctl(args: &[&str]) -> Result<(), String> {
    run(
        non_interactive_command("systemctl")
            .arg("--user")
            .args(args),
        "执行 systemd daemon 操作",
    )
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

#[cfg(windows)]
fn daemon_pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(DAEMON_PID_FILE_NAME)
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn command_success(command: &mut Command) -> bool {
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[path = "daemon_service_definitions.rs"]
mod service_definitions;
#[cfg(any(target_os = "macos", test))]
use service_definitions::launchd_plist;
#[cfg(any(target_os = "linux", test))]
use service_definitions::systemd_unit;

#[path = "daemon_service_windows.rs"]
mod windows_definition;
#[cfg(test)]
use windows_definition::{parse_windows_user_sid, windows_task_xml};

#[cfg(test)]
#[path = "daemon_service_manager_tests.rs"]
mod tests;
