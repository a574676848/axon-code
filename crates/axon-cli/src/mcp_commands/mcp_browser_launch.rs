//! 使用系统默认浏览器打开 MCP OAuth 授权页。
//!
//! CLI 与交互式 TUI 共用同一实现：授权 URL 必须在阻塞等待本地回调之前交给浏览器，
//! 否则用户永远拿不到回调地址。

use std::process::{Command, Stdio};

/// Windows 下创建进程时不弹控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 使用系统默认浏览器打开授权 URL。
///
/// 只负责拉起浏览器，不等待其退出。
///
/// # Errors
/// 启动命令无法拉起时返回可读错误。
pub(crate) fn open_authorization_url(url: &str) -> Result<(), String> {
    let mut command = browser_command(url);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("无法启动系统浏览器，请手动打开授权 URL：{error}"))
}

#[cfg(windows)]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("rundll32.exe");
    command.args(["url.dll,FileProtocolHandler", url]);
    command
}

#[cfg(target_os = "macos")]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(all(unix, not(target_os = "macos")))]
fn browser_command(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}
