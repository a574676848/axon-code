//! 「用其他应用打开」候选枚举：按平台查询系统注册的打开方式，并尽力提供应用图标。
//!
//! 本 crate 是一个**端口**：公开 API 与 [`FileApplication`] 的数据结构平台中立，
//! 具体查询由 `#[cfg(target_os = ...)]` 分派到平台适配器：
//!
//! - `windows`：Windows Shell 的关联处理程序枚举（`SHAssocEnumHandlers` + `IAssocHandler`）。
//! - `linux`：XDG 桌面条目枚举（各数据目录 `applications/*.desktop` + `mimeapps.list`）。
//! - `macos`：LaunchServices 枚举，经系统自带的 `osascript`（JXA）读取 `NSWorkspace`。
//!
//! 三个适配器都实现同一形状 `fn applications_for(&Path) -> Result<Vec<FileApplication>, String>`
//! 与 `fn open_with(&Path, &str) -> Result<(), String>`，
//! 之上再套一层平台中立的去重（[`normalize_candidates`]），**顺序原样沿用适配器**：
//! 适配器给出的就是系统自己的偏好顺序。新增平台只需补一个适配器，
//! 公开契约不变；未覆盖的目标平台返回空表而不是编译失败。
//!
//! # 字段语义（跨平台约定）
//!
//! - [`FileApplication::id`]：该平台上稳定的标识：Windows 是可执行文件绝对路径
//!   （打包应用为 AUMID），Linux 是桌面条目 id（`org.gnome.TextEditor.desktop` 形式）。
//! - [`FileApplication::default`]：**系统自己认定**的默认打开方式，与 Axon 的配置无关。
//! - [`FileApplication::icon`]：目前只有 Windows 适配器会填充；Linux / macOS 留空，
//!   缺口见各适配器模块文档。
//!
//! # 失败方向
//!
//! - **枚举整体失败**返回 `Err`（例如 Shell 拒绝枚举、平台 API 报错），由调用方决定提示方式。
//! - **单项失败一律降级**：某条候选取不到名字就跳过它，图标取不到就留 `None`，
//!   绝不因为一条候选异常而丢掉整张列表。
//! - 任何路径都不会 `panic`：生产代码里没有 `unwrap` / `expect`。
//!
//! # unsafe 的边界
//!
//! `unsafe_code` 在本 crate 被显式放开（见 `Cargo.toml`），但许可范围被 `shell_assoc_tests`
//! 模块的 `unsafe_code_is_confined_to_the_windows_adapters` 约束在 `src/windows.rs` 与
//! `src/windows_icon.rs` 两个文件内；公开 API、纯逻辑与 Linux / macOS 适配器全是安全代码。

use std::{collections::HashSet, path::Path};

#[cfg(any(target_os = "linux", test))]
mod desktop_entry;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "linux", test))]
mod linux_icon;
#[cfg(test)]
mod linux_icon_tests;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod macos_applications;
#[cfg(test)]
mod macos_tests;
#[cfg(any(target_os = "linux", test))]
mod mime_glob;
#[cfg(test)]
mod shell_assoc_tests;
#[cfg(windows)]
mod windows;
#[cfg(any(windows, test))]
mod windows_helpers;
#[cfg(windows)]
mod windows_icon;
#[cfg(all(test, windows))]
mod windows_tests;

/// 候选图标的边长（像素）。
///
/// 顶栏与文件预览都以 32 逻辑像素渲染图标，因此提取与编码统一按这个尺寸做：
/// Windows 侧用它向 Shell 请求图标，尺寸不符时按它缩放（见 `windows_icon`）。
pub const ICON_SIZE: u32 = 32;

/// 一条与某个文件关联的打开方式候选。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FileApplication {
    /// 平台稳定的标识：Windows 为可执行文件绝对路径（打包应用为 AUMID），
    /// Linux 为桌面条目 id。用于去重与回传「用户选中了哪一个」。
    pub id: String,
    /// 展示名；系统没有给出名字时回退为该标识的文件名部分。
    pub name: String,
    /// [`ICON_SIZE`]×[`ICON_SIZE`] 图标的 data URL：
    /// Windows / macOS 是 `data:image/png;base64,...`，Linux 还可能是 SVG；
    /// 图标取不到时为 `None`。
    pub icon: Option<String>,
    /// 是否为该系统认定的默认打开方式。
    pub default: bool,
}

/// 列出某个文件的候选打开方式；无文件名或无任何关联时返回空表。
///
/// # Errors
///
/// 平台适配器报告枚举整体失败时返回错误描述（中文，可直接展示）。
/// 单条候选的问题不会变成错误，只会让该候选被跳过或让它的图标为空。
pub fn applications_for(path: &Path) -> Result<Vec<FileApplication>, String> {
    #[cfg(target_os = "windows")]
    let candidates = windows::applications_for(path)?;
    #[cfg(target_os = "linux")]
    let candidates = linux::applications_for(path)?;
    #[cfg(target_os = "macos")]
    let candidates = macos::applications_for(path)?;
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let candidates = {
        // 没有适配器的目标平台返回空表，让调用方隐藏入口，而不是让整个工程编译不过。
        let _ = path;
        Vec::new()
    };
    Ok(normalize_candidates(candidates))
}

/// 用系统登记的一条打开方式打开文件。
///
/// `application_id` 必须来自 [`applications_for`] 返回的 [`FileApplication::id`]：适配器按标识
/// 回找**系统自己登记**的那条处理程序，界面因此交不出任意程序。标识不属于该文件的关联列表时
/// 如实报错，绝不退回「随便找一个」。
///
/// # Errors
///
/// 标识未登记、目标不是文件，或平台适配器启动失败时返回错误描述（中文，可直接展示）。
pub fn open_with(path: &Path, application_id: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let launched = windows::open_with(path, application_id);
    #[cfg(target_os = "linux")]
    let launched = linux::open_with(path, application_id);
    #[cfg(target_os = "macos")]
    let launched = macos::open_with(path, application_id);
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let launched = {
        let _ = (path, application_id);
        Err("当前平台不支持按应用打开文件".to_string())
    };
    launched
}

/// 候选去重：按 [`FileApplication::id`] 去重，保留**先出现**的候选。
///
/// **不排序**：顺序就是平台适配器给出的顺序，也就是**系统自己的偏好顺序**
/// （Windows 的 `SHAssocEnumHandlers`、macOS 的 `URLsForApplicationsToOpenURL`、
/// Linux 的 `mimeapps.list` 都自带优先级）。这与 DSH 的参考实现一致：它把系统给出的
/// 顺序原样交给菜单，默认项只加标记、不搬家。按展示名重排会把这个信息抹掉：
/// 「系统推荐谁」会被换成「字典序谁在前」。
///
/// 去重保留先出现的那条：适配器按自身优先级返回，先出现的更可信。
/// `id` 为空的候选（取不到标识，无法回传「选中了哪一个」）直接丢弃。
fn normalize_candidates(candidates: Vec<FileApplication>) -> Vec<FileApplication> {
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|candidate| !candidate.id.is_empty() && seen.insert(candidate.id.to_lowercase()))
        .collect()
}
