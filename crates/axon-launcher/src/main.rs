//! Axon 稳定 launcher：只读取 current.json，永不覆盖正在运行的版本文件。
//!
//! 两个入口共用一个版本清单：
//!
//! - `axon-launcher <CLI 参数>`：转发给 `versions/<id>/axon.exe`（命令行）；
//! - `axon-launcher desktop [参数]`：转发给 `versions/<id>/axon-desktop.exe`（工作台界面）。
//!
//! 快捷方式因此只需指向 launcher，升级换目录时不必改快捷方式。
//! 命令行 launcher 使用控制台子系统，用户在已有终端输入 axon 时直接复用当前终端；
//! 桌面入口通过 desktop 参数转发到工作台。

use serde::Deserialize;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    env, io,
    path::{Component, Path, PathBuf},
    process::Command,
};
#[cfg(windows)]
use windows_sys::Win32::System::Console::FreeConsole;

#[derive(Debug, Deserialize)]
struct CurrentManifest {
    #[serde(rename = "versionId")]
    version_id: String,
}

/// 打开桌面工作台的子命令名。
const DESKTOP_SUBCOMMAND: &str = "desktop";

/// Windows 不显示 daemon 控制台窗体。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() -> io::Result<()> {
    let launcher = env::current_exe()?;
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    let desktop = args
        .first()
        .is_some_and(|first| first.to_string_lossy() == DESKTOP_SUBCOMMAND);
    if desktop {
        args.remove(0);
        detach_desktop_console();
    }
    let executable = resolve_target(&launcher, desktop)?;
    let daemon_run = is_daemon_run(&args);
    let mut command = Command::new(executable);
    command.args(&args);
    if desktop {
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    } else {
        command
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
    }
    configure_child(&mut command, daemon_run);
    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn is_daemon_run(args: &[std::ffi::OsString]) -> bool {
    args.first().is_some_and(|value| value == "daemon")
        && args.get(1).is_some_and(|value| value == "run")
}

#[cfg(windows)]
fn detach_desktop_console() {
    unsafe {
        let _ = FreeConsole();
    }
}

#[cfg(not(windows))]
fn detach_desktop_console() {}

fn configure_child(command: &mut Command, daemon_run: bool) {
    #[cfg(windows)]
    if daemon_run {
        command.creation_flags(CREATE_NO_WINDOW);
    }
}

fn resolve_target(launcher: &std::path::Path, desktop: bool) -> io::Result<PathBuf> {
    let root = launcher
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| io::Error::other("无法解析 Axon 安装根目录"))?;
    let manifest: CurrentManifest =
        serde_json::from_slice(&std::fs::read(root.join("current.json"))?)
            .map_err(io::Error::other)?;
    if !is_release_id(&manifest.version_id) {
        return Err(io::Error::other("current.json versionId 无效"));
    }
    #[cfg(windows)]
    let binary_name = if desktop {
        "axon-desktop.exe"
    } else {
        "axon.exe"
    };
    #[cfg(not(windows))]
    let binary_name = if desktop { "axon-desktop" } else { "axon" };
    let target = root
        .join("versions")
        .join(manifest.version_id)
        .join(binary_name);
    target
        .is_file()
        .then_some(target)
        .ok_or_else(|| io::Error::other("current.json 指向的 Axon 版本不存在"))
}

fn is_release_id(value: &str) -> bool {
    matches!(
        Path::new(value).components().next(),
        Some(Component::Normal(_))
    ) && Path::new(value).components().count() == 1
        && !value.contains(':')
}

#[cfg(test)]
mod tests {
    use super::{is_daemon_run, is_release_id, resolve_target};
    use std::ffi::OsString;

    /// 建一份最小安装树：launcher + versions/v1 下的两个二进制 + current.json。
    fn install_tree() -> (axon_test_support::TestTempDir, std::path::PathBuf) {
        let root = axon_test_support::test_temp_dir();
        let launcher = root.path().join("launcher").join("axon-launcher");
        std::fs::create_dir_all(launcher.parent().expect("launcher 父目录"))
            .expect("launcher 目录应创建");
        std::fs::write(root.path().join("current.json"), r#"{"versionId":"v1"}"#)
            .expect("manifest 应写入");
        let version_dir = root.path().join("versions").join("v1");
        std::fs::create_dir_all(&version_dir).expect("版本目录应创建");
        let cli = version_dir.join(if cfg!(windows) { "axon.exe" } else { "axon" });
        let desktop = version_dir.join(if cfg!(windows) {
            "axon-desktop.exe"
        } else {
            "axon-desktop"
        });
        std::fs::write(&cli, b"axon").expect("版本二进制应创建");
        std::fs::write(&desktop, b"axon-desktop").expect("桌面二进制应创建");
        (root, launcher)
    }

    #[test]
    fn resolves_cli_binary_by_default() {
        let (root, launcher) = install_tree();
        let expected = root
            .path()
            .join("versions")
            .join("v1")
            .join(if cfg!(windows) { "axon.exe" } else { "axon" });
        assert_eq!(
            resolve_target(&launcher, false).expect("应解析 CLI"),
            expected
        );
    }

    #[test]
    fn resolves_desktop_binary_for_desktop_entry() {
        let (root, launcher) = install_tree();
        let expected = root
            .path()
            .join("versions")
            .join("v1")
            .join(if cfg!(windows) {
                "axon-desktop.exe"
            } else {
                "axon-desktop"
            });
        assert_eq!(
            resolve_target(&launcher, true).expect("应解析桌面端"),
            expected
        );
    }

    #[test]
    fn rejects_non_normal_release_id() {
        for value in ["", "../v1", "v1/v2", "C:release"] {
            assert!(!is_release_id(value), "{value} 应被拒绝");
        }
    }

    #[test]
    fn hides_only_daemon_run_child() {
        assert!(is_daemon_run(&[
            OsString::from("daemon"),
            OsString::from("run")
        ]));
        assert!(!is_daemon_run(&[OsString::from("desktop")]));
        assert!(!is_daemon_run(&[
            OsString::from("daemon"),
            OsString::from("start")
        ]));
    }
}
