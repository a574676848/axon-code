//! Composer 历史搜索状态机。

#[derive(Debug, Clone, Default)]
pub(crate) struct HistorySearch {
    active: bool,
    query: String,
    query_lower: String,
    original_draft: String,
    matches: Vec<usize>,
    selected: usize,
}

impl HistorySearch {
    pub(crate) fn begin(&mut self, history: &[String], draft: &str) -> Option<String> {
        self.active = true;
        draft.trim().clone_into(&mut self.query);
        self.query_lower = self.query.to_lowercase();
        draft.clone_into(&mut self.original_draft);
        self.refresh(history);
        self.current(history)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn push(&mut self, history: &[String], value: char) -> Option<String> {
        self.query.push(value);
        self.query_lower = self.query.to_lowercase();
        self.refresh(history);
        self.current(history)
    }

    pub(crate) fn backspace(&mut self, history: &[String]) -> Option<String> {
        self.query.pop();
        self.query_lower = self.query.to_lowercase();
        self.refresh(history);
        self.current(history)
    }

    pub(crate) fn previous(&mut self, history: &[String]) -> Option<String> {
        if !self.matches.is_empty() {
            self.selected = (self.selected + 1).min(self.matches.len() - 1);
        }
        self.current(history)
    }

    pub(crate) fn next(&mut self, history: &[String]) -> Option<String> {
        if !self.matches.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
        self.current(history)
    }

    pub(crate) fn accept(&mut self, history: &[String]) -> String {
        let accepted = self
            .current(history)
            .unwrap_or_else(|| self.query.trim().to_owned());
        self.reset();
        accepted
    }

    pub(crate) fn cancel(&mut self) -> String {
        let draft = std::mem::take(&mut self.original_draft);
        self.reset();
        draft
    }

    fn refresh(&mut self, history: &[String]) {
        let query = self.query_lower.trim();
        self.matches = history
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, entry)| query.is_empty() || entry.to_lowercase().contains(query))
            .map(|(index, _)| index)
            .collect();
        self.selected = 0;
    }

    fn current(&self, history: &[String]) -> Option<String> {
        self.matches
            .get(self.selected)
            .and_then(|index| history.get(*index))
            .cloned()
    }

    fn reset(&mut self) {
        self.active = false;
        self.query.clear();
        self.query_lower.clear();
        self.original_draft.clear();
        self.matches.clear();
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::HistorySearch;

    #[test]
    fn history_search_filters_newest_first_and_restores_on_cancel() {
        let history = vec![
            "cargo check".to_owned(),
            "cargo test".to_owned(),
            "git status".to_owned(),
        ];
        let mut search = HistorySearch::default();

        assert_eq!(
            search.begin(&history, "cargo").as_deref(),
            Some("cargo test")
        );
        assert_eq!(search.previous(&history).as_deref(), Some("cargo check"));
        assert_eq!(search.cancel(), "cargo");
    }

    #[test]
    fn history_search_is_case_insensitive_and_repeated_ctrl_r_walks_older() {
        let history = vec![
            "Cargo check".to_owned(),
            "cargo test".to_owned(),
            "git status".to_owned(),
            "CARGO clippy".to_owned(),
        ];
        let mut search = HistorySearch::default();

        assert_eq!(
            search.begin(&history, "cargo").as_deref(),
            Some("CARGO clippy")
        );
        assert_eq!(search.previous(&history).as_deref(), Some("cargo test"));
        assert_eq!(search.previous(&history).as_deref(), Some("Cargo check"));
        assert_eq!(search.next(&history).as_deref(), Some("cargo test"));
    }
}
