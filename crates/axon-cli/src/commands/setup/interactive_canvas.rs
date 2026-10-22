use std::io::{self, IsTerminal, Write as _};

use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate},
};
use unicode_width::UnicodeWidthStr;

use super::setup_interactive_style::{SetupStyle, paint};

const HORIZONTAL_MARGIN_COLUMNS: u16 = 4;
const VERTICAL_MARGIN_ROWS: u16 = 1;
const PROMPT_ROWS: u16 = 1;
const FRAME_VERTICAL_PADDING_ROWS: u16 = 2;
const MIN_FRAME_WIDTH: u16 = 60;
const TWO_COLUMN_MIN_WIDTH: u16 = 100;
const COLUMN_GAP_WIDTH: usize = 5;
const ANSI_RESET: &str = "\x1b[0m";

pub(super) const ESCAPE_CHOICE: &str = "\u{1b}";

pub(super) fn menu_rows(values: &[String], focus: usize, selected: Option<usize>) -> Vec<String> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let prefix = if index == focus { ">" } else { " " };
            let row = selected.map_or_else(
                || format!("{prefix} {value}"),
                |selected_index| {
                    let marker = if selected_index == index {
                        "[✓]"
                    } else {
                        "[ ]"
                    };
                    format!("{prefix} {marker} {value}")
                },
            );
            match (index == focus, selected == Some(index)) {
                (true, _) => paint(SetupStyle::Accent, row),
                (false, true) => paint(SetupStyle::Success, row),
                (false, false) => row,
            }
        })
        .collect()
}

pub(super) fn render_setup_frame(title: &str, lines: &[String]) {
    let mut stderr = io::stderr();
    let Ok((terminal_width, terminal_height)) = terminal::size() else {
        print_compact_frame(title, lines);
        return;
    };
    if !stderr.is_terminal() || terminal_width < MIN_FRAME_WIDTH {
        print_compact_frame(title, lines);
        return;
    }

    let margin = HORIZONTAL_MARGIN_COLUMNS.min(terminal_width / 8);
    let inner_width = usize::from(terminal_width.saturating_sub(margin * 2 + 4));
    let rendered_lines = lines
        .iter()
        .flat_map(|line| wrap_line(line, inner_width))
        .collect::<Vec<_>>();
    let content_height = u16::try_from(rendered_lines.len()).unwrap_or(u16::MAX);
    let minimum_height = content_height
        .saturating_add(FRAME_VERTICAL_PADDING_ROWS.saturating_mul(2))
        .saturating_add(2);
    let available_height = terminal_height.saturating_sub(VERTICAL_MARGIN_ROWS.saturating_mul(2));
    if available_height < minimum_height.saturating_add(PROMPT_ROWS) {
        print_compact_frame(title, lines);
        return;
    }
    let frame_height = available_height.saturating_sub(PROMPT_ROWS);
    let top_padding = terminal_height
        .saturating_sub(frame_height.saturating_add(PROMPT_ROWS))
        .saturating_div(2);
    let vertical_padding = frame_height
        .saturating_sub(content_height)
        .saturating_sub(2);
    let top_inner_padding = vertical_padding.saturating_div(2);
    let bottom_inner_padding = vertical_padding.saturating_sub(top_inner_padding);
    let left_padding = " ".repeat(usize::from(margin));
    let heading_width = display_width(title).saturating_add(2);

    let _ = execute!(
        stderr,
        BeginSynchronizedUpdate,
        Clear(ClearType::All),
        MoveTo(0, 0)
    );
    for _ in 0..top_padding {
        let _ = write_terminal_line(&mut stderr, "");
    }
    let _ = write_terminal_line(
        &mut stderr,
        &format!(
            "{left_padding}┌─ {title} {}┐",
            "─".repeat(inner_width.saturating_sub(heading_width))
        ),
    );
    for _ in 0..top_inner_padding {
        let _ = write_terminal_line(
            &mut stderr,
            &format!("{left_padding}│ {} │", " ".repeat(inner_width)),
        );
    }
    for line in &rendered_lines {
        let padding = inner_width.saturating_sub(display_width(line));
        let _ = write_terminal_line(
            &mut stderr,
            &format!("{left_padding}│ {line}{} │", " ".repeat(padding)),
        );
    }
    for _ in 0..bottom_inner_padding {
        let _ = write_terminal_line(
            &mut stderr,
            &format!("{left_padding}│ {} │", " ".repeat(inner_width)),
        );
    }
    let _ = write_terminal_line(
        &mut stderr,
        &format!("{left_padding}└{}┘", "─".repeat(inner_width + 2)),
    );
    let _ = execute!(stderr, EndSynchronizedUpdate);
    let _ = stderr.flush();
}

pub(super) fn read_frame_choice(render: impl Fn(&str), prompt: &str) -> String {
    if !io::stdin().is_terminal() || terminal::enable_raw_mode().is_err() {
        return read_line_choice();
    }

    let mut choice = String::new();
    loop {
        render(&format!("{prompt} {choice}"));
        match event::read() {
            Ok(Event::Resize(_, _)) => {}
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Enter => break,
                KeyCode::Esc => {
                    choice = ESCAPE_CHOICE.to_owned();
                    break;
                }
                KeyCode::Backspace => {
                    choice.pop();
                }
                KeyCode::Char(character) => {
                    choice.push(character);
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = terminal::disable_raw_mode();
    choice
}

pub(super) fn two_column_rows(values: &[String]) -> Vec<String> {
    if menu_column_count() == 1 {
        return values.to_vec();
    }

    let Ok((terminal_width, _)) = terminal::size() else {
        return values.to_vec();
    };
    let margin = HORIZONTAL_MARGIN_COLUMNS.min(terminal_width / 8);
    let inner_width = usize::from(terminal_width.saturating_sub(margin * 2 + 4));
    let column_width = inner_width
        .saturating_sub(COLUMN_GAP_WIDTH)
        .saturating_div(2);
    values
        .chunks(2)
        .flat_map(|pair| {
            let left_lines = wrap_line(&pair[0], column_width);
            let right_lines = pair.get(1).map(|right| wrap_line(right, column_width));
            let row_count = right_lines
                .as_ref()
                .map_or(left_lines.len(), |right| left_lines.len().max(right.len()));
            (0..row_count).map(move |index| {
                let left = left_lines.get(index).cloned().unwrap_or_default();
                let left_padding = column_width.saturating_sub(display_width(&left));
                right_lines.as_ref().map_or(left.clone(), |right| {
                    format!(
                        "{left}{}{}",
                        " ".repeat(left_padding.saturating_add(COLUMN_GAP_WIDTH)),
                        right.get(index).cloned().unwrap_or_default()
                    )
                })
            })
        })
        .collect()
}

pub(super) fn read_menu_choice(
    render: impl Fn(&str, usize, Option<usize>),
    prompt: &str,
    choices: &[String],
    initial_focus: usize,
    selectable: bool,
) -> String {
    read_menu_choice_with_columns(
        render,
        prompt,
        choices,
        initial_focus,
        selectable,
        menu_column_count(),
    )
}

pub(super) fn read_single_column_menu_choice(
    render: impl Fn(&str, usize, Option<usize>),
    prompt: &str,
    choices: &[String],
    initial_focus: usize,
    selectable: bool,
) -> String {
    read_menu_choice_with_columns(render, prompt, choices, initial_focus, selectable, 1)
}

fn read_menu_choice_with_columns(
    render: impl Fn(&str, usize, Option<usize>),
    prompt: &str,
    choices: &[String],
    initial_focus: usize,
    selectable: bool,
    columns: usize,
) -> String {
    if choices.is_empty() {
        return String::new();
    }
    if !io::stdin().is_terminal() || terminal::enable_raw_mode().is_err() {
        return read_line_choice();
    }

    let mut focus = initial_focus.min(choices.len().saturating_sub(1));
    let mut selected = None;
    loop {
        render(prompt, focus, selected);
        match event::read() {
            Ok(Event::Resize(_, _)) => {}
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Up => {
                    focus = move_focus_vertically_in_columns(focus, choices.len(), false, columns)
                }
                KeyCode::Down => {
                    focus = move_focus_vertically_in_columns(focus, choices.len(), true, columns)
                }
                KeyCode::Left => {
                    focus = move_focus_horizontally_in_columns(focus, choices.len(), false, columns)
                }
                KeyCode::Right => {
                    focus = move_focus_horizontally_in_columns(focus, choices.len(), true, columns)
                }
                KeyCode::Char(' ') if selectable => selected = Some(focus),
                KeyCode::Enter => {
                    let choice = selected.unwrap_or(focus);
                    let _ = terminal::disable_raw_mode();
                    return choices[choice].clone();
                }
                KeyCode::Esc => {
                    let _ = terminal::disable_raw_mode();
                    return ESCAPE_CHOICE.to_owned();
                }
                _ => {}
            },
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = terminal::disable_raw_mode();
    String::new()
}

fn print_compact_frame(title: &str, lines: &[String]) {
    let mut stderr = io::stderr();
    let _ = write_terminal_line(
        &mut stderr,
        &format!(
            "\r\n┌─ {title} ─────────────────────────────────────────────────────────────────┐"
        ),
    );
    for line in lines {
        let _ = write_terminal_line(&mut stderr, &format!("│ {line}"));
    }
    let _ = write_terminal_line(
        &mut stderr,
        "└─────────────────────────────────────────────────────────────────────────────┘",
    );
}

fn write_terminal_line(writer: &mut impl io::Write, line: &str) -> io::Result<()> {
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\r\n")
}

fn read_line_choice() -> String {
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
    input.trim().to_owned()
}

fn menu_column_count() -> usize {
    if terminal::size().is_ok_and(|(width, _)| width >= TWO_COLUMN_MIN_WIDTH) {
        2
    } else {
        1
    }
}

fn move_focus_vertically_in_columns(
    focus: usize,
    count: usize,
    down: bool,
    columns: usize,
) -> usize {
    if down {
        if focus + columns < count {
            focus + columns
        } else {
            focus
        }
    } else {
        focus.checked_sub(columns).unwrap_or(focus)
    }
}

fn move_focus_horizontally_in_columns(
    focus: usize,
    count: usize,
    right: bool,
    columns: usize,
) -> usize {
    if columns == 1 {
        return focus;
    }
    if right {
        if focus.is_multiple_of(2) && focus + 1 < count {
            focus + 1
        } else {
            focus
        }
    } else if !focus.is_multiple_of(2) {
        focus - 1
    } else {
        focus
    }
}

fn display_width(value: &str) -> usize {
    let mut plain = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' && chars.next_if_eq(&'[').is_some() {
            for escape_character in chars.by_ref() {
                if escape_character.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain.width()
}

fn wrap_line(value: &str, maximum_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut output = String::new();
    let mut width: usize = 0;
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\x1b' && chars.next_if_eq(&'[').is_some() {
            output.push(character);
            output.push('[');
            for escape_character in chars.by_ref() {
                output.push(escape_character);
                if escape_character.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        let character_width = character.to_string().width();
        if width.saturating_add(character_width) > maximum_width && !output.is_empty() {
            if output.contains("\x1b[") {
                output.push_str(ANSI_RESET);
            }
            lines.push(output);
            output = String::new();
            width = 0;
        }
        output.push(character);
        width += character_width;
    }
    if output.contains("\x1b[") {
        output.push_str(ANSI_RESET);
    }
    lines.push(output);
    lines
}

#[cfg(test)]
mod tests {
    use super::{
        display_width, menu_rows, move_focus_horizontally_in_columns,
        move_focus_vertically_in_columns, two_column_rows, wrap_line,
    };

    #[test]
    fn display_width_ignores_ansi_styles_and_counts_cjk_columns() {
        assert_eq!(display_width("\x1b[38;2;1;2;3m状态\x1b[0m"), 4);
    }

    #[test]
    fn wrap_line_splits_ansi_colored_wide_text_without_overflow() {
        let values = wrap_line(
            "\x1b[32mProvider 地址：https://example.test/very/long/path\x1b[0m",
            16,
        );
        assert!(values.len() > 1);
        assert!(values.iter().all(|value| display_width(value) <= 16));
        assert!(
            values
                .first()
                .is_some_and(|value| value.ends_with("\x1b[0m"))
        );
    }

    #[test]
    fn two_column_rows_keeps_an_odd_last_item() {
        let values = vec![
            "[1] 一".to_owned(),
            "[2] 二".to_owned(),
            "[3] 三".to_owned(),
        ];
        let rows = two_column_rows(&values);
        assert!(!rows.is_empty());
        assert!(rows.last().is_some_and(|row| row.contains("[3] 三")));
    }

    #[test]
    fn menu_rows_marks_focus_and_selected_item() {
        let rows = menu_rows(&["第一项".to_owned(), "第二项".to_owned()], 1, Some(1));
        assert!(rows[0].contains("[ ] 第一项"));
        assert!(rows[1].contains("> [✓] 第二项"));
    }

    #[test]
    fn menu_rows_omits_selection_icons_for_navigation() {
        let rows = menu_rows(&["第一项".to_owned()], 0, None);
        assert!(rows[0].contains("> 第一项"));
        assert!(!rows[0].contains("[ ]"));
    }

    #[test]
    fn single_column_navigation_moves_only_up_and_down() {
        assert_eq!(move_focus_vertically_in_columns(1, 4, true, 1), 2);
        assert_eq!(move_focus_horizontally_in_columns(1, 4, true, 1), 1);
    }
}
