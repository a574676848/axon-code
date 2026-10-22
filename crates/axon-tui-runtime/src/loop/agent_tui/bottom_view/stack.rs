//! 底部交互视图栈。

use std::time::Instant;

use crate::app_shell::composer_dock::{DockCloseResult, DockKind, DockLifecycle, DockState};

use super::BottomView;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BottomViewEntry {
    view: BottomView,
    lifecycle: DockLifecycle,
}

impl BottomViewEntry {
    fn opening(view: BottomView) -> Self {
        let desired_height = view.height();
        Self {
            view,
            lifecycle: DockLifecycle::opening(desired_height),
        }
    }

    pub(crate) fn view(&self) -> &BottomView {
        &self.view
    }

    pub(crate) fn view_mut(&mut self) -> &mut BottomView {
        &mut self.view
    }

    pub(crate) fn lifecycle(&self) -> DockLifecycle {
        self.lifecycle
    }

    fn request_close(&mut self, now: Instant) -> DockCloseResult {
        self.lifecycle.request_close_at(now)
    }

    fn advance_lifecycle(&mut self, now: Instant, duration: std::time::Duration) -> bool {
        self.lifecycle.advance(now, duration)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BottomViewStack {
    views: Vec<BottomViewEntry>,
}

impl BottomViewStack {
    pub(crate) fn push(&mut self, view: BottomView) {
        self.views.push(BottomViewEntry::opening(view));
    }

    pub(crate) fn clear(&mut self) {
        self.views.clear();
    }

    pub(crate) fn open_exclusive(&mut self, view: BottomView) {
        self.clear();
        self.push(view);
    }

    pub(crate) fn top(&self) -> Option<&BottomViewEntry> {
        self.views.last()
    }

    pub(crate) fn top_mut(&mut self) -> Option<&mut BottomViewEntry> {
        self.views.last_mut()
    }

    pub(crate) fn top_view(&self) -> Option<&BottomView> {
        self.top().map(BottomViewEntry::view)
    }

    pub(crate) fn top_kind(&self) -> Option<DockKind> {
        self.active_top().map(|entry| entry.view().kind())
    }

    pub(crate) fn contains_kind(&self, kind: DockKind) -> bool {
        self.views
            .iter()
            .any(|entry| entry.view().kind() == kind && entry.is_interactive())
    }

    pub(crate) fn top_lifecycle(&self) -> Option<DockLifecycle> {
        self.top().map(BottomViewEntry::lifecycle)
    }

    pub(crate) fn request_close_top(&mut self) -> DockCloseResult {
        let now = Instant::now();
        self.top_mut()
            .map_or(DockCloseResult::NoDock, |entry| entry.request_close(now))
    }

    pub(crate) fn advance_lifecycle(
        &mut self,
        now: Instant,
        duration: std::time::Duration,
    ) -> bool {
        let changed = self
            .top_mut()
            .is_some_and(|entry| entry.advance_lifecycle(now, duration));
        let removed_hidden = self
            .top_lifecycle()
            .is_some_and(|lifecycle| {
                matches!(
                    lifecycle.state(),
                    crate::app_shell::composer_dock::DockState::Hidden
                )
            })
            .then(|| self.views.pop())
            .flatten()
            .is_some();
        changed || removed_hidden
    }

    pub(crate) fn close_kind(&mut self, kind: DockKind) -> DockCloseResult {
        let Some(index) = self
            .views
            .iter()
            .rposition(|entry| entry.view().kind() == kind)
        else {
            return DockCloseResult::NoDock;
        };
        self.views[index].request_close(Instant::now())
    }

    pub(crate) fn is_open(&self) -> bool {
        self.active_top().is_some()
    }

    fn active_top(&self) -> Option<&BottomViewEntry> {
        self.top().filter(|entry| entry.is_interactive())
    }
}

impl BottomViewEntry {
    fn is_interactive(&self) -> bool {
        !matches!(
            self.lifecycle.state(),
            DockState::Closing | DockState::Hidden
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::app_shell::composer_dock::{DockCloseResult, DockState};
    use crate::r#loop::agent_tui::bottom_view::{SelectionView, stack::BottomViewStack};

    use super::BottomView;

    #[test]
    fn bottom_view_stack_opens_view_as_animating_dock() {
        let mut stack = BottomViewStack::default();

        stack.push(BottomView::Selection(SelectionView::skills_menu()));

        let lifecycle = stack.top_lifecycle().expect("bottom view should open");
        assert_eq!(lifecycle.state(), DockState::Opening);
        assert_eq!(lifecycle.visible_height(), 0);
    }

    #[test]
    fn bottom_view_stack_repeated_close_request_is_idempotent() {
        let mut stack = BottomViewStack::default();
        stack.push(BottomView::Selection(SelectionView::skills_menu()));

        assert_eq!(stack.request_close_top(), DockCloseResult::StartedClosing);
        let closing = stack.top_lifecycle();
        assert_eq!(stack.request_close_top(), DockCloseResult::AlreadyClosing);

        assert_eq!(stack.top_lifecycle(), closing);
        assert!(!stack.is_open());
        assert!(stack.top_lifecycle().is_some());
    }

    #[test]
    fn closing_bottom_view_is_removed_after_lifecycle_reaches_hidden() {
        let mut stack = BottomViewStack::default();
        stack.push(BottomView::Selection(SelectionView::skills_menu()));
        let now = std::time::Instant::now();
        assert!(stack.advance_lifecycle(
            now + std::time::Duration::from_millis(40),
            std::time::Duration::from_millis(40)
        ));
        stack.request_close_top();

        assert!(!stack.is_open());
        assert!(stack.top_lifecycle().is_some());
        assert!(stack.advance_lifecycle(
            now + std::time::Duration::from_millis(80),
            std::time::Duration::from_millis(40)
        ));
        assert!(!stack.is_open());
    }
}
