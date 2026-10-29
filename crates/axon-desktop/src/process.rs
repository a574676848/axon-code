//! 子进程启动：统一隐藏控制台窗体。
//!
//! Windows 上以 `std::process::Command` 启动控制台程序（`cmd`、`powershell`、
//! `pwsh`、`axon.exe` 等）时，系统会为子进程新建一个控制台窗体。
//! 桌面端每次探测或打开目标都这么做，用户就会看到「终端窗体一闪而过」。
//! 因此本模块是**唯一**的子进程启动入口：一律带上 `CREATE_NO_WINDOW`
//! 创建标志，不再让各处自行拼 `Command`。

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// 应用句柄；由组合根在启动时注入。
///
/// 官方打开插件需要 `AppHandle`，而调试桥模式下没有窗口因此也没有句柄，
/// 所以这里按「有句柄走插件、无句柄走平台命令」分流。
static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// 注入应用句柄，供打开目标与目录选择调用官方插件。
pub fn install_opener(app: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app);
}

/// 调起系统目录选择对话框。
///
/// 走官方插件：对话框挂在应用窗口上，因此是**窗口模态**的，
/// 系统里点应用其他位置不会留下一个悬空对话框；
/// 调试桥没有窗口句柄，回落到平台命令。
pub fn pick_directory(initial: Option<&str>) -> Result<Option<String>, String> {
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_dialog::DialogExt;
        let mut builder = app.dialog().file();
        if let Some(path) = initial.filter(|value| !value.is_empty()) {
            builder = builder.set_directory(path);
        }
        let picked = builder.blocking_pick_folder();
        return Ok(picked
            .and_then(|selected| selected.into_path().ok())
            .map(|path| path.to_string_lossy().to_string()));
    }
    crate::picker::pick_directory_native(initial)
}

/// 调起系统文件选择对话框，返回选中的绝对路径。
///
/// 与目录选择同源：窗口存在时走官方插件的窗口模态对话框，
/// 调试桥没有窗口句柄时回落平台命令。
pub fn pick_files(initial: Option<&str>) -> Result<Vec<String>, String> {
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_dialog::DialogExt;
        let mut builder = app.dialog().file().set_title("选择要附加给 Axon 的文件");
        if let Some(path) = initial.filter(|value| !value.is_empty()) {
            builder = builder.set_directory(path);
        }
        let picked = builder.blocking_pick_files().unwrap_or_default();
        return Ok(picked
            .into_iter()
            .filter_map(|selected| selected.into_path().ok())
            .map(|path| path.to_string_lossy().to_string())
            .collect());
    }
    crate::picker::pick_files_native(initial)
}

/// 不新建控制台窗体的创建标志。
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 隐去控制台窗体。
///
/// 非 Windows 平台无须处理，直接返回原命令，调用方不必分平台分支。
pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// 运行并等待输出，全程不显示控制台窗体。
pub fn run_output(command: &mut Command) -> std::io::Result<Output> {
    hide_console(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
}

/// 启动后立即返回，不等待、不接管标准流，全程不显示控制台窗体。
///
/// 适用于「打开文件、打开目录」这类结果由外部程序负责的动作。
pub fn spawn_detached(command: &mut Command) -> std::io::Result<()> {
    hide_console(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// 交给系统 Shell 的路径。
///
/// Windows 的 Shell（`ShellExecute` 系列与 `explorer.exe`）不接受 Rust 规范化后的
/// `\\?\` 扩展长度前缀：它只认盘符或共享路径，带前缀会被判成参数错误。文件浏览给出的
/// 正是规范化路径（[`crate::workspace_files::resolve_within`]），因此交出去之前统一还原。
///
/// 其他平台原样返回 `OsStr`：POSIX 文件名不保证是 UTF-8，经字符串中转会把非法字节换成
/// 替换字符，交给 `open` / `xdg-open` 的就不再是原路径了。
fn shell_path(path: &Path) -> std::ffi::OsString {
    #[cfg(target_os = "windows")]
    {
        std::ffi::OsString::from(crate::workspace::strip_extended_prefix(
            &path.to_string_lossy(),
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        path.as_os_str().to_os_string()
    }
}

/// 用系统默认程序打开文件或目录。
///
/// 优先走官方打开插件（其在 Windows 上直接调用系统 shell API，不经控制台，
/// 与桌面应用自身的「打开方式」行为一致）；仅在调试桥没有应用句柄时才回落到
/// 平台命令。
pub fn open_path(path: &Path) -> Result<(), String> {
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_opener::OpenerExt;
        return app
            .opener()
            .open_path(
                shell_path(path).to_string_lossy().into_owned(),
                None::<&str>,
            )
            .map_err(|error| format!("无法打开目标：{error}"));
    }
    platform_open(path)
}

/// 用指定程序打开文件或目录。
///
/// 程序由后端从精选目录解析后传进来，界面只交出标识，因此界面无法借这个入口
/// 启动任意程序；仍走官方打开插件，在 Windows 上由系统 shell 派生进程。
pub fn open_with(program: &str, path: &Path) -> Result<(), String> {
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_opener::OpenerExt;
        return app
            .opener()
            .open_path(
                shell_path(path).to_string_lossy().into_owned(),
                Some(program.to_string()),
            )
            .map_err(|error| format!("无法用所选应用打开：{error}"));
    }
    platform_open_with(program, path)
}

/// 在系统文件管理器中定位并选中目标。
pub fn reveal(path: &Path) -> Result<(), String> {
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_opener::OpenerExt;
        return app
            .opener()
            .reveal_item_in_dir(shell_path(path))
            .map_err(|error| format!("无法在文件管理器中定位：{error}"));
    }
    platform_reveal(path)
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "windows")]
fn platform_open(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("目标不存在：{}", path.display()));
    }
    spawn_detached(Command::new("explorer").arg(shell_path(path)))
        .map_err(|error| format!("无法打开目标：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "macos")]
fn platform_open(path: &Path) -> Result<(), String> {
    spawn_detached(Command::new("open").arg(shell_path(path)))
        .map_err(|error| format!("无法打开目标：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "linux")]
fn platform_open(path: &Path) -> Result<(), String> {
    spawn_detached(Command::new("xdg-open").arg(shell_path(path)))
        .map_err(|error| format!("无法打开目标：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "windows")]
fn platform_open_with(program: &str, path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("目标不存在：{}", path.display()));
    }
    spawn_detached(Command::new(program).arg(shell_path(path)))
        .map_err(|error| format!("无法用所选应用打开：{error}"))
}

/// 调试桥模式下的平台回落实现。
///
/// `explorer` 的 `/select` 需要一个参数，逗号后直接跟路径，中间不能有空格。
#[cfg(target_os = "windows")]
fn platform_reveal(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("目标不存在：{}", path.display()));
    }
    let mut command = Command::new("explorer");
    command.arg(format!("/select,{}", shell_path(path).to_string_lossy()));
    spawn_detached(&mut command).map_err(|error| format!("无法在文件管理器中定位：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "macos")]
fn platform_open_with(program: &str, path: &Path) -> Result<(), String> {
    spawn_detached(Command::new(program).arg(shell_path(path)))
        .map_err(|error| format!("无法用所选应用打开：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "macos")]
fn platform_reveal(path: &Path) -> Result<(), String> {
    spawn_detached(Command::new("open").arg("-R").arg(path))
        .map_err(|error| format!("无法在文件管理器中定位：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "linux")]
fn platform_open_with(program: &str, path: &Path) -> Result<(), String> {
    spawn_detached(Command::new(program).arg(path))
        .map_err(|error| format!("无法用所选应用打开：{error}"))
}

/// 调试桥模式下的平台回落实现：没有统一的「定位并选中」协议，退回到打开所在目录。
#[cfg(target_os = "linux")]
fn platform_reveal(path: &Path) -> Result<(), String> {
    let directory = path.parent().unwrap_or(path);
    spawn_detached(Command::new("xdg-open").arg(directory))
        .map_err(|error| format!("无法在文件管理器中定位：{error}"))
}

/// 允许在系统浏览器中打开的外部协议。
///
/// 入口只接受后端自己派生的地址，这里仍做一次白名单校验：这个命令一旦被复用到
/// 别处，也不会变成「用任意协议拉起本机程序」的通道。
const EXTERNAL_URL_SCHEMES: [&str; 2] = ["http", "https"];

/// 解析并校验一个待打开的外部地址。
fn parse_external_url(url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url.trim()).map_err(|error| format!("地址无法解析：{error}"))?;
    if !EXTERNAL_URL_SCHEMES.contains(&parsed.scheme()) {
        return Err(format!(
            "只允许打开 http 与 https 地址，收到的是 {}",
            parsed.scheme()
        ));
    }
    Ok(parsed)
}

/// Tauri IPC 命令：用系统默认浏览器打开一个外部地址。
///
/// 与打开文件同源：有应用句柄时走官方打开插件，调试桥没有句柄时回落到平台命令。
#[tauri::command(async)]
pub fn open_external_url(url: String) -> Result<(), String> {
    let parsed = parse_external_url(&url)?;
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_opener::OpenerExt;
        return app
            .opener()
            .open_url(parsed.to_string(), None::<&str>)
            .map_err(|error| format!("无法打开外部地址：{error}"));
    }
    platform_open_url(parsed.as_str())
}

/// 打开受控的系统设置协议地址（仅限宿主内部白名单常量调用，不向前端开放任意 URI）。
pub(crate) fn open_system_uri(uri: &'static str) -> Result<(), String> {
    if let Some(app) = APP_HANDLE.get() {
        use tauri_plugin_opener::OpenerExt;
        if app.opener().open_url(uri.to_string(), None::<&str>).is_ok() {
            return Ok(());
        }
    }
    platform_open_url(uri)
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "windows")]
fn platform_open_url(url: &str) -> Result<(), String> {
    spawn_detached(Command::new("explorer").arg(url))
        .map_err(|error| format!("无法打开外部地址：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "macos")]
fn platform_open_url(url: &str) -> Result<(), String> {
    spawn_detached(Command::new("open").arg(url))
        .map_err(|error| format!("无法打开外部地址：{error}"))
}

/// 调试桥模式下的平台回落实现。
#[cfg(target_os = "linux")]
fn platform_open_url(url: &str) -> Result<(), String> {
    spawn_detached(Command::new("xdg-open").arg(url))
        .map_err(|error| format!("无法打开外部地址：{error}"))
}

#[cfg(test)]
mod tests {
    use super::parse_external_url;

    #[test]
    fn external_url_accepts_only_http_family() {
        assert_eq!(
            parse_external_url(" https://github.com/a574676848/axon-code/releases ")
                .unwrap()
                .as_str(),
            "https://github.com/a574676848/axon-code/releases"
        );
        assert!(parse_external_url("http://127.0.0.1:3143/").is_ok());
    }

    #[test]
    fn external_url_rejects_local_handlers_and_garbage() {
        for rejected in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "",
        ] {
            assert!(
                parse_external_url(rejected).is_err(),
                "{rejected} 不应被接受"
            );
        }
    }
}
