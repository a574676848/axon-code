const ANSI_RESET: &str = "\x1b[0m";

#[derive(Clone, Copy)]
pub(super) enum SetupStyle {
    Muted,
    Accent,
    Success,
    Warning,
    Error,
}

pub(super) fn paint(style: SetupStyle, text: impl std::fmt::Display) -> String {
    if std::env::var_os("NO_COLOR").is_some() {
        return text.to_string();
    }

    #[cfg(feature = "tui")]
    {
        use axon_tui_runtime::{TerminalTextStyleRole, terminal_text_style_prefix};

        let role = match style {
            SetupStyle::Muted => TerminalTextStyleRole::Muted,
            SetupStyle::Accent => TerminalTextStyleRole::Accent,
            SetupStyle::Success => TerminalTextStyleRole::Success,
            SetupStyle::Warning => TerminalTextStyleRole::Warning,
            SetupStyle::Error => TerminalTextStyleRole::Error,
        };
        let prefix = terminal_text_style_prefix(role);
        if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}{text}{ANSI_RESET}")
        }
    }

    #[cfg(not(feature = "tui"))]
    {
        let _ = style;
        text.to_string()
    }
}
