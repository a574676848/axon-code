//! Agent TUI 草稿编辑能力。

use std::time::Instant;

use crate::app_shell::composer_dock::{
    COMPOSER_DOCK_SEPARATOR_HEIGHT, COMPOSER_DOCK_WORKING_STATUS_HEIGHT,
};
use crate::r#loop::agent_tui::{
    AgentTuiInputElementKind,
    draft_state::PasteDraftEffect,
    draft_text::has_multiple_visual_lines,
    large_paste::{PasteInsert, PendingPastes},
    state::AgentTuiState,
};

const COMPOSER_MIN_HEIGHT: u16 = 3;
const COMPOSER_MAX_DRAFT_LINES: u16 = 4;
const DEFAULT_TERMINAL_WIDTH: u16 = 80;

impl AgentTuiState {
    pub(crate) fn push(&mut self, value: char) {
        self.composer.draft.push_char(value);
        self.after_draft_changed();
    }

    pub(crate) fn insert_newline(&mut self) {
        self.flush_paste_burst_before_modified_input();
        self.insert_str("\n");
    }

    pub(crate) fn handle_plain_char(&mut self, value: char, now: Instant) {
        match self.composer.draft.handle_plain_char(value, now) {
            PasteDraftEffect::None => self.after_draft_changed(),
            PasteDraftEffect::InsertChar(value) => {
                self.composer.draft.push_char(value);
                self.after_draft_changed();
            }
            PasteDraftEffect::InsertPastedText(value) => self.insert_pasted_text(value),
        }
        self.fold_completed_clipboard_large_paste();
    }

    pub(crate) fn flush_paste_burst_if_due(&mut self, now: Instant) -> bool {
        let changed = match self.composer.draft.flush_paste_if_due(now) {
            PasteDraftEffect::None => false,
            PasteDraftEffect::InsertChar(value) => {
                self.push(value);
                true
            }
            PasteDraftEffect::InsertPastedText(value) => {
                if self.composer.draft.has_clipboard_large_paste() {
                    self.insert_str(&value);
                } else {
                    self.insert_pasted_text(value);
                }
                true
            }
        };
        self.fold_completed_clipboard_large_paste() || changed
    }

    pub(crate) fn flush_paste_burst_before_modified_input(&mut self) -> bool {
        let Some(value) = self.composer.draft.flush_paste_before_modified_input() else {
            return false;
        };
        if self.composer.draft.has_clipboard_large_paste() {
            self.insert_str(&value);
        } else {
            self.insert_pasted_text(value);
        }
        self.fold_completed_clipboard_large_paste();
        true
    }

    pub(crate) fn replace_draft(&mut self, value: String) {
        self.composer.draft.replace_all(value);
        self.after_draft_changed();
    }

    pub(crate) fn apply_external_edit(&mut self, value: String) {
        self.composer.draft.apply_external_edit(value);
        self.after_draft_changed();
    }

    pub(super) fn restore_submission_to_draft(
        &mut self,
        text: &str,
        submission: &super::AgentTuiSubmit,
    ) {
        self.composer.draft.replace_all(text.to_owned());
        for element in submission.input.elements() {
            match &element.kind {
                AgentTuiInputElementKind::File { path } => {
                    self.composer.draft.restore_attachment(
                        element.placeholder.clone(),
                        path.clone(),
                        crate::r#loop::agent_tui::attachments::AgentTuiAttachmentKind::File,
                    );
                }
                AgentTuiInputElementKind::Image { path } => {
                    self.composer.draft.restore_attachment(
                        element.placeholder.clone(),
                        path.clone(),
                        crate::r#loop::agent_tui::attachments::AgentTuiAttachmentKind::Image,
                    );
                }
                AgentTuiInputElementKind::Mention { .. }
                | AgentTuiInputElementKind::LargePaste { .. } => {}
            }
        }
        self.after_draft_changed();
    }

    pub(crate) fn backspace(&mut self) {
        self.composer.draft.backspace();
        self.after_draft_changed();
    }

    pub(crate) fn delete_char(&mut self) {
        self.composer.draft.delete_char();
        self.after_draft_changed();
    }

    pub(crate) fn kill_to_line_end(&mut self) {
        self.composer.draft.kill_to_line_end();
        self.after_draft_changed();
    }

    pub(crate) fn kill_to_line_start(&mut self) {
        self.composer.draft.kill_to_line_start();
        self.after_draft_changed();
    }

    pub(crate) fn kill_word_before_cursor(&mut self) {
        self.composer.draft.kill_word_before_cursor();
        self.after_draft_changed();
    }

    pub(crate) fn kill_word_after_cursor(&mut self) {
        self.composer.draft.kill_word_after_cursor();
        self.after_draft_changed();
    }

    pub(crate) fn move_cursor_left(&mut self) {
        self.composer.draft.move_cursor_left();
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_right(&mut self) {
        self.composer.draft.move_cursor_right();
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_word_left(&mut self) {
        self.composer.draft.move_cursor_word_left();
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_word_right(&mut self) {
        self.composer.draft.move_cursor_word_right();
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_home(&mut self) {
        self.composer.draft.move_cursor_home();
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_end(&mut self) {
        self.composer.draft.move_cursor_end();
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_up(&mut self) {
        self.composer
            .draft
            .move_cursor_up(self.composer_draft_content_width());
        self.after_cursor_moved();
    }

    pub(crate) fn move_cursor_down(&mut self) {
        self.composer
            .draft
            .move_cursor_down(self.composer_draft_content_width());
        self.after_cursor_moved();
    }

    pub(crate) fn clear_draft(&mut self) {
        self.composer.draft.clear();
        self.after_draft_changed();
    }

    pub(crate) fn composer_height(&self) -> u16 {
        let now = Instant::now();
        COMPOSER_MIN_HEIGHT
            .saturating_add(self.visible_draft_height().saturating_sub(1))
            .saturating_add(self.command_suggestion_panel_height())
            .saturating_add(self.queued_submit_preview_height())
            .saturating_add(if self.is_working() {
                COMPOSER_DOCK_WORKING_STATUS_HEIGHT
            } else {
                0
            })
            .saturating_add(self.composer_dock_panel_height())
            .saturating_add(self.subagent_activity_dock_height(now))
    }

    fn composer_dock_panel_height(&self) -> u16 {
        self.bottom_view_height()
    }

    fn subagent_activity_dock_height(&self, now: Instant) -> u16 {
        let rows = self.subagent_activity_visible_height(now);
        if rows == 0 {
            0
        } else {
            rows.saturating_add(COMPOSER_DOCK_SEPARATOR_HEIGHT.saturating_mul(2))
        }
    }

    pub(crate) fn cursor_row_col(&self) -> (u16, u16) {
        self.composer
            .draft
            .visual_cursor_row_col(self.composer_draft_content_width())
    }

    pub(crate) fn visible_cursor_row(&self) -> u16 {
        self.composer.draft.visible_visual_cursor_row(
            self.composer_draft_content_width(),
            self.visible_draft_height(),
        )
    }

    pub(crate) fn visible_draft_height(&self) -> u16 {
        self.composer.draft.visible_visual_height(
            self.composer_draft_content_width(),
            COMPOSER_MAX_DRAFT_LINES,
        )
    }

    pub(crate) fn draft_has_multiple_visual_lines(&self) -> bool {
        has_multiple_visual_lines(
            self.composer.draft.text(),
            self.composer_draft_content_width(),
        )
    }

    pub(crate) fn draft_cursor_is_on_first_visual_line(&self) -> bool {
        self.composer
            .draft
            .cursor_visual_row(self.composer_draft_content_width())
            == 0
    }

    pub(crate) fn draft_cursor_is_on_last_visual_line(&self) -> bool {
        let width = self.composer_draft_content_width();
        self.composer.draft.cursor_visual_row(width)
            == self
                .composer
                .draft
                .total_visual_lines(width)
                .saturating_sub(1)
    }

    /// 计算草稿顶部滚动偏移，让光标行始终落在可见区。
    pub(crate) fn draft_scroll_offset(&self, draft_height: u16) -> u16 {
        let width = self.composer_draft_content_width();
        let max_lines = draft_height.max(1) as usize;
        let total = self.composer.draft.total_visual_lines(width);
        if total <= max_lines {
            return 0;
        }
        let cursor_row = self.composer.draft.cursor_visual_row(width);
        let last_visible = total - 1;
        let first_visible = (max_lines - 1).min(last_visible);
        if cursor_row >= last_visible {
            return u16::try_from(last_visible - first_visible).unwrap_or(u16::MAX);
        }
        if cursor_row < first_visible {
            return u16::try_from(cursor_row).unwrap_or(u16::MAX);
        }
        let max_offset = last_visible - first_visible;
        let desired_first = cursor_row - first_visible;
        u16::try_from(desired_first.min(max_offset)).unwrap_or(u16::MAX)
    }

    pub(crate) fn insert_str(&mut self, value: &str) {
        self.composer.draft.insert_str(value);
        self.after_draft_changed();
    }

    pub(super) fn replace_draft_range(&mut self, start: usize, end: usize, replacement: &str) {
        self.composer.draft.replace_range(start, end, replacement);
        self.after_draft_changed();
    }

    pub(in crate::r#loop::agent_tui) fn insert_pasted_text(&mut self, value: String) {
        self.insert_pasted_text_impl(value);
    }

    fn insert_pasted_text_impl(&mut self, value: String) {
        match PendingPastes::plan_insert(self.composer.draft.text(), value) {
            PasteInsert::Inline(text) => {
                self.composer.draft.insert_str(&text);
                self.after_draft_changed();
            }
            PasteInsert::Placeholder {
                placeholder,
                content,
                char_count,
            } => {
                self.composer
                    .draft
                    .push_pending_paste(placeholder.clone(), content);
                self.composer.draft.insert_str(&placeholder);
                self.after_draft_changed();
                self.push_history_item(
                    "粘贴",
                    "大段文本",
                    "输入",
                    Some(format!("{char_count} 字符")),
                );
            }
        }
    }

    pub(in crate::r#loop::agent_tui) fn fold_completed_clipboard_large_paste(&mut self) -> bool {
        let Some(paste) = self.composer.draft.take_completed_clipboard_large_paste() else {
            return false;
        };
        self.composer
            .draft
            .replace_range(paste.start, paste.end, "");
        self.insert_pasted_text(paste.content);
        true
    }

    fn after_draft_changed(&mut self) {
        self.composer.history_index = None;
        self.refresh_command_suggestions();
    }

    fn after_cursor_moved(&mut self) {
        self.refresh_command_suggestions();
    }

    fn composer_draft_content_width(&self) -> u16 {
        let terminal_width = if self.viewport.frame_size.width == 0 {
            DEFAULT_TERMINAL_WIDTH
        } else {
            self.viewport.frame_size.width
        };
        terminal_width
            .saturating_sub(composer_prompt_width())
            .max(1)
    }
}

pub(in crate::r#loop::agent_tui) use crate::r#loop::agent_tui::agent_tui_prompt_width as composer_prompt_width;

#[cfg(test)]
mod tests {
    use super::super::runtime_flow_test_support::test_state;

    #[test]
    fn composer_height_stays_compact_for_single_line_input() {
        let mut state = test_state();
        state.replace_draft("你好".to_owned());

        assert_eq!(state.composer_height(), 3);
        assert_eq!(state.visible_draft_height(), 1);
        assert_eq!(state.visible_cursor_row(), 0);
        assert_eq!(state.cursor_row_col(), (0, 4));
    }

    #[test]
    fn composer_height_caps_command_popup_to_nine_rows() {
        let mut state = test_state();
        state.replace_draft("/".to_owned());

        assert_eq!(state.command_suggestion_panel_height(), 9);
        assert_eq!(state.composer_height(), 12);
    }

    #[test]
    fn visible_cursor_row_tracks_tail_of_multiline_input() {
        let mut state = test_state();
        state.replace_draft("1\n2\n3\n4\n5\n6\n7".to_owned());

        assert_eq!(state.visible_draft_height(), 4);
        assert_eq!(state.visible_cursor_row(), 3);
    }

    #[test]
    fn composer_height_and_cursor_follow_soft_wrapped_tail() {
        let mut state = test_state();
        state.replace_draft("abcdefghi".to_owned());

        assert_eq!(state.composer.draft.visible_visual_height(8, 4), 2);
        assert_eq!(state.composer.draft.visible_visual_cursor_row(8, 2), 1);
        assert_eq!(state.composer.draft.visual_cursor_row_col(8), (1, 1));
    }

    #[test]
    fn narrow_terminal_uses_actual_width_for_soft_wrapping() {
        let mut state = test_state();
        state.set_frame_size(ratatui::layout::Size::new(12, 24));
        let content_width = 12_u16.saturating_sub(super::composer_prompt_width()).max(1);
        state.replace_draft("x".repeat(usize::from(content_width) + 1));

        assert_eq!(state.composer_draft_content_width(), content_width);
        assert!(state.draft_has_multiple_visual_lines());
        assert_eq!(state.cursor_row_col(), (1, 1));
    }

    #[test]
    fn draft_scroll_offset_zero_when_draft_fits() {
        let mut state = test_state();
        state.replace_draft("1\n2\n3".to_owned());
        assert_eq!(state.draft_scroll_offset(4), 0);
    }

    #[test]
    fn draft_scroll_offset_anchors_to_tail_when_cursor_on_last_line() {
        let mut state = test_state();
        state.replace_draft("1\n2\n3\n4\n5\n6\n7".to_owned());
        assert_eq!(state.draft_scroll_offset(4), 3);
    }

    #[test]
    fn draft_scroll_offset_scrolls_back_when_cursor_moves_to_top() {
        let mut state = test_state();
        state.replace_draft("1\n2\n3\n4\n5\n6\n7".to_owned());
        state.composer.draft.set_cursor(0);
        assert_eq!(state.draft_scroll_offset(4), 0);
    }
}
