//! Composer 结构化弹层状态。

#[path = "composer_popup/candidate.rs"]
mod candidate;
#[path = "composer_popup/path_completion.rs"]
mod path_completion;
#[path = "composer_popup/search_catalog.rs"]
mod search_catalog;

use super::AgentTuiCatalog;
pub(crate) use candidate::{ComposerPopupItem, ComposerPopupKind, ComposerPopupMentionType};
use path_completion::path_like_token;
pub(crate) use path_completion::{PathCompletionKey, PathCompletionState};
use search_catalog::popup_items;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerPopup {
    pub(crate) kind: ComposerPopupKind,
    pub(crate) items: Vec<ComposerPopupItem>,
    pub(crate) selected: usize,
}

impl ComposerPopup {
    pub(crate) fn new(
        kind: ComposerPopupKind,
        filter: &str,
        cwd: &str,
        catalog: &AgentTuiCatalog,
    ) -> Self {
        let items = popup_items(kind, filter, cwd, catalog);
        Self {
            kind,
            items,
            selected: 0,
        }
    }

    pub(crate) fn from_items(kind: ComposerPopupKind, items: Vec<ComposerPopupItem>) -> Self {
        Self {
            kind,
            items,
            selected: 0,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn title(&self) -> &'static str {
        match self.kind {
            ComposerPopupKind::File => "Files",
            ComposerPopupKind::Image => "Images",
            ComposerPopupKind::Path => "Paths",
            ComposerPopupKind::Shell => "Shell",
            ComposerPopupKind::Skill => "Skills",
            ComposerPopupKind::Connector => "Connectors",
        }
    }

    pub(crate) fn select_previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
    }

    pub(crate) fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub(crate) fn selected_item(&self) -> Option<&ComposerPopupItem> {
        self.items.get(self.selected)
    }
}

pub(crate) fn popup_kind_for_draft(draft: &str) -> Option<(ComposerPopupKind, &str)> {
    let trimmed = draft.trim_start();
    if let Some(filter) = trimmed
        .strip_prefix("@file:")
        .or_else(|| (trimmed == "@file").then_some(""))
    {
        return Some((ComposerPopupKind::File, filter));
    }
    if let Some(filter) = trimmed
        .strip_prefix("@image:")
        .or_else(|| (trimmed == "@image").then_some(""))
    {
        return Some((ComposerPopupKind::Image, filter));
    }
    if let Some(filter) = trimmed.strip_prefix('@') {
        return Some((ComposerPopupKind::File, filter));
    }
    if let Some(filter) = trimmed.strip_prefix('$') {
        return Some((ComposerPopupKind::Skill, filter));
    }
    if let Some(filter) = trimmed.strip_prefix('!')
        && ("shell".starts_with(filter) || trimmed.starts_with("!shell"))
    {
        return Some((ComposerPopupKind::Shell, filter));
    }
    if path_like_token(trimmed) {
        return Some((ComposerPopupKind::Path, trimmed));
    }
    trimmed
        .strip_prefix('%')
        .map(|filter| (ComposerPopupKind::Connector, filter))
}
