//! 底部二级选择菜单。

use super::BottomViewEvent;
use crate::app_shell::composer_dock::DockKind;
use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionView {
    kind: DockKind,
    title: String,
    subtitle: String,
    items: Vec<SelectionItem>,
    selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionItem {
    pub(crate) label: String,
    pub(crate) description: String,
    pub(super) action: BottomViewAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BottomViewAction {
    SkillsList,
    SkillsToggle,
}

impl SelectionView {
    pub(crate) fn skills_menu() -> Self {
        Self {
            kind: DockKind::Skills,
            title: "Skills".to_owned(),
            subtitle: "选择操作".to_owned(),
            items: vec![
                SelectionItem {
                    label: "查看 skill 列表".to_owned(),
                    description: "插入 $ 并打开 skill 搜索引用".to_owned(),
                    action: BottomViewAction::SkillsList,
                },
                SelectionItem {
                    label: "启用/禁用 skill".to_owned(),
                    description: "搜索 skill，按 Space 切换状态".to_owned(),
                    action: BottomViewAction::SkillsToggle,
                },
            ],
            selected: 0,
        }
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) const fn kind(&self) -> DockKind {
        self.kind
    }

    pub(crate) fn subtitle(&self) -> &str {
        &self.subtitle
    }

    pub(crate) fn items(&self) -> &[SelectionItem] {
        &self.items
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn handle_key(&mut self, key: UiKeyEvent) -> BottomViewEvent {
        match key.code {
            UiKeyCode::Up => {
                self.select_previous();
                BottomViewEvent::None
            }
            UiKeyCode::Down => {
                self.select_next();
                BottomViewEvent::None
            }
            UiKeyCode::Enter | UiKeyCode::Tab => self
                .items
                .get(self.selected)
                .map_or(BottomViewEvent::None, |item| {
                    BottomViewEvent::Action(item.action.clone())
                }),
            UiKeyCode::Esc => BottomViewEvent::Close,
            _ => BottomViewEvent::None,
        }
    }

    pub(crate) fn height(&self) -> u16 {
        let item_rows = self.items.len().saturating_mul(2);
        u16::try_from(item_rows.saturating_add(2)).unwrap_or(u16::MAX)
    }

    fn select_previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
    }

    fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent, UiKeyModifiers};

    #[test]
    fn selection_menu_returns_selected_action() {
        let mut view = SelectionView::skills_menu();

        let event = view.handle_key(key(UiKeyCode::Enter));
        assert_eq!(event, BottomViewEvent::Action(BottomViewAction::SkillsList));

        view.handle_key(key(UiKeyCode::Down));
        let event = view.handle_key(key(UiKeyCode::Enter));
        assert_eq!(
            event,
            BottomViewEvent::Action(BottomViewAction::SkillsToggle)
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
}
