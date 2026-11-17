//! Windows 当前用户计划任务形式的 daemon 服务生命周期。
//!
//! 注册、启用、启动与停止都经 Task Scheduler 的 `schtasks.exe` 下发，任务定义本身由
//! `daemon_service_windows` 子模块生成。停止分三级兜底：先请求退出让守护进程自行收口，
//! 再结束任务实例，最后在核对映像名后强制结束。POSIX 实现见 `daemon_service_unix` 子模块。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use axon_process::non_interactive_command;

use super::windows_definition;
use super::{
    GRACEFUL_STOP_TIMEOUT_MS, PROCESS_START_TIMEOUT_MS, PROCESS_STOP_POLL_MS,
    PROCESS_STOP_TIMEOUT_MS, daemon_pid_path, daemon_process_alive, run,
};
use crate::daemon_commands::daemon_stop_request;

pub(super) fn install_windows_task(
    exe: &Path,
    config: &Path,
    runtime: &Path,
) -> Result<(), String> {
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
            windows_definition::WINDOWS_TASK_NAME,
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

pub(super) fn uninstall_windows_task(runtime_dir: &Path) -> Result<(), String> {
    if windows_task_exists() {
        stop_windows_task(runtime_dir)?;
        run(
            non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
                "/delete",
                "/tn",
                windows_definition::WINDOWS_TASK_NAME,
                "/f",
            ]),
            "删除 Windows daemon 计划任务",
        )?;
    }
    Ok(())
}

pub(super) fn windows_task_exists() -> bool {
    non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM)
        .args(["/query", "/tn", windows_definition::WINDOWS_TASK_NAME])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// 守护进程是否仍在运行。
///
/// 判据是「`daemon.pid` 指向的 PID 仍由 Axon 进程持有」，而不是计划任务状态：
/// 计划任务实例可能在守护进程仍存活时就已结束（提权进程不随外壳退出），只看任务会得出相反结论。
pub(super) fn windows_daemon_process_alive(runtime_dir: &Path) -> bool {
    daemon_pid(runtime_dir).is_some_and(daemon_process_alive)
}

pub(super) fn start_windows_task(runtime_dir: &Path) -> Result<(), String> {
    // 先恢复任务本身再判断进程：上一条 stop 若停在停进程阶段失败，任务会留在 disabled，
    // 此后「进程还活着就早返回」会让自动恢复与登录触发器永久失效，用户只看到一次成功启动。
    let enable = run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/change",
            "/tn",
            windows_definition::WINDOWS_TASK_NAME,
            "/enable",
        ]),
        "启用 Windows daemon 用户计划任务",
    );
    if windows_daemon_process_alive(runtime_dir) {
        return Ok(());
    }
    enable?;
    run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/run",
            "/tn",
            windows_definition::WINDOWS_TASK_NAME,
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
        // 任务起来了但进程立刻退出时，光说「超时」等于让人去翻日志：把最近一条错误
        // 直接写进结论，版本不符这类问题当场就能看到该做什么。
        let cause = daemon_log_error(runtime_dir);
        Err(match cause {
            Some(cause) => format!(
                "Windows daemon 用户计划任务未在启动截止时间内运行，daemon 日志最后一条错误：{cause}"
            ),
            None => {
                "Windows daemon 用户计划任务未在启动截止时间内运行，请检查用户任务历史与 daemon 日志"
                    .to_owned()
            }
        })
    }
}

/// 最近一份 daemon 日志里的最后一条错误行。
fn daemon_log_error(runtime_dir: &Path) -> Option<String> {
    const LOG_TAIL_BYTES: u64 = 256 * 1024;
    let logs: Vec<PathBuf> = fs::read_dir(runtime_dir.join("logs"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .collect();
    // 取修改时间最新的一份：按文件名排序只在「一天一个文件」的命名下成立，
    // 同层一旦出现第二类日志就会取错对象。
    let path = logs
        .into_iter()
        .max_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok())?;
    let length = fs::metadata(&path).ok()?.len();
    let mut file = fs::File::open(&path).ok()?;
    let truncated = length > LOG_TAIL_BYTES;
    if truncated {
        use std::io::Seek as _;
        file.seek(std::io::SeekFrom::End(-(LOG_TAIL_BYTES as i64)))
            .ok()?;
    }
    let mut raw = Vec::new();
    {
        use std::io::Read as _;
        file.read_to_end(&mut raw).ok()?;
    }
    // 尾窗口的起点可能切在多字节字符中间：这里按字节读入再做有损解码，
    // 否则一次截断就会让整条可观测性链路退化成通用的「请检查日志」。
    let text = String::from_utf8_lossy(&raw);
    let text = if truncated {
        text.split_once('\n')
            .map_or(text.as_ref(), |(_, rest)| rest)
    } else {
        text.as_ref()
    };
    text.lines()
        .rev()
        .find(|line| line.contains("[ERROR]"))
        .map(|line| line.trim().to_owned())
}

pub(super) fn stop_windows_task(runtime_dir: &Path) -> Result<(), String> {
    let Some(pid) = daemon_pid(runtime_dir) else {
        daemon_stop_request::clear(runtime_dir);
        if !windows_task_exists() {
            return Ok(());
        }
        // 没有 pid 文件不等于没有实例：任务还登记着就结束实例。否则 uninstall 删掉任务定义后，
        // 会留下一个既无人管理、也无法再用 daemon stop 停掉的孤儿进程。
        run(
            non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
                "/end",
                "/tn",
                windows_definition::WINDOWS_TASK_NAME,
            ]),
            "结束 Windows daemon 用户计划任务实例",
        )
        .map_err(|error| format!("未找到 daemon.pid，且结束计划任务实例失败：{error}"))?;
        return Ok(());
    };
    // 1) 先停自动恢复：否则任务可能在停止过程中把守护进程重新拉起。
    run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/change",
            "/tn",
            windows_definition::WINDOWS_TASK_NAME,
            "/disable",
        ]),
        "暂停 Windows daemon 用户计划任务自动恢复",
    )?;
    // 2) 首选「请求退出」：守护进程以 HighestAvailable 运行，普通用户结束不了它的进程，
    //    但它可以自己 drain 收口退出，这条路径与调用方权限无关。
    daemon_stop_request::request(runtime_dir)?;
    if windows_definition::wait_for_process_exit(pid, GRACEFUL_STOP_TIMEOUT_MS) {
        daemon_stop_request::clear(runtime_dir);
        return Ok(());
    }
    // 3) 兜底一：结束计划任务实例。它只结束 pwsh 外壳，未必带走提权后的守护进程。
    let task_end_error = run(
        non_interactive_command(windows_definition::WINDOWS_TASK_PROGRAM).args([
            "/end",
            "/tn",
            windows_definition::WINDOWS_TASK_NAME,
        ]),
        "停止 Windows daemon 用户计划任务",
    )
    .err();
    if windows_definition::wait_for_process_exit(pid, PROCESS_STOP_TIMEOUT_MS) {
        daemon_stop_request::clear(runtime_dir);
        return Ok(());
    }
    // 4) 兜底二：强制结束。先核对映像名：PID 可能已被回收给无关进程，
    //    而 taskkill /T 会连它的整棵子进程树一起带走。
    if !windows_definition::process_image_is_daemon(pid) {
        daemon_stop_request::clear(runtime_dir);
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
        daemon_stop_request::clear(runtime_dir);
        return Ok(());
    }
    daemon_stop_request::clear(runtime_dir);
    let detail = force_error
        .or(task_end_error)
        .unwrap_or_else(|| "未返回失败原因".to_owned());
    Err(format!(
        "守护进程 {pid} 未在截止时间内停止；最后一次失败原因：{detail}。若该进程以管理员权限运行，请在管理员终端重试"
    ))
}

fn daemon_pid(runtime_dir: &Path) -> Option<u32> {
    fs::read_to_string(daemon_pid_path(runtime_dir))
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
}
