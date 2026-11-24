//! 窗口级系统材质、原生深浅色联动与最大化状态翻转。
//!
//! 材质是可选增强而不是界面前提：不可用时托盘必须与纯实色完全一致。
//! 因此这里只回答「这台机器上能不能透光」，具体怎么画由前端的
//! `data-material` 与 `data-material-fallback` 决定，两边职责不重叠。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Emitter, Manager, Runtime, Theme, WebviewWindow};

/// 主窗口标签，与 [`crate::desktop_shell`] 保持一致。
const MAIN_WINDOW: &str = "main";

/// 最大化状态翻转事件的推送名，与前端 `useWindowDynamicSurfaces` 一致。
pub const MAXIMIZED_EVENT: &str = "axon://window-maximized-changed";

/// 进程内唯一的材质结论：启动时探测一次，之后只读取。
static MATERIAL: OnceLock<WindowMaterial> = OnceLock::new();

/// 应用句柄：主题切换发生在 IPC 命令里，不一定持有句柄，因此在这里存一份。
static APP: OnceLock<AppHandle> = OnceLock::new();

/// 系统材质的可用性结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMaterial {
    /// Windows 11 的 DWM Mica 材质。
    Mica,
    /// macOS 的 `NSVisualEffectMaterial::Sidebar` 磨砂。
    Vibrancy,
    /// 无材质可用：托盘取 100% 实色。
    Solid,
}

impl WindowMaterial {
    /// 回给前端与日志使用的材质标识。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mica => "mica",
            Self::Vibrancy => "vibrancy",
            Self::Solid => "solid",
        }
    }
}

/// 窗口初始化时建立一次性系统材质底座与亮白漫射底托。
///
/// 底托是必须的：深色壁纸下，浅色侧栏若没有一层不透明的亮底，会从底层
/// 透出一片脏灰。因此这里在建立材质之后立刻把窗口深浅色钉到持久化的主题上。
pub fn init_window_material<R: Runtime>(app: &AppHandle<R>) -> WindowMaterial {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return WindowMaterial::Solid;
    };
    let material = apply_platform_material(&window);
    let _ = window.set_theme(Some(stored_window_theme()));
    let _ = MATERIAL.set(material);
    material
}

/// 注入应用句柄：主题切换与材质联动都要用到它。
pub fn install(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// 启动时探测到的材质结论；尚未初始化时按无材质处理。
#[must_use]
pub fn material() -> WindowMaterial {
    MATERIAL.get().copied().unwrap_or(WindowMaterial::Solid)
}

/// 依据前端当前主题同步窗口深浅色（`DWMWA_USE_IMMERSIVE_DARK_MODE`）。
///
/// 这条调用极轻，且不重建材质，因此可以随主题切换反复调用：
/// 重复应用材质才是黑斑叠加与频闪的来源。
pub fn update_window_material_mode(is_dark: bool) {
    let Some(app) = APP.get() else {
        return;
    };
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let target = if is_dark { Theme::Dark } else { Theme::Light };
    let _ = window.set_theme(Some(target));
}

/// 监听最大化状态翻转，只在真的翻转时推一次事件。
///
/// 前端因此不必在每次尺寸变化时都问一次 `isMaximized()`：
/// 拖拽窗口边缘会在一秒内产生上百次尺寸变化，逐个往返 IPC 是纯粹的浪费。
pub fn watch_maximized<R: Runtime>(window: &WebviewWindow<R>) {
    let handle = window.clone();
    let current = window.is_maximized().unwrap_or(false);
    let last = Arc::new(AtomicBool::new(current));
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Resized(_)) {
            return;
        }
        let Ok(maximized) = handle.is_maximized() else {
            return;
        };
        if last.swap(maximized, Ordering::Relaxed) == maximized {
            return;
        }
        if let Err(error) = handle.app_handle().emit(MAXIMIZED_EVENT, maximized) {
            eprintln!("Axon 最大化状态事件下发失败：{error}");
        }
    });
}

/// 持久化主题对应的窗口深浅色。
pub(crate) fn stored_window_theme() -> Theme {
    let stored = crate::state::load_state().theme;
    let theme = stored.as_deref().map_or_else(
        || crate::desktop_preferences::THEME_PAPER.to_string(),
        crate::desktop_preferences::normalize_theme,
    );
    if theme == crate::desktop_preferences::THEME_OBSIDIAN {
        Theme::Dark
    } else {
        Theme::Light
    }
}

#[cfg(target_os = "windows")]
fn apply_platform_material<R: Runtime>(window: &WebviewWindow<R>) -> WindowMaterial {
    /*
     * 窗口四角的圆角由前端自己裁：托盘底座带 10px 圆角并裁掉溢出，窗口本体是透明的，
     * 因此四角透出的是桌面而不是白底。
     *
     * 不走 DWM 的 `DWMWA_WINDOW_CORNER_PREFERENCE`(33)：那需要 `DwmSetWindowAttribute`
     * 的 unsafe 调用，而本工作区把 `unsafe_code` 设为 forbid。前端裁角在三平台同一份实现，
     * 也没有「Windows 11 有、Windows 10 没有」的版本分支。
     */
    // Windows 10 的 Acrylic 在拖动窗口时有系统性重绘拖影，因此这里只尝试 Mica：
    // 失败（含 Windows 10）即直接回退实色，绝不调用 Acrylic 换那一点透光。
    let dark = stored_window_theme() == Theme::Dark;
    match window_vibrancy::apply_mica(window, Some(dark)) {
        Ok(()) => WindowMaterial::Mica,
        Err(error) => {
            eprintln!("Axon 窗口 Mica 材质不可用，回退实色托盘：{error}");
            WindowMaterial::Solid
        }
    }
}

#[cfg(target_os = "macos")]
fn apply_platform_material<R: Runtime>(window: &WebviewWindow<R>) -> WindowMaterial {
    match window_vibrancy::apply_vibrancy(
        window,
        window_vibrancy::NSVisualEffectMaterial::Sidebar,
        None,
        None,
    ) {
        Ok(()) => WindowMaterial::Vibrancy,
        Err(error) => {
            eprintln!("Axon 窗口 Vibrancy 材质不可用，回退实色托盘：{error}");
            WindowMaterial::Solid
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn apply_platform_material<R: Runtime>(_window: &WebviewWindow<R>) -> WindowMaterial {
    // Linux 走 GTK 复合器阴影加纯色层：透明合成在部分合成器上会画出黑底。
    WindowMaterial::Solid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_identifiers_are_stable() {
        // 前端按这三个字符串决定透光与回退，改名等于静默失效。
        assert_eq!(WindowMaterial::Mica.as_str(), "mica");
        assert_eq!(WindowMaterial::Vibrancy.as_str(), "vibrancy");
        assert_eq!(WindowMaterial::Solid.as_str(), "solid");
    }
}
