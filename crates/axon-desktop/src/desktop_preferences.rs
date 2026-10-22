//! 桌面级偏好的读写。
//!
//! 这些偏好只服务桌面外壳自身（托盘、通知），不属于 Agent 运行配置，
//! 因此落在桌面状态文件里，而不是 `config.toml`。

use serde::{Deserialize, Serialize};

/// 桌面偏好快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreferences {
    /// 是否发送系统通知。
    pub notifications_enabled: bool,
}

/// Tauri IPC 命令：读取桌面偏好。
#[tauri::command]
pub fn desktop_preferences() -> Result<DesktopPreferences, String> {
    let state = crate::state::load_state();
    Ok(DesktopPreferences {
        notifications_enabled: state.notifications_enabled,
    })
}

/// Tauri IPC 命令：设置是否发送系统通知。
#[tauri::command]
pub fn desktop_set_notifications(enabled: bool) -> Result<DesktopPreferences, String> {
    let mut state = crate::state::load_state();
    state.notifications_enabled = enabled;
    crate::state::save_state(&state)?;
    Ok(DesktopPreferences {
        notifications_enabled: state.notifications_enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_snapshot_uses_camel_case_keys() {
        let value = serde_json::to_value(DesktopPreferences {
            notifications_enabled: true,
        })
        .expect("序列化");
        assert_eq!(value["notificationsEnabled"], true);
    }
}
