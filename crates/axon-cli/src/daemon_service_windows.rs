//! Windows 当前用户 Task Scheduler 定义辅助。

#[cfg(windows)]
use axon_process::non_interactive_command;
#[cfg(any(windows, test))]
use base64::{Engine as _, engine::general_purpose::STANDARD};
#[cfg(any(windows, test))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

#[cfg(any(windows, test))]
pub(super) const WINDOWS_TASK_NAME: &str = "AxonDaemon";
#[cfg(windows)]
pub(super) const WINDOWS_TASK_PROGRAM: &str = "schtasks.exe";

#[cfg(any(windows, test))]
const RESTART_INTERVAL: &str = "PT5M";
#[cfg(any(windows, test))]
const RESTART_COUNT: &str = "3";
#[cfg(windows)]
const WINDOWS_POWERSHELL_PROGRAM: &str = "pwsh.exe";
#[cfg(any(windows, test))]
const POWERSHELL_OUTPUT_SETUP: &str = "$utf8=[Text.UTF8Encoding]::new($false); $OutputEncoding=$utf8; [Console]::OutputEncoding=$utf8; [Console]::InputEncoding=$utf8; $PSStyle.OutputRendering='PlainText'; $ErrorActionPreference='Stop'";
#[cfg(any(windows, test))]
const WINDOWS_TASK_RUN_LEVEL: &str = "HighestAvailable";

#[cfg(windows)]
pub(super) fn current_user_sid() -> Result<String, String> {
    let output = non_interactive_command("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .map_err(|error| format!("读取当前用户 SID 失败: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "读取当前用户 SID 失败，whoami 退出码：{:?}",
            output.status.code()
        ));
    }
    parse_windows_user_sid(&super::decode_command_output(&output.stdout))
        .ok_or_else(|| "无法从 whoami 输出解析当前用户 SID".to_owned())
}

#[cfg(windows)]
pub(super) fn current_user_name() -> Result<String, String> {
    let output = non_interactive_command("whoami")
        .output()
        .map_err(|error| format!("读取当前用户名失败: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "读取当前用户名失败，whoami 退出码：{:?}",
            output.status.code()
        ));
    }
    let user_name = super::decode_command_output(&output.stdout)
        .trim()
        .to_owned();
    if user_name.is_empty() {
        Err("whoami 未返回当前用户名".to_owned())
    } else {
        Ok(user_name)
    }
}

/// 保留登录用户身份，使用该用户可获得的最高权限运行。
#[cfg(any(windows, test))]
pub(super) fn windows_task_xml(
    powershell: &Path,
    exe: &Path,
    config: &Path,
    runtime: &Path,
    user_sid: &str,
) -> String {
    let root = runtime.parent().unwrap_or(runtime);
    let command = xml_escape(&powershell.to_string_lossy());
    let encoded_command = encode_powershell(&windows_daemon_script(exe, config, root));
    let arguments =
        format!("-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {encoded_command}");
    let working_directory = xml_escape(&root.to_string_lossy());
    let sid = xml_escape(user_sid);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Axon 用户 Daemon</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>{WINDOWS_TASK_RUN_LEVEL}</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <StartWhenAvailable>true</StartWhenAvailable>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>{RESTART_INTERVAL}</Interval><Count>{RESTART_COUNT}</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author"><Exec><Command>{command}</Command><Arguments>{arguments}</Arguments><WorkingDirectory>{working_directory}</WorkingDirectory></Exec></Actions>
</Task>
"#
    )
}

#[cfg(any(windows, test))]
fn windows_daemon_script(exe: &Path, config: &Path, root: &Path) -> String {
    format!(
        "{POWERSHELL_OUTPUT_SETUP}\n& '{}' daemon run --root '{}' --config-path '{}' | Out-Host\nexit $LASTEXITCODE",
        powershell_quote(exe),
        powershell_quote(root),
        powershell_quote(config)
    )
}

#[cfg(any(windows, test))]
fn powershell_quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[cfg(any(windows, test))]
fn encode_powershell(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    STANDARD.encode(bytes)
}

#[cfg(windows)]
pub(super) fn windows_task_xml_bytes(
    powershell: &Path,
    exe: &Path,
    config: &Path,
    runtime: &Path,
    user_sid: &str,
) -> Vec<u8> {
    let xml = windows_task_xml(powershell, exe, config, runtime, user_sid);
    let mut bytes = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[cfg(windows)]
pub(super) fn grant_current_user_task_access(
    powershell: &Path,
    user_sid: &str,
) -> Result<(), String> {
    let script = format!(
        "$service=New-Object -ComObject Schedule.Service; $service.Connect(); $task=$service.GetFolder('\\').GetTask('{WINDOWS_TASK_NAME}'); $task.SetSecurityDescriptor(\"D:(A;;FA;;;{user_sid})(A;;FA;;;BA)(A;;FA;;;SY)\", 0)"
    );
    let encoded = encode_powershell(&script);
    let status = non_interactive_command(powershell)
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .status()
        .map_err(|error| format!("设置 Windows daemon 任务权限失败: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "设置 Windows daemon 任务权限失败，退出码：{:?}",
            status.code()
        ))
    }
}

/// 仅在任务注册时解析并验证主机，运行时直接使用绝对路径。
#[cfg(windows)]
pub(super) fn powershell_executable() -> Result<PathBuf, String> {
    let output = non_interactive_command("where.exe")
        .arg(WINDOWS_POWERSHELL_PROGRAM)
        .output()
        .map_err(|error| format!("查找 PowerShell 7 失败: {error}"))?;
    let path = super::decode_command_output(&output.stdout)
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .find(|path| path.is_absolute() && path.is_file())
        .ok_or_else(|| "未找到 pwsh.exe，请安装 PowerShell 7 并加入 PATH".to_owned())?;
    let version = non_interactive_command(&path)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$PSVersionTable.PSVersion.Major",
        ])
        .output()
        .map_err(|error| format!("验证 PowerShell 版本失败: {error}"))?;
    if !version.status.success()
        || !String::from_utf8_lossy(&version.stdout)
            .trim()
            .parse::<u32>()
            .is_ok_and(|major| major >= 7)
    {
        return Err("Daemon 后台主机必须为 PowerShell 7 或更高版本".to_owned());
    }
    Ok(path)
}

#[cfg(any(windows, test))]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 从 `whoami /user /fo csv /nh` 输出中提取 SID；格式异常时返回 None。
#[cfg(any(windows, test))]
pub(super) fn parse_windows_user_sid(output: &str) -> Option<String> {
    output
        .split(|character: char| character == ',' || character == '"' || character.is_whitespace())
        .find(|field| is_sid(field))
        .map(str::to_owned)
}

#[cfg(any(windows, test))]
fn is_sid(field: &str) -> bool {
    field.strip_prefix("S-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .split('-')
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
    })
}

/// 等待目标进程退出；返回是否已在截止时间内退出。
///
/// 停止路径分两级等待：先等优雅退出，再等兜底手段生效，因此超时由调用方给出。
#[cfg(windows)]
pub(super) fn wait_for_process_exit(pid: u32, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while super::process_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(
            super::PROCESS_STOP_POLL_MS,
        ));
    }
    !super::process_alive(pid)
}

/// 目标 PID 当前是否确实是 Axon 进程。
///
/// `daemon.pid` 里的 PID 可能已被系统回收给无关进程；强行结束会误杀别人的进程树
/// （`taskkill /T` 会连子进程一起带走），因此强制结束前必须核对映像名。
#[cfg(windows)]
pub(super) fn process_image_is_axon(pid: u32) -> bool {
    super::process_image_name(pid).is_some_and(|image| image.to_ascii_lowercase().contains("axon"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_escapes_paths_and_contains_user_restart_policy() {
        let xml = windows_task_xml(
            Path::new(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            Path::new(r"C:\Axon & Tool\axon.exe"),
            Path::new(r"C:\Users\tester\config & data.toml"),
            Path::new(r"C:\Users\tester\.axon\data"),
            "S-1-5-21-1-2-3-1001",
        );
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<RestartOnFailure>"));
        assert!(xml.contains("<StartWhenAvailable>true</StartWhenAvailable>"));
        assert!(xml.contains(r"<Command>C:\Program Files\PowerShell\7\pwsh.exe</Command>"));
        assert!(xml.contains("-WindowStyle Hidden -EncodedCommand"));
        let script = windows_daemon_script(
            Path::new(r"C:\Axon & Tool\axon.exe"),
            Path::new(r"C:\Users\tester\config & data.toml"),
            Path::new(r"C:\Users\tester\.axon"),
        );
        assert!(script.contains("daemon run"));
        assert!(script.contains(" | Out-Host\nexit $LASTEXITCODE"));
        assert!(script.contains(r"C:\Axon & Tool\axon.exe"));
        assert!(script.contains("[Text.UTF8Encoding]::new($false)"));
        assert!(script.contains("$PSStyle.OutputRendering='PlainText'"));
        assert!(script.ends_with("exit $LASTEXITCODE"));
    }

    #[test]
    fn encoded_script_preserves_quotes_and_utf8_output_contract() {
        let script = windows_daemon_script(
            Path::new(r"C:\Axon's Tool\axon.exe"),
            Path::new(r"C:\用户\config.toml"),
            Path::new(r"C:\用户\.axon"),
        );
        assert!(script.contains("Axon''s Tool"));
        let bytes = STANDARD.decode(encode_powershell(&script)).unwrap();
        let units = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        assert_eq!(String::from_utf16(&units).unwrap(), script);
        assert!(script.contains("[Console]::OutputEncoding=$utf8"));
        assert!(script.contains("[Console]::InputEncoding=$utf8"));
    }

    #[cfg(windows)]
    #[test]
    fn task_xml_bytes_match_utf16_declaration() {
        let host = Path::new(r"C:\PowerShell\pwsh.exe");
        let exe = Path::new(r"C:\Axon\axon.exe");
        let root = Path::new(r"C:\Axon\data");
        let sid = "S-1-5-21-1-2-3-1001";
        let bytes = windows_task_xml_bytes(host, exe, exe, root, sid);
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            windows_task_xml(host, exe, exe, root, sid)
        );
    }
}
