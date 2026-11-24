//! 粘贴输入控制器，统一处理普通字符突发粘贴与大段粘贴占位。

use std::time::Instant;

use super::{
    input::AgentTuiInputElement,
    large_paste::PendingPastes,
    paste_burst::{CharDecision, FlushResult, PasteBurst},
};

#[derive(Debug, Clone, Default)]
pub(super) struct PasteInput {
    burst: PasteBurst,
    pending: PendingPastes,
    clipboard_large_paste: Option<ClipboardLargePasteCandidate>,
}

#[derive(Debug, Clone)]
struct ClipboardLargePasteCandidate {
    start: usize,
    end: usize,
    content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletedClipboardLargePaste {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PasteTextEffect {
    None,
    InsertChar(char),
    InsertPastedText(String),
}

impl PasteInput {
    pub(super) fn handle_plain_char(
        &mut self,
        draft: &mut String,
        cursor: &mut usize,
        value: char,
        now: Instant,
    ) -> PasteTextEffect {
        if !value.is_ascii() {
            if self.burst.append_non_ascii_char_if_buffering(value, now) {
                return PasteTextEffect::None;
            }
            self.burst.note_unbuffered_char(now);
            return PasteTextEffect::InsertChar(value);
        }
        match self.burst.on_plain_char(value, now) {
            CharDecision::RetainFirstChar => PasteTextEffect::None,
            CharDecision::BeginBufferFromPending | CharDecision::BufferAppend => {
                self.burst.append_char_to_buffer(value, now);
                PasteTextEffect::None
            }
            CharDecision::BeginBuffer { retro_chars } => {
                let before_cursor = byte_index_at(draft, *cursor);
                if let Some(grab) = self.burst.decide_begin_buffer(
                    now,
                    &draft[..before_cursor],
                    usize::from(retro_chars),
                ) {
                    draft.replace_range(grab.start_byte..before_cursor, "");
                    *cursor = cursor.saturating_sub(grab.grabbed.chars().count());
                    self.burst.append_char_to_buffer(value, now);
                    PasteTextEffect::None
                } else {
                    PasteTextEffect::InsertChar(value)
                }
            }
        }
    }

    pub(super) fn flush_if_due(&mut self, now: Instant) -> PasteTextEffect {
        match self.burst.flush_if_due(now) {
            FlushResult::None => PasteTextEffect::None,
            FlushResult::Typed(value) => PasteTextEffect::InsertChar(value),
            FlushResult::Paste(value) => PasteTextEffect::InsertPastedText(value),
        }
    }

    pub(super) fn flush_before_modified_input(&mut self) -> Option<String> {
        self.burst.flush_before_modified_input()
    }

    pub(super) fn append_newline_if_active(&mut self, now: Instant) -> bool {
        self.burst.append_newline_if_active(now)
    }

    pub(super) fn newline_should_insert_instead_of_submit(&self, now: Instant) -> bool {
        self.burst.newline_should_insert_instead_of_submit(now)
            || self.burst.unbuffered_newline_should_insert(now)
    }

    pub(super) fn observe_clipboard_multiline_prefix(
        &mut self,
        draft: &str,
        clipboard_text: &str,
    ) -> bool {
        let normalized = normalize_clipboard_text(clipboard_text);
        if normalized.is_empty() || draft.ends_with(&normalized) {
            return false;
        }
        let Some((newline_index, _)) = normalized.match_indices('\n').find(|(newline_index, _)| {
            *newline_index > 0 && draft.ends_with(&normalized[..*newline_index])
        }) else {
            return false;
        };
        if PendingPastes::requires_placeholder(&normalized) {
            let pasted_prefix_chars = normalized[..newline_index].chars().count();
            let start = draft.chars().count().saturating_sub(pasted_prefix_chars);
            let end = start + normalized.chars().count();
            let candidate = ClipboardLargePasteCandidate {
                start,
                end,
                content: normalized,
            };
            if self
                .clipboard_large_paste
                .as_ref()
                .is_none_or(|current| current.content != candidate.content)
            {
                self.clipboard_large_paste = Some(candidate);
            }
        }
        true
    }

    pub(super) fn has_clipboard_large_paste(&self) -> bool {
        self.clipboard_large_paste.is_some()
    }

    pub(super) fn take_completed_clipboard_large_paste(
        &mut self,
        draft: &str,
        cursor: usize,
    ) -> Option<CompletedClipboardLargePaste> {
        let candidate = self.clipboard_large_paste.as_ref()?;
        if cursor < candidate.end {
            return None;
        }
        if cursor > candidate.end {
            self.clipboard_large_paste = None;
            return None;
        }
        let start_byte = byte_index_at(draft, candidate.start);
        let end_byte = byte_index_at(draft, candidate.end);
        if draft.get(start_byte..end_byte) != Some(candidate.content.as_str()) {
            self.clipboard_large_paste = None;
            return None;
        }
        let candidate = self.clipboard_large_paste.take()?;
        self.burst.clear_after_explicit_paste();
        Some(CompletedClipboardLargePaste {
            start: candidate.start,
            end: candidate.end,
            content: candidate.content,
        })
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
        self.clipboard_large_paste = None;
    }

    pub(super) fn retain_live_pending(&mut self, draft: &str) {
        self.pending.retain_live(draft);
    }

    pub(super) fn expand_pending(&self, prompt: &str) -> String {
        self.pending.expand(prompt)
    }

    pub(super) fn pending_elements(&self, prompt: &str) -> Vec<AgentTuiInputElement> {
        self.pending.elements(prompt)
    }

    pub(super) fn push_pending(&mut self, placeholder: String, content: String) {
        self.pending.push(placeholder, content);
    }
}

fn byte_index_at(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

fn normalize_clipboard_text(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{PasteInput, PasteTextEffect};

    #[test]
    fn single_unicode_character_is_inserted_immediately() {
        let now = Instant::now();
        let mut input = PasteInput::default();
        let mut draft = String::new();
        let mut cursor = 0;

        assert_eq!(
            input.handle_plain_char(&mut draft, &mut cursor, '你', now),
            PasteTextEffect::InsertChar('你')
        );
    }

    #[test]
    fn mixed_ascii_unicode_paste_preserves_original_order() {
        let now = Instant::now();
        let mut input = PasteInput::default();
        let mut draft = String::new();
        let mut cursor = 0;
        let expected = "EAIP智能打单连接器_管理配置原型.html";

        for (offset, value) in expected.chars().enumerate() {
            assert_eq!(
                input.handle_plain_char(
                    &mut draft,
                    &mut cursor,
                    value,
                    now + Duration::from_millis(offset as u64)
                ),
                PasteTextEffect::None
            );
        }

        assert_eq!(
            input.flush_if_due(now + Duration::from_millis(100)),
            PasteTextEffect::InsertPastedText(expected.to_owned())
        );
    }

    #[test]
    fn clipboard_multiline_prefix_only_consumes_pending_source_newlines() {
        let clipboard = "第一行\r\n第二行\r\n第三行";
        let mut input = PasteInput::default();

        assert!(input.observe_clipboard_multiline_prefix("已有草稿：第一行", clipboard));
        assert!(input.observe_clipboard_multiline_prefix("已有草稿：第一行\n第二行", clipboard));
        assert!(
            !input
                .observe_clipboard_multiline_prefix("已有草稿：第一行\n第二行\n第三行", clipboard)
        );
        assert!(!input.observe_clipboard_multiline_prefix("无关输入", clipboard));
    }
}
