//! 底部交互视图渲染。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::bottom_view::{
        BottomView, QuestionView, SelectionView, SessionsView, SkillsToggleView,
    },
};

use super::{
    ShortcutHelpDockWidget, ToolOutputDockWidget,
    transcript_style::{role_style, strong_role_style},
};

pub(crate) struct BottomPaneView<'a> {
    pub(crate) view: &'a BottomView,
}

const PREFIX_SELECTED: &str = "› ";
const PREFIX_IDLE: &str = "  ";
const DESCRIPTION_PREFIX: &str = "    ";
const CHECKED: &str = "[x]";
const UNCHECKED: &str = "[ ]";
/// 提问面板的自定义答案行前缀。
const QUESTION_CUSTOM_PREFIX: &str = "  自定义答案: ";
/// 提问面板的快捷键提示；Esc 只收起面板，跳过是独立的 Tab 操作。
const QUESTION_HINT: &str = "  空格 选择 · 字符 自定义 · Enter 提交 · Tab 跳过 · Esc 收起";
/// 自定义答案编辑态的提示：此时上下键回到选项，Esc 只退出编辑、不收起面板。
///
/// 两个相位共用一句话会误导：编辑态里按 Esc 并不会收起面板，用户会以为按键失灵。
const QUESTION_EDIT_HINT: &str =
    "  字符 写自定义答案 · ↑↓ 回到选项 · Enter 提交 · Tab 跳过 · Esc 退出编辑";

impl Widget for BottomPaneView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        match self.view {
            BottomView::Question(view) => render_question_view(view, area, buf),
            BottomView::Selection(view) => render_selection_view(view, area, buf),
            BottomView::Sessions(view) => render_sessions_view(view, area, buf),
            BottomView::SkillsToggle(view) => render_skills_toggle_view(view, area, buf),
            BottomView::ToolOutputViewer(view) => ToolOutputDockWidget {
                title: view.title(),
                lines: view.lines(),
                scroll_offset: view.scroll(),
            }
            .render(area, buf),
            BottomView::ShortcutHelp(view) => {
                ShortcutHelpDockWidget {
                    entries: view.entries(),
                }
                .render(area, buf);
            }
        }
    }
}

fn render_selection_view(view: &SelectionView, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let title = Line::from(vec![
        Span::styled(
            view.title().to_owned(),
            strong_role_style(TranscriptColorRole::Loading),
        ),
        Span::raw(" "),
        Span::styled(
            view.subtitle().to_owned(),
            role_style(TranscriptColorRole::Muted),
        ),
    ]);
    buf.set_line(area.x, area.y, &title, area.width);

    let mut row = area.y.saturating_add(1);
    for (index, item) in view.items().iter().enumerate() {
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        let selected = index == view.selected();
        let prefix = if selected {
            PREFIX_SELECTED
        } else {
            PREFIX_IDLE
        };
        let style = if selected {
            strong_role_style(TranscriptColorRole::Accent)
        } else {
            role_style(TranscriptColorRole::Assistant)
        };
        buf.set_line(
            area.x,
            row,
            &Line::from(vec![
                Span::styled(prefix, role_style(TranscriptColorRole::Accent)),
                Span::styled(item.label.clone(), style),
            ]),
            area.width,
        );
        row = row.saturating_add(1);
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        buf.set_line(
            area.x,
            row,
            &Line::from(vec![
                Span::raw(DESCRIPTION_PREFIX),
                Span::styled(
                    item.description.clone(),
                    role_style(TranscriptColorRole::Muted),
                ),
            ]),
            area.width,
        );
        row = row.saturating_add(1);
    }
}

fn render_question_view(view: &QuestionView, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    let Some(current_q) = view.current_question() else {
        return;
    };

    let title_text = format!(
        "❓ 用户决策 [{}/{}] {}",
        view.current_index() + 1,
        view.total_questions(),
        current_q.header.as_deref().unwrap_or("")
    );
    let title = Line::from(vec![Span::styled(
        title_text,
        strong_role_style(TranscriptColorRole::Accent),
    )]);
    buf.set_line(area.x, area.y, &title, area.width);

    let mut row = area.y.saturating_add(1);
    let q_line = Line::from(vec![Span::styled(
        format!("  {}", current_q.question),
        role_style(TranscriptColorRole::Assistant),
    )]);
    buf.set_line(area.x, row, &q_line, area.width);
    row = row.saturating_add(1);

    for (index, opt) in current_q.options.iter().enumerate() {
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        let focused = index == view.focused_option();
        let is_checked = view.is_option_selected(&opt.label);
        let check_mark = if current_q.multi_select {
            if is_checked { "[x] " } else { "[ ] " }
        } else if is_checked {
            "(o) "
        } else {
            "( ) "
        };

        let prefix = if focused { " ▸ " } else { "   " };
        let style = if focused {
            strong_role_style(TranscriptColorRole::Accent)
        } else {
            role_style(TranscriptColorRole::Assistant)
        };

        let mut spans = vec![
            Span::styled(prefix, role_style(TranscriptColorRole::Accent)),
            Span::styled(check_mark, role_style(TranscriptColorRole::Muted)),
            Span::styled(opt.label.clone(), style),
        ];
        if let Some(desc) = &opt.description {
            spans.push(Span::styled(
                format!(" ({desc})"),
                role_style(TranscriptColorRole::Muted),
            ));
        }

        buf.set_line(area.x, row, &Line::from(spans), area.width);
        row = row.saturating_add(1);
    }

    render_question_custom_input(view, area, buf, row);
}

/// 自定义答案行与快捷键提示：编辑态必须有可见的回显，否则用户不知道键入了什么。
fn render_question_custom_input(view: &QuestionView, area: Rect, buf: &mut Buffer, start: u16) {
    let custom = view.custom_input();
    let editing = view.is_editing();
    if !editing && custom.is_empty() {
        return;
    }
    if start < area.y.saturating_add(area.height) {
        let caret = if editing { "▏" } else { "" };
        buf.set_line(
            area.x,
            start,
            &Line::from(vec![
                Span::styled(
                    QUESTION_CUSTOM_PREFIX.to_owned(),
                    role_style(TranscriptColorRole::Muted),
                ),
                Span::styled(
                    format!("{custom}{caret}"),
                    strong_role_style(TranscriptColorRole::Accent),
                ),
            ]),
            area.width,
        );
    }
    let hint_row = start.saturating_add(1);
    if hint_row < area.y.saturating_add(area.height) {
        buf.set_line(
            area.x,
            hint_row,
            &Line::from(Span::styled(
                if editing {
                    QUESTION_EDIT_HINT
                } else {
                    QUESTION_HINT
                }
                .to_owned(),
                role_style(TranscriptColorRole::Muted),
            )),
            area.width,
        );
    }
}

fn render_sessions_view(view: &SessionsView, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    buf.set_line(
        area.x,
        area.y,
        &Line::from(vec![
            Span::styled(
                view.title().to_owned(),
                strong_role_style(TranscriptColorRole::Loading),
            ),
            Span::raw(" "),
            Span::styled(
                view.subtitle().to_owned(),
                role_style(TranscriptColorRole::Muted),
            ),
        ]),
        area.width,
    );

    // 面板高度由输入坞决定，可能小于弹窗申请的容量：按真实高度开窗，选中项才不会被裁掉。
    let capacity = view.visible_capacity(area.height);
    if capacity == 0 {
        return;
    }
    let start = view.window_start(capacity);
    let label_width =
        usize::from(area.width).saturating_sub(UnicodeWidthStr::width(PREFIX_SELECTED));
    let description_width =
        usize::from(area.width).saturating_sub(UnicodeWidthStr::width(DESCRIPTION_PREFIX));
    let mut row = area.y.saturating_add(1);
    for (index, item) in view.items().iter().enumerate().skip(start).take(capacity) {
        let selected = index == view.selected();
        let prefix = if selected {
            PREFIX_SELECTED
        } else {
            PREFIX_IDLE
        };
        let style = if selected {
            strong_role_style(TranscriptColorRole::Accent)
        } else if item.archived {
            role_style(TranscriptColorRole::Muted)
        } else {
            role_style(TranscriptColorRole::Assistant)
        };
        buf.set_line(
            area.x,
            row,
            &Line::from(vec![
                Span::styled(prefix, role_style(TranscriptColorRole::Accent)),
                Span::styled(clip_with_ellipsis(&item.label, label_width), style),
            ]),
            area.width,
        );
        row = row.saturating_add(1);
        buf.set_line(
            area.x,
            row,
            &Line::from(vec![
                Span::raw(DESCRIPTION_PREFIX),
                Span::styled(
                    clip_with_ellipsis(&item.description, description_width),
                    role_style(TranscriptColorRole::Muted),
                ),
            ]),
            area.width,
        );
        row = row.saturating_add(1);
    }
}

/// 按显示宽度裁剪并补省略号。
///
/// `buf.set_line` 只会硬截断：超宽标题被切成半截词，用户看不出后面还有内容。
/// 中文与全角标点按 2 列计，因此必须按字符宽度累计，不能按字节或字符数切。
fn clip_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0;
    let mut clipped = String::new();
    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > budget {
            break;
        }
        used += width;
        clipped.push(character);
    }
    clipped.push('…');
    clipped
}

fn render_skills_toggle_view(view: &SkillsToggleView, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    buf.set_line(
        area.x,
        area.y,
        &Line::from(vec![
            Span::styled(
                "启用/禁用 Skills",
                strong_role_style(TranscriptColorRole::Loading),
            ),
            Span::raw(" "),
            Span::styled(
                "Space 切换 · Esc 关闭",
                role_style(TranscriptColorRole::Muted),
            ),
        ]),
        area.width,
    );
    if area.height == 1 {
        return;
    }
    buf.set_line(
        area.x,
        area.y.saturating_add(1),
        &Line::from(vec![
            Span::styled("> ", role_style(TranscriptColorRole::Accent)),
            Span::styled(
                view.query().to_owned(),
                role_style(TranscriptColorRole::Assistant),
            ),
            Span::styled("_", role_style(TranscriptColorRole::Muted)),
        ]),
        area.width,
    );

    let mut row = area.y.saturating_add(2);
    for (visible_index, item_index) in view.filtered_indices().iter().copied().enumerate() {
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        let Some(item) = view.items().get(item_index) else {
            continue;
        };
        let selected = visible_index == view.selected();
        let prefix = if selected {
            PREFIX_SELECTED
        } else {
            PREFIX_IDLE
        };
        let checkbox = if item.enabled { CHECKED } else { UNCHECKED };
        let name_style = if selected {
            strong_role_style(TranscriptColorRole::Accent)
        } else if item.enabled {
            role_style(TranscriptColorRole::ToolCompleted)
        } else {
            role_style(TranscriptColorRole::Muted)
        };
        buf.set_line(
            area.x,
            row,
            &Line::from(vec![
                Span::styled(prefix, role_style(TranscriptColorRole::Accent)),
                Span::styled(checkbox, role_style(TranscriptColorRole::Muted)),
                Span::raw(" "),
                Span::styled(item.name.clone(), name_style),
            ]),
            area.width,
        );
        row = row.saturating_add(1);
        if row >= area.y.saturating_add(area.height) {
            break;
        }
        buf.set_line(
            area.x,
            row,
            &Line::from(vec![
                Span::raw(DESCRIPTION_PREFIX),
                Span::styled(
                    item.description.clone(),
                    role_style(TranscriptColorRole::Muted),
                ),
            ]),
            area.width,
        );
        row = row.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::*;
    use crate::r#loop::agent_tui::bottom_view::{
        BottomView, SelectionView, SessionsView, SessionsViewItem, SkillsToggleView,
    };

    #[test]
    fn selection_view_renders_menu_actions() {
        let view = BottomView::Selection(SelectionView::skills_menu());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 8));

        BottomPaneView { view: &view }.render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(text.contains("Skills"));
        assert!(text.contains("查看skill列表"));
        assert!(text.contains("启用/禁用skill"));
        assert!(!buffer_text(&buffer).contains('─'));
    }

    #[test]
    fn sessions_view_renders_selected_and_archived_items() {
        let sessions = [
            crate::r#loop::agent_tui::AgentTuiSessionCatalogItem::new(
                "session-active",
                Some("普通会话".to_owned()),
                "openai",
                "gpt-5.5",
                1,
            ),
            crate::r#loop::agent_tui::AgentTuiSessionCatalogItem::new(
                "session-archived",
                Some("归档会话".to_owned()),
                "openai",
                "gpt-5.5",
                2,
            )
            .with_archived(true),
        ];
        let view = BottomView::Sessions(SessionsView::new(
            sessions
                .iter()
                .map(SessionsViewItem::from_catalog_item)
                .collect(),
            true,
        ));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 10));

        BottomPaneView { view: &view }.render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(text.contains("会话"));
        assert!(text.contains("普通会话"));
        assert!(text.contains("归档会话"));
        assert!(text.contains("已归档"));
    }

    #[test]
    fn sessions_view_scrolls_to_keep_the_late_selection_visible() {
        let sessions = (0..12)
            .map(|index| {
                crate::r#loop::agent_tui::AgentTuiSessionCatalogItem::new(
                    format!("session-{index}"),
                    Some(format!("会话 {index}")),
                    "openai",
                    "gpt-5.5",
                    index,
                )
            })
            .collect::<Vec<_>>();
        let mut view = SessionsView::new(
            sessions
                .iter()
                .map(SessionsViewItem::from_catalog_item)
                .collect(),
            false,
        );
        view.select_index_for_test(11);
        let view = BottomView::Sessions(view);
        // 高度 7：标题 1 行 + 3 条候选（每条 2 行），选中第 12 条必须滚进可视区。
        let mut buffer = Buffer::empty(Rect::new(0, 0, 40, 7));

        BottomPaneView { view: &view }.render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(text.contains("会话11"), "选中项必须落在可视区内：{text}");
        assert!(!text.contains("会话0"), "窗口外的早期项必须滚出：{text}");
    }

    #[test]
    fn sessions_view_clips_long_titles_with_ellipsis() {
        let sessions = [crate::r#loop::agent_tui::AgentTuiSessionCatalogItem::new(
            "session-long",
            Some("仅复盘随附的 turn outcome 与 evidence 记录".to_owned()),
            "openai",
            "gpt-5.5",
            1,
        )];
        let view = BottomView::Sessions(SessionsView::new(
            sessions
                .iter()
                .map(SessionsViewItem::from_catalog_item)
                .collect(),
            false,
        ));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 24, 4));

        BottomPaneView { view: &view }.render(buffer.area, &mut buffer);

        let text = buffer_text(&buffer);
        assert!(text.contains('…'), "超宽标题必须补省略号：{text}");
        assert!(
            !text.contains("evidence 记录"),
            "超出宽度的部分必须裁掉：{text}"
        );
    }

    #[test]
    fn skills_toggle_view_renders_checkbox_items() {
        let view = BottomView::SkillsToggle(SkillsToggleView::new(
            &["review".to_owned()],
            &std::collections::BTreeSet::new(),
        ));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 8));

        BottomPaneView { view: &view }.render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(text.contains("启用/禁用Skills"));
        assert!(text.contains("[x]"));
        assert!(text.contains("review"));
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }

    fn normalized_text(text: &str) -> String {
        text.split_whitespace().collect::<String>()
    }
}
