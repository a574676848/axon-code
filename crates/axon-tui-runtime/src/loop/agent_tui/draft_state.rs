//! Composer 草稿编辑状态。
//!
//! 草稿文本、光标、原子 mention 与粘贴占位属于同一个编辑器状态，必须一起移动、
//! 删除和提交，避免元数据与可见文本脱钩。

use std::{ops::Range, path::PathBuf, time::Instant};

use super::draft_elements::DraftElementStore;
pub(crate) use super::draft_elements::{DraftElementSnapshot, DraftElementVisualKind};
use super::{
    attachments::AgentTuiAttachmentKind,
    draft_text::{
        VerticalDirection, byte_index_at, current_line_end, current_line_start, next_word_end,
        previous_word_start, vertical_cursor_target, visual_cursor, visual_line_spans,
    },
    input::{AgentTuiInputElement, AgentTuiInputElementKind},
    paste_input::{CompletedClipboardLargePaste, PasteInput, PasteTextEffect},
};

#[derive(Debug, Clone, Default)]
pub(crate) struct DraftState {
    textarea: TextArea,
    paste_input: PasteInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TextArea {
    text: String,
    cursor: usize,
    preferred_column: Option<u16>,
    elements: DraftElementStore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PasteDraftEffect {
    None,
    InsertChar(char),
    InsertPastedText(String),
}

impl DraftState {
    pub(super) fn text(&self) -> &str {
        self.textarea.text()
    }

    pub(super) fn cursor(&self) -> usize {
        self.textarea.cursor()
    }

    #[cfg(test)]
    pub(super) fn set_cursor(&mut self, cursor: usize) {
        self.textarea.set_cursor(cursor);
    }

    pub(super) fn replace_all(&mut self, value: String) {
        self.textarea.replace_all(value);
        self.paste_input.clear_pending();
    }

    pub(super) fn apply_external_edit(&mut self, value: String) {
        if value == self.textarea.text() {
            return;
        }
        let attachments = self
            .textarea
            .snapshots()
            .into_iter()
            .filter_map(|snapshot| match snapshot.input.kind {
                AgentTuiInputElementKind::File { path } => Some((
                    snapshot.input.placeholder,
                    path,
                    AgentTuiAttachmentKind::File,
                )),
                AgentTuiInputElementKind::Image { path } => Some((
                    snapshot.input.placeholder,
                    path,
                    AgentTuiAttachmentKind::Image,
                )),
                AgentTuiInputElementKind::Mention { .. }
                | AgentTuiInputElementKind::LargePaste { .. } => None,
            })
            .collect::<Vec<_>>();
        self.replace_all(value);
        for (placeholder, path, kind) in attachments {
            self.textarea.restore_attachment(placeholder, path, kind);
        }
    }

    pub(super) fn clear(&mut self) {
        self.textarea.clear();
        self.paste_input.clear_pending();
    }

    pub(super) fn push_char(&mut self, value: char) {
        self.textarea.replace_atomic_element_before_insert();
        self.textarea.insert_char(value);
        self.retain_live_pending();
    }

    pub(super) fn insert_str(&mut self, value: &str) {
        self.textarea.replace_atomic_element_before_insert();
        self.textarea.insert_str(value);
        self.retain_live_pending();
    }

    pub(super) fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        self.textarea.replace_range(start, end, replacement);
        self.retain_live_pending();
    }

    pub(super) fn move_cursor_to_document_end(&mut self) {
        self.textarea
            .set_cursor(self.textarea.text().chars().count());
    }

    pub(super) fn insert_mention(
        &mut self,
        start: usize,
        text: &str,
        sigil: char,
        name: String,
        target: String,
    ) {
        self.textarea
            .insert_mention(start, text, sigil, name, target);
    }

    pub(super) fn attach_file(&mut self, path: PathBuf, kind: AgentTuiAttachmentKind) {
        let index = self.textarea.attachment_count();
        self.textarea.attach_file(index, path, kind);
        self.retain_live_pending();
    }

    pub(super) fn restore_attachment(
        &mut self,
        placeholder: String,
        path: PathBuf,
        kind: AgentTuiAttachmentKind,
    ) {
        self.textarea.restore_attachment(placeholder, path, kind);
    }

    pub(super) fn handle_plain_char(&mut self, value: char, now: Instant) -> PasteDraftEffect {
        self.textarea.reset_preferred_column();
        let (text, cursor) = self.textarea.text_and_cursor_mut();
        match self.paste_input.handle_plain_char(text, cursor, value, now) {
            PasteTextEffect::None => PasteDraftEffect::None,
            PasteTextEffect::InsertChar(value) => PasteDraftEffect::InsertChar(value),
            PasteTextEffect::InsertPastedText(value) => PasteDraftEffect::InsertPastedText(value),
        }
    }

    pub(super) fn flush_paste_if_due(&mut self, now: Instant) -> PasteDraftEffect {
        match self.paste_input.flush_if_due(now) {
            PasteTextEffect::None => PasteDraftEffect::None,
            PasteTextEffect::InsertChar(value) => PasteDraftEffect::InsertChar(value),
            PasteTextEffect::InsertPastedText(value) => PasteDraftEffect::InsertPastedText(value),
        }
    }

    pub(super) fn flush_paste_before_modified_input(&mut self) -> Option<String> {
        self.paste_input.flush_before_modified_input()
    }

    pub(super) fn append_newline_if_paste_active(&mut self, now: Instant) -> bool {
        self.paste_input.append_newline_if_active(now)
    }

    pub(super) fn paste_newline_should_insert(&self, now: Instant) -> bool {
        self.paste_input
            .newline_should_insert_instead_of_submit(now)
    }

    pub(super) fn observe_clipboard_multiline_prefix(&mut self, clipboard_text: &str) -> bool {
        self.paste_input
            .observe_clipboard_multiline_prefix(self.textarea.text(), clipboard_text)
    }

    pub(super) fn has_clipboard_large_paste(&self) -> bool {
        self.paste_input.has_clipboard_large_paste()
    }

    pub(super) fn take_completed_clipboard_large_paste(
        &mut self,
    ) -> Option<CompletedClipboardLargePaste> {
        self.paste_input
            .take_completed_clipboard_large_paste(self.textarea.text(), self.textarea.cursor())
    }

    pub(super) fn push_pending_paste(&mut self, placeholder: String, content: String) {
        self.paste_input.push_pending(placeholder, content);
    }

    pub(super) fn input_elements(&self, prompt: &str) -> Vec<AgentTuiInputElement> {
        let mut elements = self.textarea.input_elements(prompt);
        elements.extend(self.paste_input.pending_elements(prompt));
        elements
    }

    pub(super) fn element_snapshots(&self) -> Vec<DraftElementSnapshot> {
        self.textarea.snapshots()
    }

    pub(super) fn expand_pending_pastes(&self, prompt: &str) -> String {
        self.paste_input.expand_pending(prompt)
    }

    pub(super) fn backspace(&mut self) {
        self.textarea.backspace();
        self.retain_live_pending();
    }

    pub(super) fn delete_char(&mut self) {
        self.textarea.delete_char();
        self.retain_live_pending();
    }

    pub(super) fn kill_to_line_end(&mut self) {
        let line_end = current_line_end(self.text(), self.cursor());
        if self.cursor() == line_end {
            if line_end < self.text().chars().count() {
                self.kill_range(self.cursor(), line_end + 1);
            }
            return;
        }
        self.kill_range(self.cursor(), line_end);
    }

    pub(super) fn kill_to_line_start(&mut self) {
        let line_start = current_line_start(self.text(), self.cursor());
        if self.cursor() == line_start {
            if line_start > 0 {
                self.kill_range(line_start - 1, line_start);
                self.textarea.set_cursor(line_start - 1);
            }
            return;
        }
        self.kill_range(line_start, self.cursor());
        self.textarea.set_cursor(line_start);
    }

    pub(super) fn kill_word_before_cursor(&mut self) {
        let start = previous_word_start(self.text(), self.cursor());
        if start == self.cursor() {
            return;
        }
        self.kill_range(start, self.cursor());
        self.textarea.set_cursor(start);
    }

    pub(super) fn kill_word_after_cursor(&mut self) {
        let end = next_word_end(self.text(), self.cursor());
        if end != self.cursor() {
            self.textarea.replace_range(self.cursor(), end, "");
            self.retain_live_pending();
        }
    }

    pub(super) fn move_cursor_left(&mut self) {
        let cursor = self.textarea.cursor().saturating_sub(1);
        self.textarea
            .set_cursor(self.textarea.cursor_left_boundary(cursor));
    }

    pub(super) fn move_cursor_right(&mut self) {
        let cursor = (self.textarea.cursor() + 1).min(self.text().chars().count());
        self.textarea
            .set_cursor(self.textarea.cursor_right_boundary(cursor));
    }

    pub(super) fn move_cursor_word_left(&mut self) {
        let cursor = previous_word_start(self.text(), self.cursor());
        self.textarea
            .set_cursor(self.textarea.cursor_left_boundary(cursor));
    }

    pub(super) fn move_cursor_word_right(&mut self) {
        let cursor = next_word_end(self.text(), self.cursor());
        self.textarea
            .set_cursor(self.textarea.cursor_right_boundary(cursor));
    }

    pub(super) fn move_cursor_home(&mut self) {
        self.textarea
            .set_cursor(current_line_start(self.text(), self.cursor()));
    }

    pub(super) fn move_cursor_end(&mut self) {
        self.textarea
            .set_cursor(current_line_end(self.text(), self.cursor()));
    }

    pub(super) fn move_cursor_up(&mut self, width: u16) {
        self.move_cursor_vertically(VerticalDirection::Up, width);
    }

    pub(super) fn move_cursor_down(&mut self, width: u16) {
        self.move_cursor_vertically(VerticalDirection::Down, width);
    }

    pub(super) fn visual_cursor_row_col(&self, width: u16) -> (u16, u16) {
        let cursor = visual_cursor(self.text(), self.cursor(), width);
        (cursor.row, cursor.col)
    }

    pub(super) fn visible_visual_cursor_row(&self, width: u16, visible_height: u16) -> u16 {
        let cursor = visual_cursor(self.text(), self.cursor(), width);
        let draft_lines =
            u16::try_from(visual_line_spans(self.text(), width).len()).unwrap_or(u16::MAX);
        let first_visible = draft_lines.saturating_sub(visible_height);
        cursor.row.saturating_sub(first_visible)
    }

    pub(super) fn visible_visual_height(&self, width: u16, max_lines: u16) -> u16 {
        let draft_lines = u16::try_from(visual_line_spans(self.text(), width).len())
            .unwrap_or(max_lines)
            .min(max_lines);
        draft_lines.max(1)
    }

    pub(super) fn total_visual_lines(&self, width: u16) -> usize {
        visual_line_spans(self.text(), width).len()
    }

    pub(super) fn cursor_visual_row(&self, width: u16) -> usize {
        usize::from(visual_cursor(self.text(), self.cursor(), width).row)
    }

    fn move_cursor_vertically(&mut self, direction: VerticalDirection, width: u16) {
        let content_width = width.max(1);
        let current_column = visual_cursor(self.text(), self.cursor(), content_width).col;
        let preferred_column = self
            .textarea
            .preferred_column
            .unwrap_or(current_column)
            .min(content_width.saturating_sub(1));
        let target = vertical_cursor_target(
            self.text(),
            self.cursor(),
            direction,
            content_width,
            preferred_column,
        );
        self.textarea
            .set_cursor_vertically(target, preferred_column);
    }

    fn kill_range(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.textarea.replace_range(start, end, "");
        self.retain_live_pending();
    }

    fn retain_live_pending(&mut self) {
        self.paste_input.retain_live_pending(self.textarea.text());
    }
}

impl TextArea {
    fn text(&self) -> &str {
        &self.text
    }

    fn cursor(&self) -> usize {
        self.cursor
    }

    fn text_and_cursor_mut(&mut self) -> (&mut String, &mut usize) {
        (&mut self.text, &mut self.cursor)
    }

    fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.text.chars().count());
        self.reset_preferred_column();
    }

    fn set_cursor_vertically(&mut self, cursor: usize, preferred_column: u16) {
        self.cursor = cursor.min(self.text.chars().count());
        self.preferred_column = Some(preferred_column);
    }

    fn reset_preferred_column(&mut self) {
        self.preferred_column = None;
    }

    fn replace_all(&mut self, value: String) {
        self.text = value;
        self.elements.clear();
        self.cursor = self.text.chars().count();
        self.reset_preferred_column();
    }

    fn clear(&mut self) {
        self.text.clear();
        self.elements.clear();
        self.cursor = 0;
        self.reset_preferred_column();
    }

    fn insert_char(&mut self, value: char) {
        let index = byte_index_at(&self.text, self.cursor);
        self.text.insert(index, value);
        self.elements.apply_replace(self.cursor..self.cursor, 1);
        self.cursor += 1;
        self.reset_preferred_column();
    }

    fn insert_str(&mut self, value: &str) {
        let index = byte_index_at(&self.text, self.cursor);
        self.text.insert_str(index, value);
        self.elements
            .apply_replace(self.cursor..self.cursor, value.chars().count());
        self.cursor += value.chars().count();
        self.reset_preferred_column();
    }

    fn replace_range(&mut self, start: usize, end: usize, replacement: &str) {
        if start > end {
            return;
        }
        let bounded_start = start.min(self.text.chars().count());
        let bounded_end = end.min(self.text.chars().count());
        let start_byte = byte_index_at(&self.text, bounded_start);
        let end_byte = byte_index_at(&self.text, bounded_end);
        self.text.replace_range(start_byte..end_byte, replacement);
        self.elements
            .apply_replace(bounded_start..bounded_end, replacement.chars().count());
        self.cursor = bounded_start + replacement.chars().count();
        self.reset_preferred_column();
    }

    fn insert_mention(
        &mut self,
        start: usize,
        text: &str,
        sigil: char,
        name: String,
        target: String,
    ) {
        self.elements
            .insert_mention(start, text, sigil, name, target);
    }

    fn attach_file(&mut self, index: usize, path: PathBuf, kind: AgentTuiAttachmentKind) {
        self.cursor =
            self.elements
                .insert_attachment(self.cursor, index, path, kind, &mut self.text);
        self.reset_preferred_column();
    }

    fn restore_attachment(
        &mut self,
        placeholder: String,
        path: PathBuf,
        kind: AgentTuiAttachmentKind,
    ) {
        self.elements
            .restore_attachment(&self.text, placeholder, path, kind);
    }

    fn attachment_count(&self) -> usize {
        self.elements.attachment_count()
    }

    fn input_elements(&self, draft: &str) -> Vec<AgentTuiInputElement> {
        self.elements.input_elements(draft)
    }

    fn snapshots(&self) -> Vec<DraftElementSnapshot> {
        self.elements.snapshots(&self.text)
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some(range) = self.elements.atomic_range_touching_backspace(self.cursor) {
            let range = atomic_delete_range(&self.text, range);
            self.replace_range(range.start, range.end, "");
            return;
        }
        let start = byte_index_at(&self.text, self.cursor - 1);
        let end = byte_index_at(&self.text, self.cursor);
        self.text.replace_range(start..end, "");
        self.elements.apply_replace(self.cursor - 1..self.cursor, 0);
        self.cursor -= 1;
        self.reset_preferred_column();
    }

    fn delete_char(&mut self) {
        if self.cursor >= self.text.chars().count() {
            return;
        }
        if let Some(range) = self.elements.atomic_range_touching_delete(self.cursor) {
            let range = atomic_delete_range(&self.text, range);
            self.replace_range(range.start, range.end, "");
            return;
        }
        let start = byte_index_at(&self.text, self.cursor);
        let end = byte_index_at(&self.text, self.cursor + 1);
        self.text.replace_range(start..end, "");
        self.elements.apply_replace(self.cursor..self.cursor + 1, 0);
        self.reset_preferred_column();
    }

    fn replace_atomic_element_before_insert(&mut self) {
        let Some(range) = self.elements.atomic_range_containing_cursor(self.cursor) else {
            return;
        };
        self.replace_range(range.start, range.end, "");
    }

    fn cursor_left_boundary(&self, cursor: usize) -> usize {
        self.elements.cursor_left_boundary(cursor)
    }

    fn cursor_right_boundary(&self, cursor: usize) -> usize {
        self.elements.cursor_right_boundary(cursor)
    }
}

fn atomic_delete_range(value: &str, mut range: Range<usize>) -> Range<usize> {
    let chars = value.chars().collect::<Vec<_>>();
    if range.start > 0
        && range.end < chars.len()
        && chars[range.start - 1].is_whitespace()
        && chars[range.end].is_whitespace()
    {
        range.end += 1;
    }
    range
}

#[cfg(test)]
#[path = "draft_state/draft_state_tests.rs"]
mod draft_state_tests;
