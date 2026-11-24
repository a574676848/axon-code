//! 大段粘贴内容的占位、结构化元素与提交展开。

use super::input::AgentTuiInputElement;

const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PendingPastes {
    items: Vec<PendingPaste>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingPaste {
    placeholder: String,
    content: String,
}

pub(super) enum PasteInsert {
    Inline(String),
    Placeholder {
        placeholder: String,
        content: String,
        char_count: usize,
    },
}

impl PendingPastes {
    pub(super) fn requires_placeholder(value: &str) -> bool {
        value.chars().count() > LARGE_PASTE_CHAR_THRESHOLD
    }

    pub(super) fn plan_insert(draft: &str, value: String) -> PasteInsert {
        let char_count = value.chars().count();
        if char_count <= LARGE_PASTE_CHAR_THRESHOLD {
            return PasteInsert::Inline(value);
        }
        PasteInsert::Placeholder {
            placeholder: next_large_paste_placeholder(draft, char_count),
            content: value,
            char_count,
        }
    }

    pub(super) fn push(&mut self, placeholder: String, content: String) {
        self.items.push(PendingPaste {
            placeholder,
            content,
        });
    }

    pub(super) fn clear(&mut self) {
        self.items.clear();
    }

    pub(super) fn retain_live(&mut self, draft: &str) {
        self.items
            .retain(|paste| draft.contains(&paste.placeholder));
    }

    pub(super) fn expand(&self, prompt: &str) -> String {
        self.items.iter().fold(prompt.to_owned(), |text, paste| {
            text.replace(&paste.placeholder, &paste.content)
        })
    }

    pub(super) fn elements(&self, prompt: &str) -> Vec<AgentTuiInputElement> {
        self.items
            .iter()
            .filter(|paste| prompt.contains(&paste.placeholder))
            .map(|paste| {
                AgentTuiInputElement::large_paste(paste.placeholder.clone(), paste.content.clone())
            })
            .collect()
    }
}

fn next_large_paste_placeholder(draft: &str, char_count: usize) -> String {
    let base = format!("[Pasted Content {char_count} chars]");
    if !draft.contains(&base) {
        return base;
    }
    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base} #{suffix}");
        if !draft.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}
