//! 快捷键帮助 Dock Widget。

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::shortcut_help::ShortcutEntry,
    widgets::transcript_style::{role_style, strong_role_style},
};

const DOCK_TITLE: &str = "快捷键总览";
const DOCK_HINT: &str = "Esc 关闭 · 拖拽选择复制可见文本";

pub(crate) struct ShortcutHelpDockWidget<'a> {
    pub entries: &'a [ShortcutEntry],
}

impl ShortcutHelpDockWidget<'_> {
    fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(self.entries.len() + 1);
        lines.push(Line::from(vec![Span::styled(
            "常用操作",
            strong_role_style(TranscriptColorRole::Accent),
        )]));
        lines.extend(self.entries.iter().map(shortcut_line));
        lines
    }
}

impl Widget for ShortcutHelpDockWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let shell = Block::default()
            .title(DOCK_TITLE)
            .borders(Borders::ALL)
            .border_style(role_style(TranscriptColorRole::Muted));
        shell.render(area, buf);

        let inner = area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);

        Paragraph::new(self.lines())
            .wrap(Wrap { trim: true })
            .render(chunks[0], buf);
        Paragraph::new(DOCK_HINT)
            .style(role_style(TranscriptColorRole::Muted))
            .render(chunks[1], buf);
    }
}

fn shortcut_line(entry: &ShortcutEntry) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {:<14}", entry.key),
            strong_role_style(TranscriptColorRole::ToolCompleted),
        ),
        Span::styled(
            format!("{:<12}", entry.context),
            role_style(TranscriptColorRole::Accent),
        ),
        Span::styled(
            entry.action.to_owned(),
            role_style(TranscriptColorRole::Assistant),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::ShortcutHelpDockWidget;
    use crate::r#loop::agent_tui::shortcut_help::shortcut_entries;

    #[test]
    fn shortcut_help_dock_renders_drag_selection_copy_policy() {
        let area = Rect::new(0, 0, 100, 12);
        let mut buffer = Buffer::empty(area);

        ShortcutHelpDockWidget {
            entries: shortcut_entries(),
        }
        .render(area, &mut buffer);
        let text = buffer_text(&buffer);

        assert!(contains_chars_in_order(&text, "快捷键总览"));
        assert!(contains_chars_in_order(&text, "拖拽选择"));
        assert!(text.contains("Esc"));
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }

    fn contains_chars_in_order(text: &str, expected: &str) -> bool {
        let mut chars = text.chars();
        expected
            .chars()
            .all(|expected| chars.by_ref().any(|actual| actual == expected))
    }
}
