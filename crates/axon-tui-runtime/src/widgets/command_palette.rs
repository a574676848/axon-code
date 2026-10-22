//! 命令建议面板 Widget，展示 slash 命令、说明和参数提示。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::app_shell::style_tokens::TranscriptColorRole;
use crate::r#loop::agent_tui::commands::AgentTuiCommandSuggestion;
use crate::surface::display_width::DisplayWidthText;
use crate::widgets::{transcript_cells::display_width, transcript_style::role_style};

const COMMAND_PANEL_MAX_HEIGHT: u16 = 9;
const COMMAND_PANEL_FOOTER_HEIGHT: u16 = 1;
const COMMAND_HORIZONTAL_PADDING: u16 = 2;
const COMMAND_COLUMN_MIN_WIDTH: usize = 8;
const COMMAND_COLUMN_PADDING: usize = 2;
const DESCRIPTION_COLUMN_MIN_WIDTH: usize = 18;
const DESCRIPTION_GAP: &str = "  ";

/// 命令建议面板 Widget。
pub(crate) struct CommandPalette<'a> {
    /// 面板标题。
    pub title: &'a str,
    /// 命令建议列表。
    pub commands: &'a [AgentTuiCommandSuggestion],
    /// 当前选中的命令索引。
    pub selected: usize,
}

impl CommandPalette<'_> {
    pub(crate) fn required_height(command_count: usize) -> u16 {
        if command_count == 0 {
            return 0;
        }
        let max_items = COMMAND_PANEL_MAX_HEIGHT.saturating_sub(COMMAND_PANEL_FOOTER_HEIGHT);
        let visible = command_count.min(usize::from(max_items));
        u16::try_from(visible).unwrap_or(u16::MAX) + COMMAND_PANEL_FOOTER_HEIGHT
    }

    fn build_command_lines(&self, width: u16) -> Vec<Line<'static>> {
        let command_width = command_column_width(self.commands, width);
        self.visible_commands()
            .map(|(index, item)| command_line(item, index == self.selected, command_width))
            .collect()
    }

    fn visible_commands(&self) -> impl Iterator<Item = (usize, &AgentTuiCommandSuggestion)> + '_ {
        let start = selected_window_start(self.selected, self.commands.len());
        self.commands
            .iter()
            .enumerate()
            .skip(start)
            .take(usize::from(
                COMMAND_PANEL_MAX_HEIGHT.saturating_sub(COMMAND_PANEL_FOOTER_HEIGHT),
            ))
    }
}

impl Widget for CommandPalette<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 || self.commands.is_empty() {
            return;
        }
        let list_height = area.height.saturating_sub(COMMAND_PANEL_FOOTER_HEIGHT);
        if list_height == 0 {
            return;
        }
        let list_area = Rect {
            x: area.x.saturating_add(COMMAND_HORIZONTAL_PADDING),
            y: area.y,
            width: area.width.saturating_sub(COMMAND_HORIZONTAL_PADDING),
            height: list_height,
        };
        for (offset, line) in self
            .build_command_lines(list_area.width)
            .into_iter()
            .enumerate()
        {
            if offset >= usize::from(list_height) {
                break;
            }
            if let Ok(y_offset) = u16::try_from(offset) {
                line.render(
                    Rect {
                        x: list_area.x,
                        y: list_area.y.saturating_add(y_offset),
                        width: list_area.width,
                        height: 1,
                    },
                    buf,
                );
            }
        }
        let footer_y = area.y.saturating_add(list_height);
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                self.title.to_owned(),
                role_style(TranscriptColorRole::Muted),
            ),
            Span::styled(
                " · Enter 执行 · Esc 关闭",
                role_style(TranscriptColorRole::Muted),
            ),
        ])
        .render(
            Rect {
                x: area.x,
                y: footer_y,
                width: area.width,
                height: COMMAND_PANEL_FOOTER_HEIGHT,
            },
            buf,
        );
    }
}

fn selected_window_start(selected: usize, total: usize) -> usize {
    let max_items =
        usize::from(COMMAND_PANEL_MAX_HEIGHT.saturating_sub(COMMAND_PANEL_FOOTER_HEIGHT));
    if total <= max_items {
        return 0;
    }
    selected.saturating_sub(max_items.saturating_sub(1))
}

fn command_line(
    item: &AgentTuiCommandSuggestion,
    selected: bool,
    command_width: usize,
) -> Line<'static> {
    let base_style = if selected {
        selected_style().add_modifier(Modifier::BOLD)
    } else {
        unselected_style(TranscriptColorRole::Assistant)
    };
    let muted_style = if selected {
        selected_style()
    } else {
        unselected_style(TranscriptColorRole::Muted)
    };
    let command_style = if selected {
        base_style
    } else {
        unselected_style(TranscriptColorRole::Assistant).add_modifier(Modifier::BOLD)
    };
    let label = truncate_display_text(&command_display_label(item), command_width);
    let hint = visible_argument_hint(item);

    Line::from(vec![
        Span::styled(" ", command_style),
        Span::styled(label.clone(), command_style),
        Span::styled(
            " ".repeat(command_width.saturating_sub(display_width(&label))),
            command_style,
        ),
        Span::styled(hint, muted_style),
        Span::styled(DESCRIPTION_GAP, muted_style),
        Span::styled(item.description.clone(), base_style),
    ])
}

fn selected_style() -> Style {
    role_style(TranscriptColorRole::Accent).add_modifier(Modifier::REVERSED)
}

fn unselected_style(role: TranscriptColorRole) -> Style {
    role_style(role).remove_modifier(Modifier::REVERSED)
}

fn command_display_label(item: &AgentTuiCommandSuggestion) -> String {
    item.command.clone()
}

fn truncate_display_text(value: &str, max_width: usize) -> String {
    DisplayWidthText::ellipsize(value, max_width)
}

fn visible_argument_hint(item: &AgentTuiCommandSuggestion) -> String {
    if item.argument_hint.is_empty() || item.command.ends_with(&item.argument_hint) {
        String::new()
    } else {
        format!(" {}", item.argument_hint)
    }
}

fn command_column_width(commands: &[AgentTuiCommandSuggestion], area_width: u16) -> usize {
    let intrinsic = commands
        .iter()
        .map(|item| display_width(&command_display_label(item)))
        .max()
        .unwrap_or(COMMAND_COLUMN_MIN_WIDTH)
        .max(COMMAND_COLUMN_MIN_WIDTH)
        .saturating_add(COMMAND_COLUMN_PADDING);
    let max_width = usize::from(area_width)
        .saturating_sub(DESCRIPTION_COLUMN_MIN_WIDTH)
        .max(COMMAND_COLUMN_MIN_WIDTH);
    intrinsic.min(max_width)
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, widgets::Widget};

    use super::{CommandPalette, command_column_width, command_line, truncate_display_text};
    use crate::{
        app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
        r#loop::agent_tui::commands::AgentTuiCommandSuggestion,
    };

    const COMMAND_PANEL_DEFAULT_TITLE: &str = "Commands";

    #[test]
    fn command_palette_renders_selected_command_and_description() {
        let commands = [
            AgentTuiCommandSuggestion {
                command: "/model".to_owned(),
                description: "切换模型".to_owned(),
                argument_hint: "<model>".to_owned(),
            },
            AgentTuiCommandSuggestion {
                command: "/think".to_owned(),
                description: "设置推理强度".to_owned(),
                argument_hint: String::new(),
            },
        ];
        let area = Rect::new(0, 0, 80, 9);
        let mut buffer = Buffer::empty(area);

        CommandPalette {
            commands: &commands,
            title: COMMAND_PANEL_DEFAULT_TITLE,
            selected: 0,
        }
        .render(area, &mut buffer);
        let text = buffer_text(&buffer);

        assert!(text.contains("/model"));
        assert!(text.contains("<model>"));
        assert!(text.contains("/think"));
        assert!(text.contains("Commands"));
        assert!(text.contains("Enter"));
        assert!(text.contains("Esc"));
        assert!(!text.contains("┌"));
        assert!(!text.contains("┐"));
        assert!(!text.contains("└"));
        assert!(!text.contains("┘"));
    }

    #[test]
    fn command_palette_keeps_long_model_rows_column_aligned() {
        let commands = [
            AgentTuiCommandSuggestion {
                command: "/model claude-sonnet-4-6".to_owned(),
                description: "切换模型：claude-sonnet-4-6".to_owned(),
                argument_hint: "claude-sonnet-4-6".to_owned(),
            },
            AgentTuiCommandSuggestion {
                command: "/model gpt-5.5".to_owned(),
                description: "切换模型：gpt-5.5".to_owned(),
                argument_hint: "gpt-5.5".to_owned(),
            },
        ];
        let command_width = command_column_width(&commands, 80);
        let first = command_line(&commands[0], true, command_width);
        let second = command_line(&commands[1], false, command_width);

        assert_eq!(first.spans[1].content.as_ref(), "/model claude-sonnet-4-6");
        assert_eq!(second.spans[1].content.as_ref(), "/model gpt-5.5");
        assert_eq!(
            first.spans[5].content.as_ref(),
            "切换模型：claude-sonnet-4-6"
        );
        assert_eq!(second.spans[5].content.as_ref(), "切换模型：gpt-5.5");
        assert_eq!(
            truncate_display_text("/model claude-sonnet-4-6", 12),
            "/model clau…"
        );

        let first_left_width =
            crate::widgets::transcript_cells::display_width(first.spans[1].content.as_ref())
                + first.spans[2].content.len();
        let second_left_width =
            crate::widgets::transcript_cells::display_width(second.spans[1].content.as_ref())
                + second.spans[2].content.len();
        assert_eq!(first_left_width, second_left_width);
        assert!(first.spans[3].content.is_empty());
        assert!(second.spans[3].content.is_empty());

        let palette = transcript_palette();
        let selected_command_style = first.spans[1].style;
        assert_eq!(
            selected_command_style.fg,
            Some(palette.color(TranscriptColorRole::Accent))
        );
        assert!(selected_command_style.add_modifier.contains(Modifier::BOLD));
        assert!(
            selected_command_style
                .add_modifier
                .contains(Modifier::REVERSED)
        );

        let unselected_command_style = second.spans[1].style;
        assert_eq!(
            unselected_command_style.fg,
            Some(palette.color(TranscriptColorRole::Assistant))
        );
        assert!(
            unselected_command_style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !unselected_command_style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            unselected_command_style
                .sub_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn command_palette_windows_total_height_nine_rows() {
        let commands = (0..12)
            .map(|index| AgentTuiCommandSuggestion {
                command: format!("/cmd{index}"),
                description: format!("命令 {index}"),
                argument_hint: String::new(),
            })
            .collect::<Vec<_>>();
        let area = Rect::new(0, 0, 80, 9);
        let mut buffer = Buffer::empty(area);

        assert_eq!(CommandPalette::required_height(commands.len()), 9);

        CommandPalette {
            commands: &commands,
            title: COMMAND_PANEL_DEFAULT_TITLE,
            selected: 11,
        }
        .render(area, &mut buffer);
        let text = buffer_text(&buffer);

        assert!(text.contains("/cmd4"));
        assert!(text.contains("/cmd11"));
        assert!(!text.contains("/cmd3"));
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }
}
