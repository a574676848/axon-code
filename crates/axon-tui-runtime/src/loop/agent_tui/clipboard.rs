//! Agent TUI 剪贴板读写能力。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use image::{ImageBuffer, Rgba};

pub(super) const CLIPBOARD_IMAGE_FILE_PREFIX: &str = "axon-clipboard-image";
// arboard 的独立实例不共享锁；Windows 原生剪贴板访问必须在进程内串行。
#[cfg(windows)]
static CLIPBOARD_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(target_os = "linux")]
const POWERSHELL_CLIPBOARD_IMAGE_SCRIPT: &str = "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $img = Get-Clipboard -Format Image; if ($img -ne $null) { $p=[System.IO.Path]::GetTempFileName(); $p = [System.IO.Path]::ChangeExtension($p,'png'); $img.Save($p,[System.Drawing.Imaging.ImageFormat]::Png); Write-Output $p } else { exit 1 }";

#[cfg(target_os = "linux")]
use axon_process::non_interactive_command;

#[derive(Debug, Clone, Default)]
pub(super) enum ClipboardBackend {
    #[default]
    System,
    #[cfg(test)]
    Memory {
        text: Option<String>,
        image: Option<arboard::ImageData<'static>>,
    },
}

impl ClipboardBackend {
    pub(super) fn copy_text(&mut self, text: &str) -> Result<(), String> {
        match self {
            Self::System => copy_to_clipboard(text),
            #[cfg(test)]
            Self::Memory { text: stored, .. } => {
                let text = text.trim();
                if text.is_empty() {
                    return Err("没有可复制的内容".to_owned());
                }
                *stored = Some(text.to_owned());
                Ok(())
            }
        }
    }

    pub(super) fn read_text(&self) -> Result<String, String> {
        match self {
            Self::System => read_clipboard_text(),
            #[cfg(test)]
            Self::Memory { text, .. } => text
                .clone()
                .ok_or_else(|| "剪贴板中没有可读取的文本".to_owned()),
        }
    }

    pub(super) fn read_image(&self, image_dir: &Path) -> Result<PathBuf, String> {
        match self {
            Self::System => read_clipboard_image(image_dir),
            #[cfg(test)]
            Self::Memory { image, .. } => image
                .clone()
                .ok_or_else(|| "剪贴板中没有可读取的图片".to_owned())
                .and_then(|image| save_clipboard_image(image, image_dir)),
        }
    }
}

fn with_clipboard_access<T>(operation: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    let _clipboard_access = CLIPBOARD_ACCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    operation()
}

/// 将文本写入系统剪贴板。
pub(super) fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("没有可复制的内容".to_owned());
    }
    with_clipboard_access(|| {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("剪贴板不可用：{error}"))?;
        clipboard
            .set_text(trimmed.to_owned())
            .map_err(|error| format!("写入剪贴板失败：{error}"))
    })
}

pub(super) fn read_clipboard_text() -> Result<String, String> {
    with_clipboard_access(|| {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("剪贴板不可用：{error}"))?;
        read_clipboard_text_with(&mut clipboard)
    })
}

pub(super) fn read_clipboard_image(image_dir: &Path) -> Result<PathBuf, String> {
    with_clipboard_access(|| {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("剪贴板不可用：{error}"))?;
        read_clipboard_image_with(&mut clipboard, image_dir)
    })
}

fn read_clipboard_text_with(clipboard: &mut arboard::Clipboard) -> Result<String, String> {
    clipboard
        .get_text()
        .map_err(|error| format!("剪贴板中没有可读取的文本：{error}"))
}

fn read_clipboard_image_with(
    clipboard: &mut arboard::Clipboard,
    image_dir: &Path,
) -> Result<PathBuf, String> {
    if let Ok(path) = clipboard_file_list_image(clipboard, image_dir) {
        return Ok(path);
    }
    match clipboard.get_image() {
        Ok(image) => save_clipboard_image(image, image_dir),
        Err(error) => {
            #[cfg(target_os = "linux")]
            {
                wsl_clipboard_image_fallback(error.to_string())
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(format!("剪贴板中没有可读取的图片：{error}"))
            }
        }
    }
}

fn clipboard_file_list_image(
    clipboard: &mut arboard::Clipboard,
    image_dir: &Path,
) -> Result<PathBuf, String> {
    let files = clipboard
        .get()
        .file_list()
        .map_err(|error| format!("读取剪贴板文件列表失败：{error}"))?;
    let source = files
        .into_iter()
        .find(|path| image::open(path).is_ok())
        .ok_or_else(|| "剪贴板文件列表中没有图片".to_owned())?;
    persist_image_file(&source, image_dir)
}

fn save_clipboard_image(
    image: arboard::ImageData<'_>,
    image_dir: &Path,
) -> Result<PathBuf, String> {
    let width = u32::try_from(image.width).map_err(|_| "剪贴板图片宽度过大".to_owned())?;
    let height = u32::try_from(image.height).map_err(|_| "剪贴板图片高度过大".to_owned())?;
    let buffer = ImageBuffer::<Rgba<u8>, _>::from_raw(width, height, image.bytes.into_owned())
        .ok_or_else(|| "剪贴板图片数据不完整".to_owned())?;
    let path = image_dir.join(format!(
        "{CLIPBOARD_IMAGE_FILE_PREFIX}-{}.png",
        timestamp_millis()
    ));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建剪贴板图片目录失败：{error}"))?;
    }
    buffer
        .save(&path)
        .map_err(|error| format!("保存剪贴板图片失败：{error}"))?;
    Ok(path)
}

fn persist_image_file(source: &Path, image_dir: &Path) -> Result<PathBuf, String> {
    let image = image::open(source).map_err(|error| format!("读取剪贴板图片文件失败：{error}"))?;
    let path = image_dir.join(format!(
        "{CLIPBOARD_IMAGE_FILE_PREFIX}-{}.png",
        timestamp_millis()
    ));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建剪贴板图片目录失败：{error}"))?;
    }
    image
        .save(&path)
        .map_err(|error| format!("保存剪贴板图片失败：{error}"))?;
    Ok(path)
}

#[cfg(target_os = "linux")]
fn wsl_clipboard_image_fallback(original_error: String) -> Result<PathBuf, String> {
    if !is_probably_wsl() {
        return Err(format!("剪贴板中没有可读取的图片：{original_error}"));
    }
    let Some(windows_path) = dump_windows_clipboard_image() else {
        return Err(format!("剪贴板中没有可读取的图片：{original_error}"));
    };
    windows_path_to_wsl(&windows_path)
        .ok_or_else(|| format!("无法映射 Windows 剪贴板图片路径：{windows_path}"))
}

#[cfg(target_os = "linux")]
fn is_probably_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || fs::read_to_string("/proc/version")
            .map(|value| value.to_ascii_lowercase().contains("microsoft"))
            .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn dump_windows_clipboard_image() -> Option<String> {
    for command in ["powershell.exe", "pwsh", "powershell"] {
        let Ok(output) = non_interactive_command(command)
            .args(["-NoProfile", "-Command", POWERSHELL_CLIPBOARD_IMAGE_SCRIPT])
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn windows_path_to_wsl(value: &str) -> Option<PathBuf> {
    let mut chars = value.chars();
    let drive = chars.next()?.to_ascii_lowercase();
    if chars.next()? != ':' {
        return None;
    }
    let rest = chars.as_str().trim_start_matches(['\\', '/']);
    Some(PathBuf::from(format!(
        "/mnt/{}/{}",
        drive,
        rest.replace('\\', "/")
    )))
}

fn timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

#[cfg(test)]
#[path = "clipboard_tests.rs"]
mod tests;
