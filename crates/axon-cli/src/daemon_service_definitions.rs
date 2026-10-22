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
pub(super) fn systemd_unit(exe: &Path, config: &Path, runtime: &Path) -> String {
    let exe = systemd_path(&exe.to_string_lossy());
    let config = systemd_path(&config.to_string_lossy());
    format!(
        "[Unit]\nDescription=Axon Daemon\nAfter=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=0\n\n[Service]\nType=simple\nExecStart=\"{exe}\" daemon run --root \"{}\" --config-path \"{config}\"\nRestart=always\nRestartSec=5\nKillSignal=SIGTERM\nTimeoutStopSec=20\n\n[Install]\nWantedBy=default.target\n",
        systemd_path(&runtime.parent().unwrap_or(runtime).to_string_lossy())
    )
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
