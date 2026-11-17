#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::path::Path;

#[cfg(any(target_os = "macos", test))]
const LAUNCHD_LABEL: &str = "com.axon.daemon";

#[cfg(any(target_os = "macos", test))]
pub(super) fn launchd_plist(exe: &Path, config: &Path, runtime: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Label</key><string>{LAUNCHD_LABEL}</string><key>ProgramArguments</key><array><string>{}</string><string>daemon</string><string>run</string><string>--root</string><string>{}</string><string>--config-path</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>30</integer><key>ExitTimeOut</key><integer>15</integer><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>
"#,
        xml(&exe.to_string_lossy()),
        xml(&runtime.parent().unwrap_or(runtime).to_string_lossy()),
        xml(&config.to_string_lossy()),
        xml(&runtime
            .join("logs/axon-daemon-service.out.log")
            .to_string_lossy()),
        xml(&runtime
            .join("logs/axon-daemon-service.err.log")
            .to_string_lossy())
    )
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn systemd_unit(
    exe: &Path,
    config: &Path,
    runtime: &Path,
    environment: &[(String, String)],
) -> String {
    let exe = systemd_path(&exe.to_string_lossy());
    let config = systemd_path(&config.to_string_lossy());
    let environment_lines = systemd_environment(environment);
    format!(
        "[Unit]\nDescription=Axon Daemon\nAfter=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=0\n\n[Service]\nType=simple\n{environment_lines}ExecStart=\"{exe}\" daemon run --root \"{}\" --config-path \"{config}\"\nRestart=always\nRestartSec=5\nKillSignal=SIGTERM\nTimeoutStopSec=20\n\n[Install]\nWantedBy=default.target\n",
        systemd_path(&runtime.parent().unwrap_or(runtime).to_string_lossy())
    )
}

/// 把环境变量渲染成 systemd 单元的 `Environment=` 行；空表渲染为空串。
///
/// systemd 用户实例只提供最小环境，交互 shell 的 `PATH` 扩展与 `SHELL` 都不会被继承，
/// 因此安装时把现场值固化进单元。值沿用 `systemd_path` 的转义口径（`%` → `%%`、
/// `\` → `\\`、`"` → `\"`），与 `ExecStart` 里的路径保持同一套规则，不另造第二套。
#[cfg(any(target_os = "linux", test))]
fn systemd_environment(environment: &[(String, String)]) -> String {
    environment
        .iter()
        .map(|(key, value)| format!("Environment=\"{}={}\"\n", key, systemd_path(value)))
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "linux", test))]
fn systemd_path(value: &str) -> String {
    value
        .replace('%', "%%")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}
