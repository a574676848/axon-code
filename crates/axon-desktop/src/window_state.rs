//! 窗口几何记忆与静默呈现。
//!
//! 两件事共用同一条时间线：窗口创建后先按上次的几何静默恢复（**绝不恢复可见性**），
//! 之后由前端首帧就绪信号触发显示，另有 350ms 兜底超时。可见性之所以被摘出恢复项：
//! 窗口一恢复几何就显示，那一刻前端还没画完第一帧，用户看到的就是一块白底加一次位置跳动。
//!
//! 几何全部按**逻辑像素**落盘与恢复：DPI 变了，窗口还是同一个观感大小。
//!
//! 记什么、什么时候允许改写，由三条判据决定，三条都对应一类真实窗口状态：
//!
//! - **最小化期间一律不改写**。最小化时系统同样会把「是否最大化」判成否，照着它记
//!   就会把用户最大化过的窗口记成普通窗口，下次启动按最大化之前的尺寸打开。
//! - **最大化期间不记尺寸**。满屏尺寸不是用户要的还原尺寸，记了就把还原态顶成满屏。
//! - **最大化期间不记位置**。最大化那一瞬系统会把窗口搬到屏幕外的负坐标，
//!   那是最大化原点而不是用户的摆放，记下来会让还原后的窗口贴着屏幕边缘被切掉。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    WebviewWindow, WindowEvent,
};

/// 主窗口标签，与 [`crate::desktop_shell`] 保持一致。
const MAIN_WINDOW: &str = "main";

/// 几何文件名。与历史上窗口状态插件同处一份目录，避免运行态数据里多出一个新位置。
const GEOMETRY_FILE: &str = ".window-state.json";

/// 几何结构版本。老格式（插件存的物理像素）没有这个字段，读进来会被整体丢弃。
const GEOMETRY_VERSION: u32 = 2;

/// 首帧就绪的兜底等待。
///
/// 前端脚本异常（白屏、首帧报错）时窗口不能永远不出现，因此超时即自行显示。
/// 350ms 是冷启动呈现时间的及格线，也是用户还不会察觉「为什么慢了一下」的上限。
const READY_TIMEOUT_MS: u64 = 350;

/// 恢复时的尺寸下限，与 `tauri.conf.json` 的 `minWidth` / `minHeight` 同值。
///
/// 必须自己夹：无边框窗口去掉了系统可调整边框（`WS_THICKFRAME`），系统不再用
/// `WM_GETMINMAXINFO` 约束程序化改尺寸，一份过小的历史几何会把界面压成破框态。
const MIN_RESTORE_WIDTH: f64 = 800.0;
/// 恢复时的尺寸下限（高），与 `minWidth` / `minHeight` 同源。
const MIN_RESTORE_HEIGHT: f64 = 580.0;

/// 窗口是否已经呈现过。托盘收起再唤回走的是另一条路径，不重复计数。
static REVEALED: AtomicBool = AtomicBool::new(false);

/// 进程内唯一的几何记忆：窗口事件写、退出与关闭时读。
static GEOMETRY: OnceLock<Mutex<WindowGeometry>> = OnceLock::new();

/// 一份窗口几何：普通态（还原态）的尺寸与位置，外加两个布尔态。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowGeometry {
    /// 结构版本。
    version: u32,
    /// 普通态内容区宽度（逻辑像素）。
    width: f64,
    /// 普通态内容区高度（逻辑像素）。
    height: f64,
    /// 普通态窗口左上角横坐标（逻辑像素）。
    x: f64,
    /// 普通态窗口左上角纵坐标（逻辑像素）。
    y: f64,
    /// 是否最大化。
    maximized: bool,
    /// 是否全屏。
    fullscreen: bool,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            version: GEOMETRY_VERSION,
            width: 0.0,
            height: 0.0,
            x: 0.0,
            y: 0.0,
            maximized: false,
            fullscreen: false,
        }
    }
}

impl WindowGeometry {
    /// 是否拿到过真实尺寸。全 0 的默认值不参与落盘，免得拿空记录覆盖上一次的几何。
    fn is_recorded(self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }

    /// 是否是一份可用的记录：版本一致且有真实尺寸。
    fn is_usable(self) -> bool {
        self.version == GEOMETRY_VERSION && self.is_recorded()
    }

    /// 尺寸变化后的记忆。
    ///
    /// 最小化与最大化都不改写尺寸：前者会把宽高报成 0，后者报的是满屏尺寸。
    fn with_resize(
        self,
        inner: PhysicalSize<u32>,
        scale: f64,
        minimized: bool,
        maximized: bool,
        fullscreen: bool,
    ) -> Self {
        if minimized {
            return self;
        }
        let mut next = Self {
            maximized,
            fullscreen,
            ..self
        };
        if maximized || inner.width == 0 || inner.height == 0 {
            return next;
        }
        next.width = f64::from(inner.width) / scale;
        next.height = f64::from(inner.height) / scale;
        next
    }

    /// 位置变化后的记忆。只有既没最大化也没最小化时才记用户的摆放。
    fn with_move(
        self,
        outer: PhysicalPosition<i32>,
        scale: f64,
        minimized: bool,
        maximized: bool,
    ) -> Self {
        if minimized || maximized {
            return self;
        }
        Self {
            x: f64::from(outer.x) / scale,
            y: f64::from(outer.y) / scale,
            ..self
        }
    }

    /// 恢复前把尺寸夹进「最小可视」与「最大一块屏」之间。
    fn clamped_size(self, widest: Option<(f64, f64)>) -> (f64, f64) {
        let mut width = self.width.max(MIN_RESTORE_WIDTH);
        let mut height = self.height.max(MIN_RESTORE_HEIGHT);
        if let Some((max_width, max_height)) = widest {
            width = width.min(max_width.max(MIN_RESTORE_WIDTH));
            height = height.min(max_height.max(MIN_RESTORE_HEIGHT));
        }
        (width, height)
    }
}

/// 安装几何记忆：先恢复，再把「此刻的普通态几何」立为记忆初值，最后挂上事件跟踪。
pub fn install(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let saved = load(app).filter(|geometry| geometry.is_usable());
    store(apply(&window, saved));
    watch(window);
}

/// 保存当前几何。关闭、退出与最大化翻转都会走到这里。
pub fn flush(app: &AppHandle) {
    let Some(path) = geometry_path(app) else {
        return;
    };
    let geometry = current();
    if !geometry.is_usable() {
        return;
    }
    if let Some(directory) = path.parent() {
        let _ = std::fs::create_dir_all(directory);
    }
    match serde_json::to_vec_pretty(&geometry) {
        Ok(json) => {
            if let Err(error) = std::fs::write(&path, json) {
                eprintln!("Axon 窗口几何保存失败：{error}");
            }
        }
        Err(error) => eprintln!("Axon 窗口几何序列化失败：{error}"),
    }
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
    crate::window_effects::material().as_str().to_string()
}

/// 几何文件路径：与其余桌面客户端数据同处系统应用数据目录。
fn geometry_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join(GEOMETRY_FILE))
}

/// 读取几何。文件缺失、损坏或来自老格式都按「没有记录」处理。
fn load(app: &AppHandle) -> Option<WindowGeometry> {
    let path = geometry_path(app)?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 应用一份几何，返回落地后的普通态几何（作为记忆初值）。
fn apply(window: &WebviewWindow, saved: Option<WindowGeometry>) -> WindowGeometry {
    let mut geometry = saved.unwrap_or_default();
    if geometry.is_recorded() {
        let (width, height) = geometry.clamped_size(widest_work_area(window));
        let _ = window.set_size(LogicalSize::new(width, height));
        geometry.width = width;
        geometry.height = height;
        if position_is_visible(window, &geometry) {
            let _ = window.set_position(LogicalPosition::new(geometry.x, geometry.y));
        } else if let Some(position) = logical_outer_position(window) {
            // 记下的位置已经不在任何一块屏上（换过显示器或拔过外接屏）：留在系统给的位置。
            geometry.x = position.0;
            geometry.y = position.1;
        }
    } else if let Some((width, height)) = logical_inner_size(window) {
        geometry = WindowGeometry {
            version: GEOMETRY_VERSION,
            width,
            height,
            ..geometry
        };
        if let Some(position) = logical_outer_position(window) {
            geometry.x = position.0;
            geometry.y = position.1;
        }
    }
    geometry.version = GEOMETRY_VERSION;
    if geometry.maximized {
        let _ = window.maximize();
    }
    if geometry.fullscreen {
        let _ = window.set_fullscreen(true);
    }
    geometry
}

/// 挂上窗口事件跟踪：尺寸、位置、关闭三处更新记忆。
fn watch(window: WebviewWindow) {
    let handle = window.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            let minimized = handle.is_minimized().unwrap_or(false);
            let maximized = handle.is_maximized().unwrap_or(false);
            let fullscreen = handle.is_fullscreen().unwrap_or(false);
            let scale = handle.scale_factor().unwrap_or(1.0);
            let before = current();
            let after = before.with_resize(*size, scale, minimized, maximized, fullscreen);
            if after == before {
                return;
            }
            store(after);
            // 最大化翻转是用户看得见的一步，立刻落盘：此时离开应用的路径可能不经过正常退出。
            if after.maximized != before.maximized {
                flush(handle.app_handle());
            }
        }
        WindowEvent::Moved(position) => {
            let minimized = handle.is_minimized().unwrap_or(false);
            let maximized = handle.is_maximized().unwrap_or(false);
            let scale = handle.scale_factor().unwrap_or(1.0);
            let before = current();
            let after = before.with_move(*position, scale, minimized, maximized);
            if after != before {
                store(after);
            }
        }
        WindowEvent::CloseRequested { .. } => flush(handle.app_handle()),
        _ => {}
    });
}

/// 内容区尺寸（逻辑像素）。
fn logical_inner_size(window: &WebviewWindow) -> Option<(f64, f64)> {
    let scale = window.scale_factor().ok()?;
    let size = window.inner_size().ok()?;
    if size.width == 0 || size.height == 0 {
        return None;
    }
    Some((
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    ))
}

/// 窗口左上角坐标（逻辑像素）。
fn logical_outer_position(window: &WebviewWindow) -> Option<(f64, f64)> {
    let scale = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?;
    Some((f64::from(position.x) / scale, f64::from(position.y) / scale))
}

/// 记下的位置是否落在某块屏的工作区里。
///
/// 判据取左上角：窗口左上角在屏内，用户就一定抓得到标题栏把它拖回来。
fn position_is_visible(window: &WebviewWindow, geometry: &WindowGeometry) -> bool {
    let Ok(scale) = window.scale_factor() else {
        return false;
    };
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    let x = geometry.x * scale;
    let y = geometry.y * scale;
    if !x.is_finite() || !y.is_finite() {
        return false;
    }
    monitors.iter().any(|monitor| {
        let area = monitor.work_area();
        let left = f64::from(area.position.x);
        let top = f64::from(area.position.y);
        let right = left + f64::from(area.size.width);
        let bottom = top + f64::from(area.size.height);
        x >= left && x < right && y >= top && y < bottom
    })
}

/// 所有屏里最大的一块工作区（逻辑像素），用来给恢复尺寸封顶。
fn widest_work_area(window: &WebviewWindow) -> Option<(f64, f64)> {
    let monitors = window.available_monitors().ok()?;
    monitors
        .iter()
        .map(|monitor| {
            let area = monitor.work_area();
            let scale = monitor.scale_factor();
            (
                f64::from(area.size.width) / scale,
                f64::from(area.size.height) / scale,
            )
        })
        .fold(None, |widest, current| match widest {
            None => Some(current),
            Some((width, height)) => Some((width.max(current.0), height.max(current.1))),
        })
}

/// 进程内的几何记忆。
fn record() -> &'static Mutex<WindowGeometry> {
    GEOMETRY.get_or_init(|| Mutex::new(WindowGeometry::default()))
}

/// 读一份当前记忆。
fn current() -> WindowGeometry {
    *record()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 覆写当前记忆。
fn store(geometry: WindowGeometry) {
    *record()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = geometry;
}

#[cfg(test)]
mod tests {
    use super::{MIN_RESTORE_HEIGHT, MIN_RESTORE_WIDTH, WindowGeometry};
    use tauri::{PhysicalPosition, PhysicalSize};

    /// 一份 1440x900 屏、200% 缩放上的普通态窗口。
    fn base() -> WindowGeometry {
        WindowGeometry {
            width: 620.0,
            height: 460.0,
            x: 120.0,
            y: 90.0,
            ..WindowGeometry::default()
        }
    }

    #[test]
    fn minimizing_keeps_the_maximized_memory() {
        // 回归判据：最大化后最小化，记忆必须仍然是「最大化」。
        // 系统在最小化时也会把是否最大化判成否，照着它记就会把窗口记成普通窗口。
        let maximized = WindowGeometry {
            maximized: true,
            ..base()
        };
        let after = maximized.with_resize(PhysicalSize::new(0, 0), 2.0, true, false, false);
        assert!(after.maximized);
        assert_eq!(after.width, 620.0);
        assert_eq!(after.height, 460.0);

        let moved = after.with_move(PhysicalPosition::new(-200, -200), 2.0, true, false);
        assert_eq!(moved.x, 120.0);
        assert_eq!(moved.y, 90.0);
    }

    #[test]
    fn maximizing_keeps_the_restore_size() {
        // 最大化报上来的是满屏尺寸，记下来会把还原态顶成满屏。
        let after = base().with_resize(PhysicalSize::new(2880, 1704), 2.0, false, true, false);
        assert!(after.maximized);
        assert_eq!(after.width, 620.0);
        assert_eq!(after.height, 460.0);
    }

    #[test]
    fn maximizing_does_not_record_the_screen_origin() {
        // 最大化那一瞬窗口被搬到屏幕外的负坐标，那是系统的最大化原点。
        let after = base().with_move(PhysicalPosition::new(-7, -7), 2.0, false, true);
        assert_eq!(after.x, 120.0);
        assert_eq!(after.y, 90.0);
    }

    #[test]
    fn resizing_and_moving_in_the_normal_state_are_recorded_in_logical_pixels() {
        let after = base()
            .with_resize(PhysicalSize::new(1600, 1200), 2.0, false, false, false)
            .with_move(PhysicalPosition::new(400, 300), 2.0, false, false);
        assert_eq!((after.width, after.height), (800.0, 600.0));
        assert_eq!((after.x, after.y), (200.0, 150.0));
        assert!(!after.maximized);
    }

    #[test]
    fn tiny_saved_size_is_clamped_to_the_minimum_visible_area() {
        let tiny = WindowGeometry {
            width: 100.0,
            height: 80.0,
            ..base()
        };
        let (width, height) = tiny.clamped_size(None);
        assert_eq!(width, MIN_RESTORE_WIDTH);
        assert_eq!(height, MIN_RESTORE_HEIGHT);

        // 反过来，比最大一块屏还大的历史尺寸也要收回来。
        let huge = WindowGeometry {
            width: 5000.0,
            height: 4000.0,
            ..base()
        };
        let (width, height) = huge.clamped_size(Some((1440.0, 852.0)));
        assert_eq!((width, height), (1440.0, 852.0));
    }

    #[test]
    fn legacy_physical_pixel_state_is_discarded() {
        // 老格式（窗口状态插件）没有版本号，且坐标是物理像素，读进来必须整体丢弃。
        let legacy = r#"{
          "main": {
            "width": 1144,
            "height": 630,
            "x": -13,
            "y": -13,
            "prev_x": -13,
            "prev_y": -13,
            "maximized": false,
            "visible": true,
            "decorated": true,
            "fullscreen": false
          }
        }"#;
        assert!(serde_json::from_str::<WindowGeometry>(legacy).is_err());
    }

    #[test]
    fn geometry_round_trips_through_the_state_file_format() {
        let geometry = WindowGeometry {
            maximized: true,
            fullscreen: false,
            ..base()
        };
        let json = serde_json::to_string(&geometry).expect("应可序列化");
        let parsed: WindowGeometry = serde_json::from_str(&json).expect("应可反序列化");
        assert_eq!(parsed, geometry);
        assert!(parsed.is_usable());
    }

    #[test]
    fn empty_record_is_never_written() {
        // 从未观测到真实尺寸时不许落盘，否则退出会把上一次的几何抹成 0。
        assert!(!WindowGeometry::default().is_usable());
    }
}
