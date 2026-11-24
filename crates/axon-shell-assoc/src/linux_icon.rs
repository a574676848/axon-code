//! Linux 图标的**纯查找与编码**（只读文件系统，不启动任何进程）。
//!
//! 与 `linux.rs` 分开是为了可验证性：查找规则、尺寸优先顺序与编码上限只吃路径与字节，
//! 因此能在任意平台（含开发机上的 Windows 与 CI）用临时目录直接单测；
//! `linux.rs` 里剩下的才是真正只能在 Linux 上跑的环境变量与 XDG 目录展开。
//!
//! 规则与既有工作台产品一致：`Icon=` 是绝对路径就直接读；否则在各数据目录里按
//! `icons/hicolor/<尺寸>/apps/`（尺寸由大到小）→ `icons/hicolor/scalable/apps/` →
//! `pixmaps/` 的顺序找 PNG 或 SVG。

use std::path::{Path, PathBuf};

/// 图标主题里按尺寸从大到小查找的目录。
///
/// 与既有工作台产品同一份列表：它先给大图再缩，这里也按同一顺序找，
/// 因此同一台机器上两个产品取到的是同一个文件。
const ICON_SIZE_DIRS: &[&str] = &["512x512", "256x256", "128x128", "64x64", "48x48", "32x32"];

/// 图标候选的扩展名。
const ICON_SUFFIXES: &[&str] = &["png", "svg"];

/// 单个图标的最大字节数。
///
/// 图标只是装饰：菜单一次可能列十几个候选，一个几百 KB 的图标会把一次 IPC
/// 撑成几兆，宁可这一条没有图标。
const MAX_ICON_BYTES: u64 = 256 * 1024;

/// 把 `Icon=` 解析成 data URL；取不到时返回 `None`。
pub(crate) fn icon_data_url(data_dirs: &[PathBuf], icon: Option<&str>) -> Option<String> {
    let icon = icon.filter(|value| !value.is_empty())?;
    let path = if Path::new(icon).is_absolute() {
        PathBuf::from(icon)
    } else {
        find_themed_icon(data_dirs, icon)?
    };
    encode_icon(&path)
}

/// 在图标主题的常规目录里找一个图标名；找不到返回 `None`。
pub(crate) fn find_themed_icon(data_dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    for dir in data_dirs {
        let hicolor = dir.join("icons").join("hicolor");
        for size in ICON_SIZE_DIRS {
            for suffix in ICON_SUFFIXES {
                let candidate = hicolor
                    .join(size)
                    .join("apps")
                    .join(format!("{name}.{suffix}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        let scalable = hicolor
            .join("scalable")
            .join("apps")
            .join(format!("{name}.svg"));
        if scalable.is_file() {
            return Some(scalable);
        }
        for suffix in ICON_SUFFIXES {
            let candidate = dir.join("pixmaps").join(format!("{name}.{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 读图标文件并编码成 data URL。
///
/// 只认 PNG 与 SVG：其他格式（XPM、JPEG）浏览器不一定渲染得出来，而图标只是装饰，
/// 认不出来不如没有。读不出来或超过上限同样返回 `None`，绝不让一条候选的图标
/// 影响整张列表。
pub(crate) fn encode_icon(path: &Path) -> Option<String> {
    let content_type = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_ICON_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    use base64::Engine as _;
    Some(format!(
        "data:{content_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}
