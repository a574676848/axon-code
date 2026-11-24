//! Agent TUI 终端会话生命周期守卫。

use std::{fmt, io, io::Write, panic};

use crossterm::{
    Command,
    cursor::{self, SetCursorStyle},
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;

use crate::app_shell::layout::AGENT_TUI_INITIAL_VIEWPORT_HEIGHT;

use super::{AgentTuiError, custom_terminal::Terminal};

pub(super) type AgentTerminal = Terminal<CrosstermBackend<io::Stdout>>;

pub(super) struct AgentTerminalSession<W = io::Stdout>
where
    W: Write,
{
    terminal: Terminal<CrosstermBackend<W>>,
    restored: bool,
}

impl AgentTerminalSession<io::Stdout> {
    pub(super) fn start() -> Result<Self, AgentTuiError> {
        install_panic_restore_hook();
        terminal::enable_raw_mode().map_err(AgentTuiError::Io)?;
        let mut stdout = io::stdout();
        enable_agent_terminal_modes(&mut stdout).map_err(AgentTuiError::Io)?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend, AGENT_TUI_INITIAL_VIEWPORT_HEIGHT).map_err(AgentTuiError::Io)?;
        terminal.clear().map_err(AgentTuiError::Io)?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    pub(super) fn terminal_mut(&mut self) -> &mut AgentTerminal {
        &mut self.terminal
    }
}

impl<W> AgentTerminalSession<W>
where
    W: Write,
{
    pub(super) fn restore(&mut self) -> Result<(), AgentTuiError> {
        if self.restored {
            return Ok(());
        }
        restore_terminal(&mut self.terminal)?;
        self.restored = true;
        Ok(())
    }
}

impl<W> Drop for AgentTerminalSession<W>
where
    W: Write,
{
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
impl AgentTerminalSession {
    fn restored_for_test() -> AgentTerminalSession<Vec<u8>> {
        AgentTerminalSession {
            terminal: Terminal::new_for_test(
                CrosstermBackend::new(Vec::new()),
                AGENT_TUI_INITIAL_VIEWPORT_HEIGHT,
            )
            .expect("测试终端包装应可构造"),
            restored: true,
        }
    }
}

pub(super) fn suspend_for_external_editor<W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
) -> Result<(), AgentTuiError>
where
    W: Write,
{
    restore_terminal(terminal)
}

pub(super) fn resume_after_external_editor<W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
) -> Result<(), AgentTuiError>
where
    W: Write,
{
    terminal::enable_raw_mode().map_err(AgentTuiError::Io)?;
    enable_agent_terminal_modes(terminal.backend_mut()).map_err(AgentTuiError::Io)?;
    terminal.clear().map_err(AgentTuiError::Io)
}

fn enable_agent_terminal_modes(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(
        writer,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableAlternateScroll,
        SetCursorStyle::BlinkingBar,
        cursor::Show
    )
}

fn restore_terminal<W>(terminal: &mut Terminal<CrosstermBackend<W>>) -> Result<(), AgentTuiError>
where
    W: Write,
{
    restore_terminal_modes(terminal.backend_mut())
}

fn restore_terminal_modes(writer: &mut impl io::Write) -> Result<(), AgentTuiError> {
    let mut first_error: Option<io::Error> = None;
    if let Err(error) = execute!(
        writer,
        cursor::Show,
        SetCursorStyle::DefaultUserShape,
        DisableBracketedPaste,
        DisableAlternateScroll,
        DisableMouseCapture,
        LeaveAlternateScreen
    ) {
        first_error = Some(error);
    }
    if let Err(error) = terminal::disable_raw_mode() {
        first_error.get_or_insert(error);
    }
    first_error.map_or(Ok(()), |error| Err(AgentTuiError::Io(error)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnableAlternateScroll;

impl Command for EnableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "EnableAlternateScroll 必须通过 ANSI 序列执行",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007l")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "DisableAlternateScroll 必须通过 ANSI 序列执行",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

fn install_panic_restore_hook() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            let _ = restore_stdout_terminal();
            previous(panic_info);
        }));
    });
}

fn restore_stdout_terminal() -> Result<(), AgentTuiError> {
    restore_terminal_modes(&mut io::stdout())
}

#[cfg(test)]
mod tests {
    use crossterm::Command;
    use crossterm::event::{EnableBracketedPaste, EnableMouseCapture};
    use crossterm::terminal::EnterAlternateScreen;

    use super::{AgentTerminalSession, DisableAlternateScroll, EnableAlternateScroll};

    #[test]
    fn restored_terminal_session_is_idempotent() {
        let mut session = AgentTerminalSession::restored_for_test();

        session.restore().expect("已恢复的终端会话可重复恢复");
    }

    #[test]
    fn alternate_scroll_commands_match_fullscreen_app_shell_mode() {
        let mut enable = String::new();
        EnableAlternateScroll
            .write_ansi(&mut enable)
            .expect("enable alternate scroll ANSI 应可写入");
        let mut disable = String::new();
        DisableAlternateScroll
            .write_ansi(&mut disable)
            .expect("disable alternate scroll ANSI 应可写入");

        assert_eq!(enable, "\x1b[?1007h");
        assert_eq!(disable, "\x1b[?1007l");
    }

    #[test]
    fn agent_terminal_modes_encode_fullscreen_with_mouse_capture() {
        let mut alternate_screen = String::new();
        EnterAlternateScreen
            .write_ansi(&mut alternate_screen)
            .expect("alternate screen ANSI 应可写入");
        assert_eq!(alternate_screen, "\x1b[?1049h");

        let mut alternate_scroll = String::new();
        EnableAlternateScroll
            .write_ansi(&mut alternate_scroll)
            .expect("alternate scroll ANSI 应可写入");
        assert_eq!(alternate_scroll, "\x1b[?1007h");

        let mut bracketed_paste = String::new();
        EnableBracketedPaste
            .write_ansi(&mut bracketed_paste)
            .expect("bracketed paste ANSI 应可写入");
        assert_eq!(bracketed_paste, "\x1b[?2004h");

        let mut mouse_capture = String::new();
        EnableMouseCapture
            .write_ansi(&mut mouse_capture)
            .expect("mouse capture ANSI 应可写入");
        assert!(mouse_capture.contains("\x1b[?1000h"));
        assert!(mouse_capture.contains("\x1b[?1002h"));
        assert!(mouse_capture.contains("\x1b[?1015h"));
        assert!(mouse_capture.contains("\x1b[?1006h"));
    }
}
