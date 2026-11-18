//! 桌面级偏好的读写。
//!
//! 这些偏好只服务桌面外壳自身（托盘、通知、主题），不属于 Agent 运行配置，
//! 因此落在桌面状态文件里，而不是 `config.toml`。

use serde::{Deserialize, Serialize};

use crate::connection::Runtime;
use crate::state::{DesktopState, load_state, save_state};

/// 默认主题：暖石纸白。
pub const THEME_PAPER: &str = "paper";

/// 第二套主题：黑曜石暗色。
pub const THEME_OBSIDIAN: &str = "obsidian";

/// 把外部传入的主题标识归一化。
///
/// 不认识的取值一律回落到暖石纸白，而不是报错：主题属于外观偏好，
/// 一个旧版本或手改过的状态文件不该让设置页打不开。
#[must_use]
pub fn normalize_theme(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
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
fn snapshot(state: &DesktopState, runtime: &Runtime) -> DesktopPreferences {
    let client = runtime.document().client;
    DesktopPreferences {
        notifications_enabled: state.notifications_enabled,
        theme: theme_of(state),
        sidebar_collapsed: runtime.sidebar_collapsed(),
        composer_model: runtime.composer_model(),
        composer_reasoning_effort: runtime.composer_reasoning_effort(),
        last_workspace_dir: client.last_workspace_dir,
        last_session_id: client.last_session_id,
        expanded_workspace_ids: client.expanded_workspace_ids,
    }
}

/// 桌面偏好快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPreferences {
    /// 是否发送系统通知。
    pub notifications_enabled: bool,
    /// 界面主题标识，取值为 `paper` 或 `obsidian`。
    pub theme: String,
    /// 左侧导航是否折叠；空值表示用户还没有手动表态。
    pub sidebar_collapsed: Option<bool>,
    /// 输入坞最近一次显式选择的模型名称；空值表示用户从未选过。
    pub composer_model: Option<String>,
    /// 输入坞最近一次显式选择的思考强度；空值表示用户从未选过。
    pub composer_reasoning_effort: Option<String>,
    /// 上次打开的工作区目录；空值表示还没有打开过。
    pub last_workspace_dir: Option<String>,
    /// 上次打开的会话标识；空值表示还没有打开过。
    pub last_session_id: Option<String>,
    /// 侧栏展开着的工作区标识。
    pub expanded_workspace_ids: Vec<String>,
}

/// Tauri IPC 命令：记下「上次看到哪」。
///
/// 三个入参各自可缺省：只点开会话时只写会话，只折叠工作区时只写展开清单。
/// 这份事实与展示完全在本地状态文件里，不向服务端推导。
///
/// # Errors
///
/// 状态文件只读或写盘失败。
#[tauri::command(async)]
pub fn desktop_remember_view(
    workspace_dir: Option<String>,
    session_id: Option<String>,
    expanded_workspace_ids: Option<Vec<String>>,
) -> Result<DesktopPreferences, String> {
    let runtime = crate::connection::runtime();
    runtime.update_document(|document| {
        if let Some(workspace_dir) = workspace_dir {
            let trimmed = workspace_dir.trim().to_owned();
            document.client.last_workspace_dir = (!trimmed.is_empty()).then_some(trimmed);
        }
        if let Some(session_id) = session_id {
            let trimmed = session_id.trim().to_owned();
            document.client.last_session_id = (!trimmed.is_empty()).then_some(trimmed);
        }
        if let Some(expanded) = expanded_workspace_ids {
            document.client.expanded_workspace_ids = expanded
                .into_iter()
                .map(|id| id.trim().to_owned())
                .filter(|id| !id.is_empty())
                .collect();
        }
        Ok(())
    })?;
    Ok(snapshot(&load_state(), runtime))
}

/// Tauri IPC 命令：读取桌面偏好。
#[tauri::command(async)]
pub fn desktop_preferences() -> Result<DesktopPreferences, String> {
    let state = load_state();
    Ok(snapshot(&state, crate::connection::runtime()))
}

/// Tauri IPC 命令：设置是否发送系统通知。
#[tauri::command(async)]
pub fn desktop_set_notifications(enabled: bool) -> Result<DesktopPreferences, String> {
    let runtime = crate::connection::runtime();
    let mut state = load_state();
    state.notifications_enabled = enabled;
    save_state(&state)?;
    Ok(snapshot(&state, runtime))
}

/// Tauri IPC 命令：切换界面主题。
///
/// 纸白存空值，其余主题存标识本身。入参先归一化，非法取值落回纸白，
/// 状态文件里不会出现第三种取值。已下线主题（如旧的 `forest`）同样落回纸白。
#[tauri::command(async)]
pub fn desktop_set_theme(theme: String) -> Result<DesktopPreferences, String> {
    let normalized = normalize_theme(&theme);
    let runtime = crate::connection::runtime();
    let mut state = load_state();
    state.theme = if normalized == THEME_PAPER {
        None
    } else {
        Some(normalized.clone())
    };
    save_state(&state)?;
    crate::window_effects::update_window_material_mode(normalized == THEME_OBSIDIAN);
    Ok(snapshot(&state, runtime))
}

/// Tauri IPC 命令：设置左侧导航是否折叠。
#[tauri::command(async)]
pub fn desktop_set_sidebar_collapsed(collapsed: bool) -> Result<DesktopPreferences, String> {
    let runtime = crate::connection::runtime();
    runtime.set_sidebar_collapsed(collapsed)?;
    let state = load_state();
    Ok(snapshot(&state, runtime))
}

/// Tauri IPC 命令：记下输入坞改选的主模型名称。
///
/// 存的是配置里的展示名而不是 upstream id：id 会随供应商版本漂移，名称才是用户
/// 在输入坞里认得出的那一个，也只有名称能在新会话里按配置目录重新校验一次。
#[tauri::command(async)]
pub fn desktop_set_composer_model(model: String) -> Result<DesktopPreferences, String> {
    let runtime = crate::connection::runtime();
    runtime.set_composer_model(Some(&model))?;
    let state = load_state();
    Ok(snapshot(&state, runtime))
}

/// Tauri IPC 命令：记下输入坞改选的思考强度。
#[tauri::command(async)]
pub fn desktop_set_composer_reasoning_effort(effort: String) -> Result<DesktopPreferences, String> {
    let runtime = crate::connection::runtime();
    runtime.set_composer_reasoning_effort(Some(&effort))?;
    let state = load_state();
    Ok(snapshot(&state, runtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_snapshot_uses_camel_case_keys() {
        let value = serde_json::to_value(DesktopPreferences {
            notifications_enabled: true,
            theme: THEME_PAPER.to_string(),
            sidebar_collapsed: Some(true),
            composer_model: Some("deepseek-v4.1-flash".to_string()),
            composer_reasoning_effort: Some("high".to_string()),
            last_workspace_dir: Some("C:/Users/tester/.axon".to_string()),
            last_session_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string()),
            expanded_workspace_ids: vec!["workspace-a".to_string()],
        })
        .expect("序列化");
        assert_eq!(value["notificationsEnabled"], true);
        assert_eq!(value["theme"], THEME_PAPER);
        assert_eq!(value["sidebarCollapsed"], true);
        assert_eq!(value["composerModel"], "deepseek-v4.1-flash");
        assert_eq!(value["composerReasoningEffort"], "high");
        // 「上次看到哪」全在本地状态文件里，界面按这三项恢复展开与会话。
        assert_eq!(value["lastWorkspaceDir"], "C:/Users/tester/.axon");
        assert_eq!(value["lastSessionId"], "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(value["expandedWorkspaceIds"][0], "workspace-a");
    }

    #[test]
    fn empty_composer_cache_serializes_as_null() {
        // 从未选过就是没有缓存：界面按 null 判断，不能收到空字符串。
        let value = serde_json::to_value(DesktopPreferences {
            notifications_enabled: true,
            theme: THEME_PAPER.to_string(),
            sidebar_collapsed: None,
            composer_model: None,
            composer_reasoning_effort: None,
            last_workspace_dir: None,
            last_session_id: None,
            expanded_workspace_ids: Vec::new(),
        })
        .expect("序列化");
        assert!(value["composerModel"].is_null());
        assert!(value["composerReasoningEffort"].is_null());
        assert!(value["lastSessionId"].is_null());
        assert_eq!(
            value["expandedWorkspaceIds"].as_array().map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn unknown_theme_falls_back_to_paper() {
        assert_eq!(normalize_theme("obsidian"), THEME_OBSIDIAN);
        assert_eq!(normalize_theme(" Obsidian "), THEME_OBSIDIAN);
        assert_eq!(normalize_theme("solarized"), THEME_PAPER);
        assert_eq!(normalize_theme(""), THEME_PAPER);
        // 下线主题的旧状态文件必须静默回落，而不是让设置页打不开。
        assert_eq!(normalize_theme("forest"), THEME_PAPER);
    }

    #[test]
    fn empty_state_theme_reads_as_paper() {
        let state = DesktopState::default();
        assert_eq!(theme_of(&state), THEME_PAPER);
    }

    #[test]
    fn stored_theme_survives_round_trip() {
        let state = DesktopState {
            theme: Some(THEME_OBSIDIAN.to_string()),
            ..DesktopState::default()
        };
        assert_eq!(theme_of(&state), THEME_OBSIDIAN);

        let paper_state = DesktopState {
            theme: None,
            ..DesktopState::default()
        };
        assert_eq!(theme_of(&paper_state), THEME_PAPER);
    }
}
