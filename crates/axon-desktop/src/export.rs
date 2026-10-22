//! 正文导出：把一段 Markdown 正文存到用户指定的位置。
//!
//! 另存为对话框在 Rust 侧直接调官方插件，因此渲染层不必拥有文件系统权限；
//! 写入一律 UTF-8 无 BOM，与其他产出文件保持一致。

use tauri_plugin_dialog::DialogExt;

use crate::conversation::attachment::safe_name;

/// 把建议名收敛成以 `.md` 结尾的安全文件名。
///
/// 收敛规则与附件落地同源（单段文件名、剔除非法字符），因此导出不会因为
/// 调用方给了一个带目录成分的名字而写到别处去。
fn markdown_file_name(suggested: &str) -> String {
    let name = safe_name(suggested);
    if name.to_ascii_lowercase().ends_with(".md") {
        name
    } else {
        format!("{name}.md")
    }
}

/// Tauri IPC 命令：把正文另存为 Markdown 文件。
///
/// 返回写入的绝对路径；用户在对话框里取消时返回 `None`，取消不是错误。
#[tauri::command]
pub fn export_markdown(
    app: tauri::AppHandle,
    suggested_name: String,
    content: String,
) -> Result<Option<String>, String> {
    if content.trim().is_empty() {
        return Err("正文为空，无需导出".to_string());
    }
    let chosen = app
        .dialog()
        .file()
        .set_title("导出为 Markdown")
        .set_file_name(markdown_file_name(&suggested_name))
        .add_filter("Markdown", &["md"])
        .blocking_save_file();
    let Some(path) = chosen.and_then(|selected| selected.into_path().ok()) else {
        return Ok(None);
    };
    std::fs::write(&path, content.as_bytes()).map_err(|error| format!("写入文件失败：{error}"))?;
    Ok(Some(path.to_string_lossy().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggested_names_are_reduced_to_a_safe_markdown_file() {
        assert_eq!(
            markdown_file_name("assistant-20260101-120000.md"),
            "assistant-20260101-120000.md"
        );
        assert_eq!(markdown_file_name("../../etc/passwd"), "passwd.md");
        assert_eq!(markdown_file_name("C:\\tmp\\reply.MD"), "reply.MD");
        assert_eq!(markdown_file_name("   "), "attachment.md");
        assert!(!markdown_file_name("a:b*c?.md").contains([':', '*', '?']));
    }
}
