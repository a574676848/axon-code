//! 工具完整输出 Dock Widget。

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap},
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    widgets::transcript_style::{role_style, strong_role_style},
};

const OUTPUT_DOCK_HINT: &str = "Esc 关闭 · E 跳到错误 · 拖拽选择复制输出 · 截断时查看 artifact · PgUp/PgDn 滚动 · Ctrl+Home/End 跳转";
const STDERR_PREFIX: &str = "stderr";
const STDOUT_PREFIX: &str = "stdout";
const OUTPUT_PREFIX: &str = "output";

/// 工具完整输出 Dock。
pub(crate) struct ToolOutputDockWidget<'a> {
    /// 标题。
    pub title: &'a str,
    /// 输出行。
    pub lines: &'a [String],
    /// 滚动偏移。
    pub scroll_offset: u16,
}

impl Widget for ToolOutputDockWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let shell = Block::default()
            .title(Line::from(vec![Span::styled(
                self.title,
                strong_role_style(TranscriptColorRole::Accent),
            )]))
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

        let output = self
            .lines
            .iter()
            .map(|line| Line::from(Span::styled(line.clone(), output_line_style(line))))
            .collect::<Vec<_>>();
        Paragraph::new(output)
            .style(role_style(TranscriptColorRole::Assistant))
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset, 0))
            .render(chunks[0], buf);

        Paragraph::new(OUTPUT_DOCK_HINT)
            .style(role_style(TranscriptColorRole::Muted))
            .wrap(Wrap { trim: true })
            .render(chunks[1], buf);
    }
}

fn output_line_role(line: &str) -> TranscriptColorRole {
    let trimmed = line.trim_start();
    if trimmed.starts_with(STDERR_PREFIX) || trimmed.to_ascii_lowercase().contains("error") {
        return TranscriptColorRole::Error;
    }
    if trimmed.starts_with(STDOUT_PREFIX) {
        return TranscriptColorRole::ToolCompleted;
    }
    if trimmed.starts_with(OUTPUT_PREFIX) {
        return TranscriptColorRole::Accent;
    }
    TranscriptColorRole::Assistant
}

fn output_line_style(line: &str) -> Style {
    role_style(output_line_role(line))
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use crate::{
        app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
        widgets::tool_output_dock::output_line_role,
    };

    use super::ToolOutputDockWidget;

    #[test]
    fn tool_output_dock_renders_title_output_and_hint() {
        let lines = vec!["cargo check".to_owned(), "Finished dev profile".to_owned()];
        let area = Rect::new(0, 0, 100, 8);
        let mut buffer = Buffer::empty(area);

        ToolOutputDockWidget {
            title: "工具输出 · builtin.shell_command",
            lines: &lines,
            scroll_offset: 0,
        }
        .render(area, &mut buffer);
        let text = buffer_text(&buffer);

        assert!(text.contains("Finished dev profile"));
        assert!(super::OUTPUT_DOCK_HINT.contains("E 跳到错误"));
        assert!(super::OUTPUT_DOCK_HINT.contains("拖拽选择复制输出"));
        assert!(super::OUTPUT_DOCK_HINT.contains("截断时查看 artifact"));
    }

    #[test]
    fn tool_output_dock_highlights_error_streams() {
        assert_eq!(
            output_line_role("stderr #001 │ failed"),
            TranscriptColorRole::Error
        );
        assert_eq!(
            output_line_role("stdout #002 │ ok"),
            TranscriptColorRole::ToolCompleted
        );
        assert_eq!(
            output_line_role("output #003 │ info"),
            TranscriptColorRole::Accent
        );
        assert_eq!(
            super::output_line_style("stderr #001 │ failed").fg,
            Some(transcript_palette().color(TranscriptColorRole::Error))
        );
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }
}
