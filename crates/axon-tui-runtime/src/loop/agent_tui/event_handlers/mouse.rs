//! Agent TUI 鼠标事件语义映射。

use std::time::Instant;

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::ui_workflow::action::UiAction;

pub(super) fn map_mouse_to_action(mouse: MouseEvent) -> Option<UiAction> {
    if let Some(action) = map_visible_text_selection_action(mouse, Instant::now()) {
        return Some(action);
    }
    if let Some(action) = map_mouse_wheel_to_viewport_action(mouse) {
        return Some(action);
    }
    None
}

fn map_visible_text_selection_action(mouse: MouseEvent, now: Instant) -> Option<UiAction> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => Some(UiAction::VisibleTextSelectionBegin {
            row: mouse.row,
            column: mouse.column,
        }),
        MouseEventKind::Drag(MouseButton::Left) => Some(UiAction::VisibleTextSelectionDrag {
            row: mouse.row,
            column: mouse.column,
        }),
        MouseEventKind::Up(MouseButton::Left) => Some(UiAction::VisibleTextSelectionRelease {
            row: mouse.row,
            column: mouse.column,
            now,
        }),
        _ => None,
    }
}

fn map_mouse_wheel_to_viewport_action(mouse: MouseEvent) -> Option<UiAction> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(UiAction::ScrollActiveSurfaceUp),
        MouseEventKind::ScrollDown => Some(UiAction::ScrollActiveSurfaceDown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use super::map_visible_text_selection_action;
    use crate::ui_workflow::action::UiAction;

    #[test]
    fn mouse_release_keeps_final_selection_position() {
        let now = Instant::now();
        let action = map_visible_text_selection_action(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 47,
                row: 9,
                modifiers: KeyModifiers::NONE,
            },
            now,
        );

        assert_eq!(
            action,
            Some(UiAction::VisibleTextSelectionRelease {
                row: 9,
                column: 47,
                now,
            })
        );
    }
}
