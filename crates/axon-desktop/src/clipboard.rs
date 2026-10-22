//! 剪贴板：桌面端经原生通道读写。
//!
//! 刻意不用网页层的 `navigator.clipboard`：它对读取强制弹一次系统授权，
//! 对写入还要求窗口处于焦点，两者都会让右键菜单里的复制粘贴时灵时不灵。
//! 原生通道与系统菜单同源，没有任何网页权限介入。
//!
//! 与 TUI 同一条约束：arboard 的独立实例不共享锁，Windows 上必须在进程内串行。

/// arboard 的独立实例不共享锁；Windows 原生剪贴板访问必须在进程内串行。
#[cfg(windows)]
static CLIPBOARD_ACCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 串行进入剪贴板访问。
fn with_clipboard_access<T>(operation: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    let _clipboard_access = CLIPBOARD_ACCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    operation()
}

/// 读取剪贴板文本。
///
/// 剪贴板为空或只有图片时返回空串：没有可粘贴的文本不是错误，
/// 界面据此给出「没有内容」的提示即可。
pub fn read_text() -> Result<String, String> {
    with_clipboard_access(|| {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("剪贴板不可用：{error}"))?;
        match clipboard.get_text() {
            Ok(text) => Ok(text),
            Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
            Err(error) => Err(format!("读取剪贴板失败：{error}")),
        }
    })
}

/// 写入剪贴板文本。
pub fn write_text(text: &str) -> Result<(), String> {
    with_clipboard_access(|| {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("剪贴板不可用：{error}"))?;
        clipboard
            .set_text(text.to_owned())
            .map_err(|error| format!("写入剪贴板失败：{error}"))
    })
}

/// Tauri IPC 命令：读取剪贴板文本。
#[tauri::command]
pub fn clipboard_read_text() -> Result<String, String> {
    read_text()
}

/// Tauri IPC 命令：写入剪贴板文本。
#[tauri::command]
pub fn clipboard_write_text(text: String) -> Result<(), String> {
    write_text(&text)
}
