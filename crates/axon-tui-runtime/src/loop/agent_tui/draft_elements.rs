//! Composer 草稿结构化元素存储。

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use super::{
    attachments::AgentTuiAttachmentKind,
    draft_text::{byte_index_at, find_char_range, shift_index},
    input::{AgentTuiInputElement, AgentTuiInputElementKind, file_placeholder, image_placeholder},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DraftElementSnapshot {
    pub(super) id: u64,
    pub(super) range: Range<usize>,
    pub(super) text: String,
    pub(super) visual: DraftElementVisualKind,
    pub(super) input: AgentTuiInputElement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraftElementVisualKind {
    Mention(char),
    File,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DraftElementStore {
    next_id: u64,
    elements: Vec<DraftElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DraftElement {
    id: u64,
    range: Range<usize>,
    kind: DraftElementKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DraftElementKind {
    Mention {
        sigil: char,
        name: String,
        target: String,
    },
    Attachment {
        placeholder: String,
        path: PathBuf,
        kind: AgentTuiAttachmentKind,
    },
}

impl DraftElementStore {
    pub(super) fn clear(&mut self) {
        self.elements.clear();
    }

    pub(super) fn insert_mention(
        &mut self,
        start: usize,
        text: &str,
        sigil: char,
        name: String,
        target: String,
    ) {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let end = start.saturating_add(text.chars().count());
        self.elements.push(DraftElement {
            id,
            range: start..end,
            kind: DraftElementKind::Mention {
                sigil,
                name,
                target,
            },
        });
        self.elements.sort_by_key(|element| element.range.start);
    }

    pub(super) fn insert_attachment(
        &mut self,
        cursor: usize,
        index: usize,
        path: PathBuf,
        kind: AgentTuiAttachmentKind,
        text: &mut String,
    ) -> usize {
        let placeholder = attachment_placeholder(index, &path, kind);
        let start_byte = byte_index_at(text, cursor);
        let prefix = if cursor == 0 { "" } else { " " };
        let suffix = if cursor >= text.chars().count() {
            ""
        } else {
            " "
        };
        let inserted = format!("{prefix}{placeholder}{suffix}");
        text.insert_str(start_byte, &inserted);
        let start = cursor + prefix.chars().count();
        let end = start + placeholder.chars().count();
        self.apply_replace(cursor..cursor, inserted.chars().count());
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.elements.push(DraftElement {
            id,
            range: start..end,
            kind: DraftElementKind::Attachment {
                placeholder,
                path,
                kind,
            },
        });
        self.elements.sort_by_key(|element| element.range.start);
        cursor + inserted.chars().count()
    }

    pub(super) fn attachment_count(&self) -> usize {
        self.elements
            .iter()
            .filter(|element| matches!(element.kind, DraftElementKind::Attachment { .. }))
            .count()
    }

    pub(super) fn restore_attachment(
        &mut self,
        text: &str,
        placeholder: String,
        path: PathBuf,
        kind: AgentTuiAttachmentKind,
    ) {
        let Some(start) = find_char_range(text, &placeholder) else {
            return;
        };
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let end = start + placeholder.chars().count();
        self.elements.push(DraftElement {
            id,
            range: start..end,
            kind: DraftElementKind::Attachment {
                placeholder,
                path,
                kind,
            },
        });
        self.elements.sort_by_key(|element| element.range.start);
    }

    pub(super) fn apply_replace(&mut self, range: Range<usize>, replacement_len: usize) {
        let original_len = range.end.saturating_sub(range.start);
        self.elements
            .retain(|element| element.range.end <= range.start || element.range.start >= range.end);
        for element in &mut self.elements {
            if element.range.start < range.end {
                continue;
            }
            element.range.start = shift_index(element.range.start, original_len, replacement_len);
            element.range.end = shift_index(element.range.end, original_len, replacement_len);
        }
    }

    pub(super) fn input_elements(&self, draft: &str) -> Vec<AgentTuiInputElement> {
        self.snapshots(draft)
            .into_iter()
            .map(|snapshot| snapshot.input)
            .collect()
    }

    pub(super) fn snapshots(&self, draft: &str) -> Vec<DraftElementSnapshot> {
        self.elements
            .iter()
            .filter_map(|element| element.snapshot(draft))
            .collect()
    }

    pub(super) fn atomic_range_touching_backspace(&self, cursor: usize) -> Option<Range<usize>> {
        if cursor == 0 {
            return None;
        }
        self.elements
            .iter()
            .find(|element| cursor > element.range.start && cursor <= element.range.end)
            .map(|element| element.range.clone())
    }

    pub(super) fn atomic_range_touching_delete(&self, cursor: usize) -> Option<Range<usize>> {
        self.elements
            .iter()
            .find(|element| cursor >= element.range.start && cursor < element.range.end)
            .map(|element| element.range.clone())
    }

    pub(super) fn atomic_range_containing_cursor(&self, cursor: usize) -> Option<Range<usize>> {
        self.elements
            .iter()
            .find(|element| cursor > element.range.start && cursor < element.range.end)
            .map(|element| element.range.clone())
    }

    pub(super) fn cursor_left_boundary(&self, cursor: usize) -> usize {
        self.elements
            .iter()
            .find(|element| cursor > element.range.start && cursor <= element.range.end)
            .map_or(cursor, |element| element.range.start)
    }

    pub(super) fn cursor_right_boundary(&self, cursor: usize) -> usize {
        self.elements
            .iter()
            .find(|element| cursor >= element.range.start && cursor < element.range.end)
            .map_or(cursor, |element| element.range.end)
    }
}

impl DraftElement {
    fn input_element(&self) -> AgentTuiInputElement {
        match &self.kind {
            DraftElementKind::Mention {
                sigil,
                name,
                target,
            } => AgentTuiInputElement::mention(*sigil, name.clone(), target.clone()),
            DraftElementKind::Attachment { path, kind, .. } => {
                AgentTuiInputElement::attachment(0, path.clone(), *kind)
            }
        }
    }

    fn snapshot(&self, draft: &str) -> Option<DraftElementSnapshot> {
        let text = draft
            .chars()
            .skip(self.range.start)
            .take(self.range.end.saturating_sub(self.range.start))
            .collect::<String>();
        match &self.kind {
            DraftElementKind::Mention { sigil, name, .. } if text == format!("{sigil}{name}") => {
                Some(DraftElementSnapshot {
                    id: self.id,
                    range: self.range.clone(),
                    text,
                    visual: DraftElementVisualKind::Mention(*sigil),
                    input: self.input_element(),
                })
            }
            DraftElementKind::Mention { .. } => None,
            DraftElementKind::Attachment {
                placeholder,
                path,
                kind,
            } if text == *placeholder => Some(DraftElementSnapshot {
                id: self.id,
                range: self.range.clone(),
                text,
                visual: attachment_visual_kind(*kind),
                input: AgentTuiInputElement {
                    placeholder: placeholder.clone(),
                    kind: match kind {
                        AgentTuiAttachmentKind::File => {
                            AgentTuiInputElementKind::File { path: path.clone() }
                        }
                        AgentTuiAttachmentKind::Image => {
                            AgentTuiInputElementKind::Image { path: path.clone() }
                        }
                    },
                },
            }),
            DraftElementKind::Attachment { .. } => None,
        }
    }
}

fn attachment_placeholder(index: usize, path: &Path, kind: AgentTuiAttachmentKind) -> String {
    match kind {
        AgentTuiAttachmentKind::File => file_placeholder(index, path),
        AgentTuiAttachmentKind::Image => image_placeholder(index, path),
    }
}

fn attachment_visual_kind(kind: AgentTuiAttachmentKind) -> DraftElementVisualKind {
    match kind {
        AgentTuiAttachmentKind::File => DraftElementVisualKind::File,
        AgentTuiAttachmentKind::Image => DraftElementVisualKind::Image,
    }
}
