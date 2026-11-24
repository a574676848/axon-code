//! TUI 快捷键语义集中定义。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn is_exit_key(key: &KeyEvent) -> bool {
    key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE
}

pub(crate) fn is_runtime_exit_key(key: &KeyEvent) -> bool {
    is_exit_key(key)
}

pub(crate) fn is_text_input_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_)) && !has_reserved_text_modifier(key.modifiers)
}

fn has_reserved_text_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        || modifiers.contains(KeyModifiers::ALT)
        || modifiers.contains(KeyModifiers::SUPER)
        || modifiers.contains(KeyModifiers::HYPER)
        || modifiers.contains(KeyModifiers::META)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{is_exit_key, is_runtime_exit_key, is_text_input_key};

    #[test]
    fn esc_is_exit_key_and_ctrl_c_is_not_tui_control() {
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert!(is_exit_key(&esc));
        assert!(is_runtime_exit_key(&esc));
        assert!(!is_exit_key(&ctrl_c));
        assert!(!is_runtime_exit_key(&ctrl_c));
    }

    #[test]
    fn text_input_excludes_terminal_control_shortcuts() {
        let plain = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let shifted = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let alt_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);

        assert!(is_text_input_key(&plain));
        assert!(is_text_input_key(&shifted));
        assert!(!is_text_input_key(&ctrl_c));
        assert!(!is_text_input_key(&alt_x));
    }
}
