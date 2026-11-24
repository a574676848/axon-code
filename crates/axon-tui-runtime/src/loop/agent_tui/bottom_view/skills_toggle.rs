//! Skill 启用状态管理视图。

use std::collections::BTreeSet;

use super::BottomViewEvent;
use crate::r#loop::agent_tui::AgentTuiSkillStateChange;
use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillsToggleView {
    items: Vec<SkillsToggleItem>,
    filtered_indices: Vec<usize>,
    selected: usize,
    query: String,
    initial_enabled_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillsToggleItem {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) initial_enabled: bool,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillsToggleSummary {
    pub(crate) enabled_count: usize,
    pub(crate) disabled_count: usize,
    pub(crate) enabled_delta: usize,
    pub(crate) disabled_delta: usize,
    pub(crate) disabled_skills: BTreeSet<String>,
    pub(crate) changes: Vec<AgentTuiSkillStateChange>,
}

const DEFAULT_SKILL_DESCRIPTION: &str = "Skill";
const MAX_VISIBLE_ITEMS: usize = 8;

impl SkillsToggleView {
    pub(crate) fn new(skill_names: &[String], disabled_skills: &BTreeSet<String>) -> Self {
        let items = skill_names
            .iter()
            .map(|name| SkillsToggleItem {
                name: name.clone(),
                description: DEFAULT_SKILL_DESCRIPTION.to_owned(),
                initial_enabled: !disabled_skills.contains(name),
                enabled: !disabled_skills.contains(name),
            })
            .collect::<Vec<_>>();
        let filtered_indices = (0..items.len()).collect::<Vec<_>>();
        let initial_enabled_count = items.iter().filter(|item| item.enabled).count();
        Self {
            initial_enabled_count,
            items,
            filtered_indices,
            selected: 0,
            query: String::new(),
        }
    }

    pub(crate) fn items(&self) -> &[SkillsToggleItem] {
        &self.items
    }

    pub(crate) fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn handle_key(&mut self, key: UiKeyEvent) -> BottomViewEvent {
        match key.code {
            UiKeyCode::Up => self.select_previous(),
            UiKeyCode::Down => self.select_next(),
            UiKeyCode::Char(' ') if key.modifiers.is_plain() => self.toggle_selected(),
            UiKeyCode::Backspace => {
                self.query.pop();
                self.apply_filter();
            }
            UiKeyCode::Esc | UiKeyCode::Enter => {
                return BottomViewEvent::SkillsClosed(self.summary());
            }
            UiKeyCode::Char(value) if key.modifiers.is_plain_or_shift() => {
                self.query.push(value);
                self.apply_filter();
            }
            _ => {}
        }
        BottomViewEvent::None
    }

    pub(super) fn summary(&self) -> SkillsToggleSummary {
        let enabled_count = self.items.iter().filter(|item| item.enabled).count();
        let disabled_count = self.items.len().saturating_sub(enabled_count);
        let disabled_skills = self
            .items
            .iter()
            .filter(|item| !item.enabled)
            .map(|item| item.name.clone())
            .collect();
        let changes = self
            .items
            .iter()
            .filter(|item| item.enabled != item.initial_enabled)
            .map(|item| AgentTuiSkillStateChange {
                name: item.name.clone(),
                enabled: item.enabled,
            })
            .collect();
        SkillsToggleSummary {
            enabled_count,
            disabled_count,
            enabled_delta: enabled_count.saturating_sub(self.initial_enabled_count),
            disabled_delta: self.initial_enabled_count.saturating_sub(enabled_count),
            disabled_skills,
            changes,
        }
    }

    pub(crate) fn height(&self) -> u16 {
        let visible_items = self.filtered_indices.len().clamp(1, MAX_VISIBLE_ITEMS);
        u16::try_from(visible_items.saturating_mul(2).saturating_add(2)).unwrap_or(u16::MAX)
    }

    fn select_previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or(self.filtered_indices.len() - 1);
    }

    fn select_next(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.selected = (self.selected + 1) % self.filtered_indices.len();
        }
    }

    fn toggle_selected(&mut self) {
        let Some(item_index) = self.filtered_indices.get(self.selected).copied() else {
            return;
        };
        if let Some(item) = self.items.get_mut(item_index) {
            item.enabled = !item.enabled;
        }
    }

    fn apply_filter(&mut self) {
        let query = self.query.trim().to_ascii_lowercase();
        self.filtered_indices = if query.is_empty() {
            (0..self.items.len()).collect()
        } else {
            let mut matches = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    skill_matches(&query, item).then_some((index, skill_score(&query, item)))
                })
                .collect::<Vec<_>>();
            matches.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
            matches.into_iter().map(|(index, _score)| index).collect()
        };
        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
    }
}

fn skill_matches(query: &str, item: &SkillsToggleItem) -> bool {
    item.name.to_ascii_lowercase().contains(query)
        || item.description.to_ascii_lowercase().contains(query)
        || fuzzy_matches(query, &item.name.to_ascii_lowercase())
}

fn skill_score(query: &str, item: &SkillsToggleItem) -> i32 {
    let name = item.name.to_ascii_lowercase();
    if name == query {
        return 100;
    }
    if name.starts_with(query) {
        return 80;
    }
    if name.contains(query) {
        return 60;
    }
    40
}

fn fuzzy_matches(query: &str, value: &str) -> bool {
    let mut query_chars = query.chars();
    let Some(mut expected) = query_chars.next() else {
        return true;
    };
    for value_char in value.chars() {
        if value_char == expected {
            match query_chars.next() {
                Some(next) => expected = next,
                None => return true,
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent, UiKeyModifiers};

    #[test]
    fn skills_toggle_filters_and_toggles_selected_item() {
        let mut disabled_skills = BTreeSet::new();
        disabled_skills.insert("frontend-design".to_owned());
        let mut view = SkillsToggleView::new(
            &[
                "frontend-design".to_owned(),
                "gitnexus-debugging".to_owned(),
            ],
            &disabled_skills,
        );

        view.handle_key(key(UiKeyCode::Char('f')));
        view.handle_key(key(UiKeyCode::Char('d')));
        assert_eq!(view.filtered_indices().len(), 1);
        assert_eq!(
            view.items()[view.filtered_indices()[0]].name,
            "frontend-design"
        );

        view.handle_key(key(UiKeyCode::Char(' ')));
        let summary = view.summary();

        assert_eq!(summary.enabled_count, 2);
        assert_eq!(summary.disabled_count, 0);
        assert_eq!(summary.enabled_delta, 1);
        assert!(summary.disabled_skills.is_empty());
        assert_eq!(
            summary.changes,
            vec![AgentTuiSkillStateChange {
                name: "frontend-design".to_owned(),
                enabled: true,
            }]
        );
    }

    const fn key(code: UiKeyCode) -> UiKeyEvent {
        UiKeyEvent {
            code,
            modifiers: UiKeyModifiers {
                control: false,
                alt: false,
                shift: false,
            },
        }
    }

    #[test]
    fn skills_toggle_summary_keeps_disabled_state_but_records_only_changed_items() {
        let mut disabled_skills = BTreeSet::new();
        disabled_skills.insert("gitnexus-debugging".to_owned());
        let view = SkillsToggleView::new(
            &[
                "frontend-design".to_owned(),
                "gitnexus-debugging".to_owned(),
            ],
            &disabled_skills,
        );

        let summary = view.summary();

        assert_eq!(summary.enabled_count, 1);
        assert_eq!(summary.disabled_count, 1);
        assert_eq!(
            summary.disabled_skills,
            BTreeSet::from(["gitnexus-debugging".to_owned()])
        );
        assert!(summary.changes.is_empty());
    }
}
