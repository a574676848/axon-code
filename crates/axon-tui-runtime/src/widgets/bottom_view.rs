//! 底部交互视图渲染。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::bottom_view::{BottomView, QuestionView, SelectionView, SkillsToggleView},
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
    use crate::r#loop::agent_tui::bottom_view::{BottomView, SelectionView, SkillsToggleView};

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
