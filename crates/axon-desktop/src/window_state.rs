//! 窗口几何记忆与静默呈现。
//!
//! 两件事共用同一条时间线：窗口创建后由插件静默恢复尺寸、位置、最大化与全屏
//! （**绝不恢复可见性**），之后由前端的首帧就绪信号触发显示，另有 350ms 兜底超时。
//!
//! 之所以把可见性从恢复项里摘出去：插件默认会在恢复几何时顺手把窗口显示出来，
//! 那一刻前端还没画完第一帧，用户看到的就是一块白底加一次位置跳动。

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_window_state::{Builder as WindowStateBuilder, StateFlags};

use crate::window_effects;

/// 主窗口标签，与 [`crate::desktop_shell`] 保持一致。
const MAIN_WINDOW: &str = "main";

/// 首帧就绪的兜底等待。
///
/// 前端脚本异常（白屏、首帧报错）时窗口不能永远不出现，因此超时即自行显示。
/// 350ms 是冷启动呈现时间的及格线，也是用户还不会察觉「为什么慢了一下」的上限。
const READY_TIMEOUT_MS: u64 = 350;

/// 窗口是否已经呈现过。托盘收起再唤回走的是另一条路径，不重复计数。
static REVEALED: AtomicBool = AtomicBool::new(false);

/// 窗口状态插件：只恢复几何，绝不恢复可见性。
pub fn plugin() -> TauriPlugin<Wry> {
    WindowStateBuilder::default()
        .with_state_flags(
            StateFlags::SIZE
                | StateFlags::POSITION
                | StateFlags::MAXIMIZED
                | StateFlags::FULLSCREEN,
        )
        .build()
}

/// 安装兜底呈现：超时仍未收到就绪信号就自己把窗口露出来。
pub fn install_show_gate(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(READY_TIMEOUT_MS));
        reveal(&handle);
    });
}

/// 呈现主窗口。
///
/// 幂等：就绪信号与兜底超时可能同时到达，先到的那一次生效，
/// 后到的直接返回，因此不会出现显示两次带来的焦点抖动。
pub fn reveal(app: &AppHandle) {
    if REVEALED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let _ = window.show();
    let _ = window.set_focus();
}

/// Tauri IPC 命令：前端首帧就绪。
///
/// 显示窗口与回执系统材质结论合并成一次往返：界面在同一帧里既完成呈现，
/// 又拿到「要不要透光」的答案，因此不会先画实色再切成半透。
#[tauri::command]
pub fn desktop_notify_ready(app: AppHandle) -> String {
    reveal(&app);
    window_effects::material().as_str().to_string()
}
