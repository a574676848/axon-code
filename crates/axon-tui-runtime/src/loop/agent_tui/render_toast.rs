//! Agent TUI 临时 toast 渲染。

use ratatui::layout::Rect;
use ratatui::style::Style;

use super::{
    custom_terminal::Frame,
    state::{AgentTuiState, ToastKind},
};
use crate::app_shell::style_tokens::{ACCENT_GREEN, ACCENT_RED};

const TOAST_HORIZONTAL_PADDING_COLUMNS: u16 = 2;
const TOAST_RIGHT_MARGIN_COLUMNS: u16 = 1;

pub(super) fn render_toast(frame: &mut Frame, state: &AgentTuiState, composer_area: Rect) {
    let Some(toast) = state.current_toast(std::time::Instant::now()) else {
        return;
    };
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text_width = u16::try_from(toast.text.chars().count()).unwrap_or(u16::MAX);
    let width = text_width
        .saturating_add(TOAST_HORIZONTAL_PADDING_COLUMNS)
        .min(area.width);
    let x = area
        .right()
        .saturating_sub(width)
        .saturating_sub(TOAST_RIGHT_MARGIN_COLUMNS);
    let y = composer_area
        .y
        .saturating_sub(1)
        .min(area.bottom().saturating_sub(1));
    let color = match toast.kind {
        ToastKind::Success => ACCENT_GREEN,
        ToastKind::Error => ACCENT_RED,
    };
    let message = format!(" {}", toast.text);
    frame.buffer_mut().set_string(
        x,
        y,
        padded_to_width(&message, usize::from(width)),
        Style::new().fg(color),
    );
}

fn padded_to_width(value: &str, width: usize) -> String {
    let mut text = value.chars().take(width).collect::<String>();
    let current_width = text.chars().count();
    if current_width < width {
        text.push_str(&" ".repeat(width - current_width));
    }
    text
}
