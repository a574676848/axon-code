//! 操作系统级麦克风隐私状态探测与原生系统设置跳转。
//!
//! 桌面应用不同于网页：当操作系统层（如 Windows 隐私设置里的「麦克风访问权限」或
//! 「允许桌面应用访问你的麦克风」、macOS 的「隐私与安全性 › 麦克风」）拦截了采集时，
//! 浏览器权限弹窗帮不上任何忙，用户需要明确知道被哪一层拦截，并能一键打开系统
//! 对应的设置面板完成授权。

use serde::Serialize;

/// 操作系统麦克风隐私状态回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneAccessDto {
    /// 状态枚举：
    /// - `"granted"`：系统隐私层已放行；
    /// - `"system_denied"`：系统麦克风总开关已关闭；
    /// - `"desktop_denied"`：系统禁止桌面应用访问麦克风；
    /// - `"unknown"`：平台由运行期授权接管或未显式写入策略。
    pub state: &'static str,
    /// 当前平台是否支持一键拉起系统麦克风设置页。
    pub can_open_settings: bool,
    /// 面向界面展示的系统设置入口名称。
    pub settings_label: &'static str,
}

/// 解析 Windows `reg query` 输出里的 `Value    REG_SZ    Allow|Deny` 行。
#[must_use]
pub fn parse_windows_consent_value(stdout: &str) -> Option<bool> {
    for raw_line in stdout.lines() {
        let line = raw_line.trim();
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        if !name.eq_ignore_ascii_case("Value") {
            continue;
        }
        let Some(_reg_type) = parts.next() else {
            continue;
        };
        let Some(decision) = parts.next() else {
            continue;
        };
        if decision.eq_ignore_ascii_case("Allow") {
            return Some(true);
        }
        if decision.eq_ignore_ascii_case("Deny") {
            return Some(false);
        }
    }
    None
}

/// 根据 Windows 全局开关与桌面应用子项合成最终状态。
#[must_use]
pub const fn classify_windows_consent(
    global: Option<bool>,
    non_packaged: Option<bool>,
) -> &'static str {
    if matches!(global, Some(false)) {
        return "system_denied";
    }
    if matches!(non_packaged, Some(false)) {
        return "desktop_denied";
    }
    if matches!(non_packaged, Some(true)) || matches!(global, Some(true)) {
        return "granted";
    }
    "unknown"
}

#[cfg(target_os = "windows")]
fn query_windows_consent_key(key: &str) -> Option<bool> {
    let mut command = std::process::Command::new("reg");
    command.args(["query", key, "/v", "Value"]);
    let output = crate::process::run_output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_windows_consent_value(&text)
}

/// 探测当前操作系统的麦克风隐私状态（同步实现，供 IPC 与调试桥共用）。
#[must_use]
pub fn probe_microphone_status() -> MicrophoneAccessDto {
    #[cfg(target_os = "windows")]
    {
        const ROOT: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
        const NON_PACKAGED: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged";
        let global = query_windows_consent_key(ROOT);
        let desktop = query_windows_consent_key(NON_PACKAGED);
        MicrophoneAccessDto {
            state: classify_windows_consent(global, desktop),
            can_open_settings: true,
            settings_label: "打开 Windows 麦克风隐私设置",
        }
    }
    #[cfg(target_os = "macos")]
    {
        MicrophoneAccessDto {
            state: "unknown",
            can_open_settings: true,
            settings_label: "打开系统设置 › 隐私与安全性 › 麦克风",
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        MicrophoneAccessDto {
            state: "unknown",
            can_open_settings: true,
            settings_label: "打开系统声音设置",
        }
    }
}

/// 打开操作系统原生的麦克风隐私或音频输入设置面板。
///
/// # Errors
///
/// 当前平台没有可用的系统设置入口或拉起失败。
#[tauri::command]
pub fn voice_open_microphone_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        crate::process::open_system_uri("ms-settings:privacy-microphone")
            .map_err(|error| format!("无法打开 Windows 麦克风隐私设置：{error}"))
    }
    #[cfg(target_os = "macos")]
    {
        crate::process::open_system_uri(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        )
        .map_err(|error| format!("无法打开系统麦克风隐私设置：{error}"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        for (program, args) in [
            ("gnome-control-center", &["sound"][..]),
            ("pavucontrol", &[][..]),
            ("systemsettings", &["kcm_pulseaudio"][..]),
        ] {
            let mut command = std::process::Command::new(program);
            command.args(args);
            if crate::process::spawn_detached(&mut command).is_ok() {
                return Ok(());
            }
        }
        Err("未能拉起系统声音设置，请手动检查系统的音频输入设备与权限".to_owned())
    }
}

/// 查询当前系统的麦克风隐私授权状态。
#[tauri::command]
#[must_use]
pub fn voice_microphone_status() -> MicrophoneAccessDto {
    probe_microphone_status()
}

#[cfg(test)]
mod tests {
    use super::{classify_windows_consent, parse_windows_consent_value};

    #[test]
    fn windows_reg_query_output_parses_allow_and_deny() {
        let allow_sample = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone\\NonPackaged\r\n    Value    REG_SZ    Allow\r\n\r\n";
        assert_eq!(parse_windows_consent_value(allow_sample), Some(true));

        let deny_sample = "\nHKEY_CURRENT_USER\\...\\microphone\n    Value    REG_SZ    Deny\n";
        assert_eq!(parse_windows_consent_value(deny_sample), Some(false));

        assert_eq!(parse_windows_consent_value("无关输出"), None);
    }

    #[test]
    fn windows_consent_classification_prioritises_denials() {
        assert_eq!(
            classify_windows_consent(Some(false), Some(true)),
            "system_denied"
        );
        assert_eq!(
            classify_windows_consent(Some(true), Some(false)),
            "desktop_denied"
        );
        assert_eq!(classify_windows_consent(Some(true), Some(true)), "granted");
        assert_eq!(classify_windows_consent(None, Some(true)), "granted");
        assert_eq!(classify_windows_consent(None, None), "unknown");
    }
}
