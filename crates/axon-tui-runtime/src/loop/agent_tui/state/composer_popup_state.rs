//! Composer 结构化弹层状态逻辑。

use super::super::{
    composer_popup::{ComposerPopup, PathCompletionKey, popup_kind_for_draft},
    state::AgentTuiState,
};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn select_previous_composer_popup_item(&mut self) {
        if let Some(popup) = self.composer.composer_popup.as_mut() {
            popup.select_previous();
        }
    }

    pub(in crate::r#loop::agent_tui) fn select_next_composer_popup_item(&mut self) {
        if let Some(popup) = self.composer.composer_popup.as_mut() {
            popup.select_next();
        }
    }

    pub(in crate::r#loop::agent_tui) fn close_composer_popup(&mut self) {
        self.composer.composer_popup = None;
    }

    pub(in crate::r#loop::agent_tui) fn accept_selected_composer_popup_item(&mut self) -> bool {
        let Some(item) = self
            .composer
            .composer_popup
            .as_ref()
            .and_then(|popup| popup.selected_item())
            .cloned()
        else {
            return false;
        };
        let insert_text = item.insert_text.trim_end();
        let token = current_draft_token(self.composer.draft.text(), self.composer.draft.cursor());
        let replacement = format!("{insert_text} ");
        let replace_end = mention_replace_end(self.composer.draft.text(), token.range.end);
        self.replace_draft_range(token.range.start, replace_end, &replacement);
        if let Some((sigil, name)) = mention_from_insert_text(insert_text) {
            self.composer.draft.insert_mention(
                token.range.start,
                insert_text,
                sigil,
                name.to_owned(),
                item.target,
            );
        }
        self.close_composer_popup();
        self.composer.history_index = None;
        self.refresh_command_suggestions();
        true
    }

    pub(super) fn refresh_composer_popup(&mut self) {
        if self.composer.draft.text().trim_start().starts_with('/') {
            self.composer.composer_popup = None;
            self.composer.path_completion.clear();
            return;
        }
        let token = current_draft_token(self.composer.draft.text(), self.composer.draft.cursor());
        let Some((kind, filter)) = popup_kind_for_draft(&token.text) else {
            self.composer.composer_popup = None;
            self.composer.path_completion.clear();
            return;
        };
        let popup = if let Some(key) =
            PathCompletionKey::from_popup(kind, filter.trim(), &self.header.cwd)
        {
            self.composer.path_completion.request(key.clone());
            ComposerPopup::from_items(
                kind,
                self.composer.path_completion.cached_items(&key).to_vec(),
            )
        } else {
            ComposerPopup::new(kind, filter.trim(), &self.header.cwd, &self.header.catalog)
        };
        self.composer.composer_popup = (!popup.is_empty()).then_some(popup);
    }

    pub(super) fn poll_composer_path_completion(&mut self) -> bool {
        if !self.composer.path_completion.poll() {
            return false;
        }
        self.refresh_composer_popup();
        true
    }
}

fn mention_from_insert_text(value: &str) -> Option<(char, &str)> {
    let trimmed = value.trim();
    let mut chars = trimmed.chars();
    let sigil = chars.next()?;
    if !matches!(sigil, '@' | '$' | '#' | '%') {
        return None;
    }
    let name = chars.as_str();
    (!name.is_empty()).then_some((sigil, name))
}

fn mention_replace_end(draft: &str, token_end: usize) -> usize {
    if has_whitespace_after(draft, token_end) {
        return token_end + 1;
    }
    token_end
}

fn has_whitespace_after(draft: &str, char_index: usize) -> bool {
    draft
        .chars()
        .nth(char_index)
        .is_some_and(char::is_whitespace)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DraftTokenRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DraftToken {
    range: DraftTokenRange,
    text: String,
}

fn current_draft_token(draft: &str, cursor: usize) -> DraftToken {
    let range = draft_token_range(draft, cursor);
    DraftToken {
        range,
        text: draft_range_text(draft, range),
    }
}

fn draft_token_range(draft: &str, cursor: usize) -> DraftTokenRange {
    let chars = draft.chars().collect::<Vec<_>>();
    let bounded_cursor = cursor.min(chars.len());
    let mut start = bounded_cursor;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = bounded_cursor;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    DraftTokenRange { start, end }
}

fn draft_range_text(draft: &str, range: DraftTokenRange) -> String {
    draft
        .chars()
        .skip(range.start)
        .take(range.end.saturating_sub(range.start))
        .collect()
}
