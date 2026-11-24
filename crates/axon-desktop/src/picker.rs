//! 原生目录选择器。
//!
//! Tauri 2 的官方对话框插件不在本 crate 的依赖白名单内，
//! 因此这里按平台调用系统自带能力，保持零新增依赖且三平台可用：
//! Windows 用 `FolderBrowserDialog`，macOS 用 `osascript`，Linux 用 `zenity`。
//!
//! 三态语义必须区分清楚：
//! - `Ok(Some(path))`：用户选定了一个目录；
//! - `Ok(None)`：用户主动取消，属于正常操作；
//! - `Err(..)`：当前系统没有可用的选择器，属于能力缺失，界面需要如实告知。
//!
//! Windows 分支把脚本以 UTF-16LE 编码后经 `-EncodedCommand` 传入，
//! 避免中文提示语在命令行参数传递中被按本地代码页误解码。

use std::process::Command;

/// 选择器提示语。
const PICKER_PROMPT: &str = "选择工作区目录";

/// 从命令输出中取第一行非空文本。
fn first_non_empty_line(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(ToString::to_string)
}

/// 把脚本编码为 PowerShell 的 `-EncodedCommand` 参数（UTF-16LE 加 Base64）。
#[cfg(target_os = "windows")]
fn encode_powershell(script: &str) -> String {
    /// Base64 标准字母表。
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).map_or(0, |value| u32::from(*value));
        let b2 = chunk.get(2).map_or(0, |value| u32::from(*value));
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(target_os = "windows")]
/// 使用 PowerShell 调起系统目录对话框。
///
/// 仅在调试桥没有窗口句柄时使用；真实窗口走官方插件的窗口模态对话框。
pub fn pick_directory_native(initial: Option<&str>) -> Result<Option<String>, String> {
    let selected = initial
        .map(|value| value.replace('\'', "''"))
        .unwrap_or_default();
    let script = format!(
        r#"
Add-Type -AssemblyName System.Windows.Forms | Out-Null
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = '{PICKER_PROMPT}'
$dialog.UseDescriptionForTitle = $true
$dialog.ShowNewFolderButton = $true
$initial = '{selected}'
if ($initial -ne '' -and (Test-Path $initial)) {{ $dialog.SelectedPath = $initial }}
if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{
    [Console]::Out.Write($dialog.SelectedPath)
}}
"#
    );

    let encoded = encode_powershell(&script);
    // 目录选择器由 PowerShell 调起系统对话框；必须隐藏控制台，
    // 否则点击「添加工作区」会先闪出一个终端窗体。
    let output = crate::process::run_output(Command::new("powershell").args([
        "-NoProfile",
        "-STA",
        "-EncodedCommand",
        &encoded,
    ]))
    .map_err(|error| format!("目录选择器启动失败：{error}"))?;
    Ok(first_non_empty_line(&output.stdout))
}

/// macOS 原生目录选择器：经 osascript 调起系统对话框。
#[cfg(target_os = "macos")]
pub fn pick_directory_native(initial: Option<&str>) -> Result<Option<String>, String> {
    let location = match initial.filter(|value| !value.is_empty()) {
        Some(path) => format!(" default location POSIX file \"{path}\""),
        None => String::new(),
    };
    let script = format!("POSIX path of (choose folder with prompt \"{PICKER_PROMPT}\"{location})");
    let output = crate::process::run_output(Command::new("osascript").args(["-e", &script]))
        .map_err(|_| "当前系统未提供可用的目录选择器".to_string())?;
    Ok(first_non_empty_line(&output.stdout))
}

/// 其它 Unix 平台的原生目录选择器：经 zenity 调起对话框。
#[cfg(all(unix, not(target_os = "macos")))]
pub fn pick_directory_native(initial: Option<&str>) -> Result<Option<String>, String> {
    let prompt = format!("--title={PICKER_PROMPT}");
    let mut command = Command::new("zenity");
    command.args(["--file-selection", "--directory", &prompt]);
    if let Some(path) = initial.filter(|value| !value.is_empty()) {
        command.arg(format!("--filename={path}/"));
    }
    let output = crate::process::run_output(&mut command)
        .map_err(|_| "当前系统未提供可用的目录选择器".to_string())?;
    Ok(first_non_empty_line(&output.stdout))
}

/// Tauri IPC 命令：打开系统原生目录选择器。
///
/// 会记住上一次选中的目录作为本次的初始位置，减少重复定位。
#[tauri::command]
pub fn pick_directory() -> Result<Option<String>, String> {
    let mut state = crate::state::load_state();
    let selected = crate::process::pick_directory(state.last_workspace_dir.as_deref())?;
    if let Some(path) = selected.as_deref() {
        state.last_workspace_dir = Some(path.to_string());
        crate::state::save_state(&state)?;
    }
    Ok(selected)
}

/// 无窗口场景下的多文件选择。
///
/// 桌面窗口存在时统一走官方插件的窗口模态对话框；只有调试桥这类没有窗口句柄的
/// 场景会落到这里，而多选对话框没有跨平台的命令行等价物，因此如实拒绝。
pub fn pick_files_native(_initial: Option<&str>) -> Result<Vec<String>, String> {
    Err("当前环境没有可用窗口，无法打开文件选择器".to_string())
}
