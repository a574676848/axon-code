//! 会话目录弹窗视图。
//!
//! 候选只来自 `header.catalog.sessions`（宿主加载的真实会话目录），
//! 默认隐藏归档会话，`/sessions --all` 才把归档项一并列出。

use super::BottomViewEvent;
use crate::app_shell::composer_dock::DockKind;
use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

/// 标题行占用的行数。
const TITLE_ROWS: u16 = 1;
/// 底部提示行占用的行数；没有溢出时留空，保证高度与容量一一对应。
const HINT_ROWS: u16 = 1;
/// 每个候选占用的行数：标题与来源同一行，来源贴在行尾。
const ITEM_ROWS: u16 = 1;
/// 弹窗一次最多展示的候选数。
///
/// 面板实际可用高度由 ComposerDock 决定，可能小于这里申请的容量；
/// 渲染侧按真实高度算窗口并滚动，选中项因此始终可见。
const MAX_VISIBLE_ITEMS: usize = 6;
/// 归档会话在描述行里的前缀。
const ARCHIVED_PREFIX: &str = "已归档 · ";
/// 会话缺少 provider/model 时的占位描述。
const UNKNOWN_ORIGIN: &str = "未知来源";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionsView {
    items: Vec<SessionsViewItem>,
    selected: usize,
    show_archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionsViewItem {
    pub(crate) session_id: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) archived: bool,
}

impl SessionsView {
    /// 按目录构建弹窗；`show_archived` 为假时过滤掉归档会话。
    pub(crate) fn new(items: Vec<SessionsViewItem>, show_archived: bool) -> Self {
        let items = items
            .into_iter()
            .filter(|item| show_archived || !item.archived)
            .collect();
        Self {
            items,
            selected: 0,
            show_archived,
        }
    }

    pub(crate) fn items(&self) -> &[SessionsViewItem] {
        &self.items
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    /// 当前选中项的会话 ID；没有候选项时为空。
    pub(crate) fn selected_session_id(&self) -> Option<&str> {
        self.items
            .get(self.selected)
            .map(|item| item.session_id.as_str())
    }

    pub(super) fn handle_key(&mut self, key: UiKeyEvent) -> BottomViewEvent {
        match key.code {
            UiKeyCode::Up => self.select_previous(),
            UiKeyCode::Down => self.select_next(),
            UiKeyCode::Enter | UiKeyCode::Tab => {
                return self
                    .selected_session_id()
                    .map_or(BottomViewEvent::None, |id| {
                        BottomViewEvent::SessionSelected(id.to_owned())
                    });
            }
            UiKeyCode::Esc => return BottomViewEvent::Close,
            _ => {}
        }
        BottomViewEvent::None
    }

    pub(crate) fn height(&self) -> u16 {
        let visible_items = self.items.len().clamp(1, MAX_VISIBLE_ITEMS);
        // 高度与容量一一对应：标题行 + 提示行 + 每个候选自己的行数。
        let item_rows = u16::try_from(visible_items)
            .unwrap_or(u16::MAX)
            .saturating_mul(ITEM_ROWS);
        item_rows
            .saturating_add(TITLE_ROWS)
            .saturating_add(HINT_ROWS)
    }

    /// 面板实际可用高度能放下几条候选。
    ///
    /// 高度由 ComposerDock 决定，可能小于这里申请的容量；渲染侧必须按真实高度开窗，
    /// 否则选中项会跑到可视区外面。
    pub(crate) fn visible_capacity(&self, area_height: u16) -> usize {
        usize::from(area_height.saturating_sub(TITLE_ROWS) / ITEM_ROWS)
    }

    /// 让选中项落在窗口内的起始下标。
    ///
    /// 窗口只由「选中项 + 可用容量」推导，不额外存滚动状态：每帧都自洽，
    /// 也不会出现「滚出可视区后回不来」的偏移。
    pub(crate) fn window_start(&self, capacity: usize) -> usize {
        if capacity == 0 || self.items.len() <= capacity {
            return 0;
        }
        self.selected
            .saturating_sub(capacity - 1)
            .min(self.items.len() - capacity)
    }

    /// 测试用：把选中项移到指定下标（真实入口是方向键，视图内部不对外暴露定位）。
    #[cfg(test)]
    pub(crate) fn select_index_for_test(&mut self, index: usize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = index.min(self.items.len() - 1);
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

impl SessionsViewItem {
    /// 从会话目录项生成弹窗候选。
    pub(crate) fn from_catalog_item(item: &super::super::AgentTuiSessionCatalogItem) -> Self {
        let origin = session_origin(item);
        let description = if item.archived {
            format!("{ARCHIVED_PREFIX}{origin}")
        } else {
            origin
        };
        Self {
            session_id: item.session_id.clone(),
            label: item.label().to_owned(),
            description,
            archived: item.archived,
        }
    }
}

fn session_origin(item: &super::super::AgentTuiSessionCatalogItem) -> String {
    let provider = item.provider.trim();
    let model = item.model.trim();
    match (provider.is_empty(), model.is_empty()) {
        (false, false) => format!("{provider} · {model}"),
        (false, true) => provider.to_owned(),
        (true, false) => model.to_owned(),
        (true, true) => UNKNOWN_ORIGIN.to_owned(),
    }
}

impl SessionsView {
    /// 视图标识。
    pub(crate) const fn kind(&self) -> DockKind {
        DockKind::Sessions
    }

    /// 弹窗标题。
    pub(crate) const fn title(&self) -> &'static str {
        "会话"
    }

    /// 弹窗副标题；归档可见性必须显式可见，否则用户不知道列表被过滤过。
    pub(crate) const fn subtitle(&self) -> &'static str {
        if self.show_archived {
            "含归档 · ↑↓ 选择 · Enter 载入 · Esc 关闭"
        } else {
            "未归档 · ↑↓ 选择 · Enter 载入 · Esc 关闭（--all 含归档）"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::agent_tui::AgentTuiSessionCatalogItem;
    use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent, UiKeyModifiers};

    fn item(session_id: &str, archived: bool) -> AgentTuiSessionCatalogItem {
        AgentTuiSessionCatalogItem::new(
            session_id,
            Some(format!("{session_id} 标题")),
            "openai",
            "gpt-5.5",
            1,
        )
        .with_archived(archived)
    }

    fn view(show_archived: bool) -> SessionsView {
        SessionsView::new(
            [item("s-active", false), item("s-archived", true)]
                .iter()
                .map(SessionsViewItem::from_catalog_item)
                .collect(),
            show_archived,
        )
    }

    #[test]
    fn sessions_view_hides_archived_sessions_by_default() {
        let view = view(false);

        assert_eq!(view.items().len(), 1);
        assert_eq!(view.items()[0].session_id, "s-active");
        assert!(view.subtitle().starts_with("未归档"));
    }

    #[test]
    fn sessions_view_all_keeps_archived_sessions_visible() {
        let view = view(true);

        assert_eq!(view.items().len(), 2);
        assert_eq!(view.items()[1].session_id, "s-archived");
        assert!(view.items()[1].archived);
        assert!(view.items()[1].description.starts_with(ARCHIVED_PREFIX));
        assert!(view.subtitle().starts_with("含归档"));
    }

    #[test]
    fn sessions_view_moves_selection_wrapping_both_directions() {
        let mut view = view(true);

        assert_eq!(view.selected_session_id(), Some("s-active"));
        view.handle_key(key(UiKeyCode::Down));
        assert_eq!(view.selected_session_id(), Some("s-archived"));
        view.handle_key(key(UiKeyCode::Down));
        assert_eq!(view.selected_session_id(), Some("s-active"));
        view.handle_key(key(UiKeyCode::Up));
        assert_eq!(view.selected_session_id(), Some("s-archived"));
    }

    #[test]
    fn sessions_view_enter_reports_selected_session() {
        let mut view = view(true);

        view.handle_key(key(UiKeyCode::Down));
        let event = view.handle_key(key(UiKeyCode::Enter));

        assert_eq!(
            event,
            BottomViewEvent::SessionSelected("s-archived".to_owned())
        );
    }

    #[test]
    fn sessions_view_esc_closes_and_empty_catalog_stays_inert() {
        let mut empty = SessionsView::new(Vec::new(), false);

        assert_eq!(
            empty.handle_key(key(UiKeyCode::Enter)),
            BottomViewEvent::None
        );
        assert_eq!(
            empty.handle_key(key(UiKeyCode::Esc)),
            BottomViewEvent::Close
        );
        assert_eq!(empty.height(), 3);
    }

    #[test]
    fn session_popup_capacity_follows_the_real_panel_height() {
        let view = view(true);

        // 标题行之外每项占 1 行：高度 1 放不下任何候选，高度 2 只放得下一条。
        assert_eq!(view.visible_capacity(1), 0);
        assert_eq!(view.visible_capacity(2), 1);
        assert_eq!(view.visible_capacity(3), 2);
    }

    #[test]
    fn session_popup_window_keeps_the_selected_item_visible() {
        let items = (0..12)
            .map(|index| SessionsViewItem::from_catalog_item(&item(&format!("s-{index}"), false)))
            .collect::<Vec<_>>();
        let mut view = SessionsView::new(items, false);

        assert_eq!(view.window_start(3), 0, "顶部选中项窗口贴顶");
        for _ in 0..3 {
            view.handle_key(key(UiKeyCode::Down));
        }
        assert_eq!(view.window_start(3), 1, "选中第 4 项时窗口必须下移一行");
        for _ in 0..8 {
            view.handle_key(key(UiKeyCode::Down));
        }
        assert_eq!(view.selected(), 11);
        assert_eq!(view.window_start(3), 9, "末尾选中项窗口贴底");
        assert_eq!(view.window_start(0), 0, "容量为 0 时不滚动");
        assert_eq!(view.window_start(20), 0, "放得下时窗口贴顶");
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
