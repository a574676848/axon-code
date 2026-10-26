//! 桌面级偏好的读写。
//!
//! 这些偏好只服务桌面外壳自身（托盘、通知、主题），不属于 Agent 运行配置，
//! 因此落在桌面状态文件里，而不是 `config.toml`。

use serde::{Deserialize, Serialize};

use crate::state::{DesktopState, load_state, save_state};

/// 默认主题：暖石纸白。
pub const THEME_PAPER: &str = "paper";

/// 第二套主题：冷杉矿石。
pub const THEME_FOREST: &str = "forest";

/// 第三套主题：黑曜石暗色。
pub const THEME_OBSIDIAN: &str = "obsidian";

/// 把外部传入的主题标识归一化。
///
/// 不认识的取值一律回落到暖石纸白，而不是报错：主题属于外观偏好，
/// 一个旧版本或手改过的状态文件不该让设置页打不开。
#[must_use]
pub fn normalize_theme(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        THEME_FOREST => THEME_FOREST.to_string(),
        THEME_OBSIDIAN => THEME_OBSIDIAN.to_string(),
        _ => THEME_PAPER.to_string(),
    }
}

/// 从持久状态里读出主题标识；空值即纸白。
fn theme_of(state: &DesktopState) -> String {
    state
        .theme
        .as_deref()
        .map_or_else(|| THEME_PAPER.to_string(), normalize_theme)
}

/// 组成一次快照，避免读写两处各拼一遍字段。
fn snapshot(state: &DesktopState) -> DesktopPreferences {
    DesktopPreferences {
        notifications_enabled: state.notifications_enabled,
        theme: theme_of(state),
    }
}

/// 桌面偏好快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreferences {
    /// 是否发送系统通知。
    pub notifications_enabled: bool,
    /// 界面主题标识，取值为 `paper`、`forest` 或 `obsidian`。
    pub theme: String,
}

/// Tauri IPC 命令：读取桌面偏好。
#[tauri::command]
pub fn desktop_preferences() -> Result<DesktopPreferences, String> {
    Ok(snapshot(&load_state()))
}

/// Tauri IPC 命令：设置是否发送系统通知。
#[tauri::command]
pub fn desktop_set_notifications(enabled: bool) -> Result<DesktopPreferences, String> {
    let mut state = load_state();
    state.notifications_enabled = enabled;
    save_state(&state)?;
    Ok(snapshot(&state))
}

/// Tauri IPC 命令：切换界面主题。
///
/// 纸白存空值，森林存标识本身。入参先归一化，非法取值落回纸白，
/// 状态文件里不会出现第三种取值。
#[tauri::command]
pub fn desktop_set_theme(theme: String) -> Result<DesktopPreferences, String> {
    let normalized = normalize_theme(&theme);
    let mut state = load_state();
    state.theme = if normalized == THEME_PAPER {
        None
    } else {
        Some(normalized)
    };
    save_state(&state)?;
    Ok(snapshot(&state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_snapshot_uses_camel_case_keys() {
        let value = serde_json::to_value(DesktopPreferences {
            notifications_enabled: true,
            theme: THEME_PAPER.to_string(),
        })
        .expect("序列化");
        assert_eq!(value["notificationsEnabled"], true);
        assert_eq!(value["theme"], THEME_PAPER);
    }

    #[test]
    fn unknown_theme_falls_back_to_paper() {
        assert_eq!(normalize_theme("forest"), THEME_FOREST);
        assert_eq!(normalize_theme(" Forest "), THEME_FOREST);
        assert_eq!(normalize_theme("obsidian"), THEME_OBSIDIAN);
        assert_eq!(normalize_theme(" Obsidian "), THEME_OBSIDIAN);
        assert_eq!(normalize_theme("solarized"), THEME_PAPER);
        assert_eq!(normalize_theme(""), THEME_PAPER);
    }

    #[test]
    fn empty_state_theme_reads_as_paper() {
        let state = DesktopState::default();
        assert_eq!(theme_of(&state), THEME_PAPER);
    }

    #[test]
    fn stored_theme_survives_round_trip() {
        let state = DesktopState {
            theme: Some(THEME_FOREST.to_string()),
            ..DesktopState::default()
        };
        assert_eq!(theme_of(&state), THEME_FOREST);

        let dark_state = DesktopState {
            theme: Some(THEME_OBSIDIAN.to_string()),
            ..DesktopState::default()
        };
        assert_eq!(theme_of(&dark_state), THEME_OBSIDIAN);
    }
}
