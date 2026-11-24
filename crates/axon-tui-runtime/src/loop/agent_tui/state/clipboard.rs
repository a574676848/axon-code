//! Agent TUI 当前上下文复制状态逻辑。

use std::path::PathBuf;
use std::time::Instant;

use super::super::{
    attachments::{AgentTuiAttachmentKind, validated_attachment},
    state::AgentTuiState,
};
use super::ToastKind;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn copy_visible_text_selection_to_clipboard(
        &mut self,
    ) -> bool {
        let mut clipboard = self.clipboard.clone();
        let copied = self.copy_visible_text_selection_with(|text| clipboard.copy_text(text));
        self.clipboard = clipboard;
        copied
    }

    pub(in crate::r#loop::agent_tui) fn paste_from_system_clipboard(&mut self) {
        match self.clipboard.read_text() {
            Ok(text) => self.paste(&text),
            Err(error) => self.push_local_error("剪贴板读取失败", error, None),
        }
    }

    pub(in crate::r#loop::agent_tui) fn paste_image_from_system_clipboard(&mut self) {
        match self.clipboard.read_image(&self.bridge.runtime_tmp_dir) {
            Ok(path) => self.attach_clipboard_image(path),
            Err(error) => self.push_local_error("剪贴板图片读取失败", error, None),
        }
    }

    pub(in crate::r#loop::agent_tui) fn copy_visible_text_selection_with(
        &mut self,
        copy: impl FnOnce(&str) -> Result<(), String>,
    ) -> bool {
        let Some(text) = self.visible_text_selection.finish() else {
            return false;
        };
        match copy(&text) {
            Ok(()) => self.push_toast(
                format!("已复制 {} 个字符", text.chars().count()),
                ToastKind::Success,
                Instant::now(),
            ),
            Err(error) => self.push_toast(
                format!("复制失败：{error}"),
                ToastKind::Error,
                Instant::now(),
            ),
        }
        true
    }

    fn attach_clipboard_image(&mut self, path: PathBuf) {
        match validated_attachment(&path.display().to_string(), AgentTuiAttachmentKind::Image) {
            Ok(attachment) => self
                .composer
                .draft
                .attach_file(attachment.as_path_buf(), attachment.kind),
            Err(error) => self.push_local_error("剪贴板图片无效", error, None),
        }
    }
}
