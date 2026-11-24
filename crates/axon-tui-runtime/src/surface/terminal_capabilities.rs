//! 终端兼容能力识别与降级策略。

use std::sync::OnceLock;

use serde::Serialize;

const PROFILE_WINDOWS_TERMINAL: &str = "windows_terminal";
const PROFILE_CONHOST: &str = "conhost";
const PROFILE_WSL: &str = "wsl";
const PROFILE_MACOS_TERMINAL: &str = "macos_terminal";
const PROFILE_ITERM2: &str = "iterm2";
const PROFILE_GHOSTTY: &str = "ghostty";
const PROFILE_WEZTERM: &str = "wezterm";
const PROFILE_ALACRITTY: &str = "alacritty";
const PROFILE_XTERM: &str = "xterm";
const PROFILE_TMUX: &str = "tmux";
const PROFILE_SCREEN: &str = "screen";
const PROFILE_VSCODE: &str = "vscode";
const PROFILE_UNKNOWN: &str = "unknown";

const DEGRADE_HYPERLINK_OSC8: &str = "osc8";
const DEGRADE_HYPERLINK_PLAIN_TEXT: &str = "plain_text";
const DEGRADE_COPY_SYSTEM_CLIPBOARD: &str = "system_clipboard";
const DEGRADE_COPY_FAIL_SILENT: &str = "copy_fail_silent";
const DEGRADE_MOUSE_ENABLED: &str = "mouse_enabled";
const DEGRADE_MOUSE_KEYBOARD_ONLY: &str = "keyboard_only";
const DEGRADE_RESIZE_RENDER_VIEWPORT_REFLOW: &str = "render_viewport_reflow";
const DEGRADE_RESIZE_SLOW_LINK_REFLOW: &str = "slow_link_render_viewport_reflow";

static TERMINAL_COMPATIBILITY_POLICY: OnceLock<TerminalCompatibilityPolicy> = OnceLock::new();

/// TUI 当前宿主终端的兼容能力快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalCompatibilitySnapshot {
    /// 识别出的终端族。
    pub profile: &'static str,
    /// 本地、SSH 或慢速 SSH 链路。
    pub transport: &'static str,
    /// 是否允许向 Buffer 注入 OSC 8 hyperlink。
    pub osc8_hyperlinks: bool,
    /// 是否允许使用 OSC 52 协议剪贴板。
    pub osc52_clipboard: bool,
    /// 是否把鼠标作为可用交互增强。
    pub mouse: bool,
    /// 是否按 unicode-width 计算显示宽度。
    pub unicode_width: bool,
    /// 是否支持 resize 后由 RenderViewport 重排历史。
    pub resize_reflow: bool,
    /// 是否启用 synchronized update 边界。
    pub synchronized_update: bool,
    /// 是否启用 alternate scroll 模式。
    pub alternate_scroll: bool,
    /// 是否识别为慢速远端链路。
    pub slow_link: bool,
    /// hyperlink 渲染模式。
    pub hyperlink_mode: &'static str,
    /// 复制降级模式。
    pub copy_mode: &'static str,
    /// 鼠标降级模式。
    pub mouse_mode: &'static str,
    /// resize 降级模式。
    pub resize_mode: &'static str,
    /// 人类可读的兼容性说明。
    pub notes: Vec<&'static str>,
}

impl Default for TerminalCompatibilitySnapshot {
    fn default() -> Self {
        TerminalCompatibilityPolicy::from_env().snapshot()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalCompatibilityPolicy {
    profile: &'static str,
    transport: TerminalTransport,
    osc8_hyperlinks: bool,
    osc52_clipboard: bool,
    mouse: bool,
    synchronized_update: bool,
    alternate_scroll: bool,
    notes: &'static [&'static str],
}

impl TerminalCompatibilityPolicy {
    pub(crate) fn current() -> Self {
        *TERMINAL_COMPATIBILITY_POLICY.get_or_init(Self::from_env)
    }

    pub(crate) fn from_env() -> Self {
        Self::from_environment(&TerminalEnvironment::from_env())
    }

    pub(crate) fn from_environment(env: &TerminalEnvironment) -> Self {
        let transport = TerminalTransport::from_environment(env);
        let mut policy = if env.has("TMUX") {
            Self::tmux(transport)
        } else if env.has("STY") {
            Self::screen(transport)
        } else if env.has("WT_SESSION") {
            Self::windows_terminal(transport)
        } else if env.has("WSL_DISTRO_NAME") {
            Self::wsl(transport)
        } else {
            Self::direct_terminal(env, transport)
        };
        if transport == TerminalTransport::SshSlowLink {
            policy.osc52_clipboard = false;
            policy.mouse = false;
        }
        policy
    }

    pub(crate) const fn supports_osc8_hyperlinks(self) -> bool {
        self.osc8_hyperlinks
    }

    pub(crate) const fn supports_osc52_clipboard(self) -> bool {
        self.osc52_clipboard
    }

    pub(crate) const fn supports_mouse(self) -> bool {
        self.mouse
    }

    pub(crate) const fn supports_diff_viewer(self) -> bool {
        self.synchronized_update && !matches!(self.transport, TerminalTransport::SshSlowLink)
    }

    pub(crate) fn snapshot(self) -> TerminalCompatibilitySnapshot {
        TerminalCompatibilitySnapshot {
            profile: self.profile,
            transport: self.transport.as_str(),
            osc8_hyperlinks: self.osc8_hyperlinks,
            osc52_clipboard: self.osc52_clipboard,
            mouse: self.mouse,
            unicode_width: true,
            resize_reflow: true,
            synchronized_update: self.synchronized_update,
            alternate_scroll: self.alternate_scroll,
            slow_link: self.transport == TerminalTransport::SshSlowLink,
            hyperlink_mode: if self.osc8_hyperlinks {
                DEGRADE_HYPERLINK_OSC8
            } else {
                DEGRADE_HYPERLINK_PLAIN_TEXT
            },
            copy_mode: if self.osc52_clipboard {
                DEGRADE_COPY_SYSTEM_CLIPBOARD
            } else {
                DEGRADE_COPY_FAIL_SILENT
            },
            mouse_mode: if self.mouse {
                DEGRADE_MOUSE_ENABLED
            } else {
                DEGRADE_MOUSE_KEYBOARD_ONLY
            },
            resize_mode: if self.transport == TerminalTransport::SshSlowLink {
                DEGRADE_RESIZE_SLOW_LINK_REFLOW
            } else {
                DEGRADE_RESIZE_RENDER_VIEWPORT_REFLOW
            },
            notes: self.notes.to_vec(),
        }
    }

    const fn windows_terminal(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_WINDOWS_TERMINAL,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: true,
            alternate_scroll: true,
            notes: &["Windows Terminal 支持 OSC 8，复制走系统剪贴板而非 OSC 52。"],
        }
    }

    const fn wsl(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_WSL,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: true,
            alternate_scroll: true,
            notes: &["WSL 默认继承宿主终端能力，Axon 不依赖 OSC 52。"],
        }
    }

    const fn wezterm(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_WEZTERM,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: true,
            mouse: false,
            synchronized_update: true,
            alternate_scroll: true,
            notes: &["WezTerm 支持 OSC 8，Axon 仍优先使用系统剪贴板。"],
        }
    }

    const fn alacritty(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_ALACRITTY,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: false,
            alternate_scroll: true,
            notes: &["Alacritty 链接能力按现代版本处理，复制不依赖 OSC 52。"],
        }
    }

    const fn iterm2(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_ITERM2,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: true,
            mouse: false,
            synchronized_update: true,
            alternate_scroll: true,
            notes: &["iTerm2 支持 OSC 链路，但 tmux/screen 内需显式透传。"],
        }
    }

    const fn ghostty(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_GHOSTTY,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: true,
            alternate_scroll: true,
            notes: &["Ghostty 按现代终端处理 OSC 8，复制走系统剪贴板。"],
        }
    }

    const fn macos_terminal(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_MACOS_TERMINAL,
            transport,
            osc8_hyperlinks: false,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: false,
            alternate_scroll: true,
            notes: &["macOS Terminal 不可靠支持 OSC 8，链接降级为纯文本。"],
        }
    }

    const fn xterm(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_XTERM,
            transport,
            osc8_hyperlinks: true,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: false,
            alternate_scroll: true,
            notes: &["xterm 能力差异较大，复制默认不走 OSC 52。"],
        }
    }

    const fn conhost(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_CONHOST,
            transport,
            osc8_hyperlinks: false,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: false,
            alternate_scroll: true,
            notes: &["conhost 采用保守降级：纯文本链接、键盘优先、系统剪贴板。"],
        }
    }

    const fn tmux(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_TMUX,
            transport,
            osc8_hyperlinks: false,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: false,
            alternate_scroll: true,
            notes: &["tmux 默认按未配置 passthrough 处理；配置透传前链接降级纯文本。"],
        }
    }

    const fn screen(transport: TerminalTransport) -> Self {
        Self {
            profile: PROFILE_SCREEN,
            transport,
            osc8_hyperlinks: false,
            osc52_clipboard: false,
            mouse: false,
            synchronized_update: false,
            alternate_scroll: true,
            notes: &["screen 对 OSC 8/OSC 52 不稳定，默认键盘和纯文本降级。"],
        }
    }

    fn direct_terminal(env: &TerminalEnvironment, transport: TerminalTransport) -> Self {
        let term_program = env.lower("TERM_PROGRAM");
        let term = env.lower("TERM");
        if running_in_vscode_terminal(env, &term_program) || term.contains("vscode") {
            return Self {
                profile: PROFILE_VSCODE,
                transport,
                osc8_hyperlinks: true,
                osc52_clipboard: false,
                mouse: false,
                synchronized_update: true,
                alternate_scroll: true,
                notes: &["VS Code 终端按现代 xterm.js 能力处理，复制走宿主剪贴板。"],
            };
        }
        if env.has("WEZTERM_EXECUTABLE") || term_program.contains("wezterm") {
            return Self::wezterm(transport);
        }
        if env.has("ALACRITTY_SOCKET") || term_program.contains("alacritty") {
            return Self::alacritty(transport);
        }
        if env.has("GHOSTTY_RESOURCES_DIR") || term_program.contains("ghostty") {
            return Self::ghostty(transport);
        }
        if env.has("ITERM_SESSION_ID") || term_program.contains("iterm") {
            return Self::iterm2(transport);
        }
        if term_program.contains("apple_terminal") {
            return Self::macos_terminal(transport);
        }
        if term.contains("xterm") {
            return Self::xterm(transport);
        }
        if cfg!(windows) {
            Self::conhost(transport)
        } else {
            Self {
                profile: PROFILE_UNKNOWN,
                transport,
                osc8_hyperlinks: false,
                osc52_clipboard: false,
                mouse: false,
                synchronized_update: false,
                alternate_scroll: true,
                notes: &["未知终端采用保守降级，确保核心输入和历史查看可用。"],
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalTransport {
    Local,
    Ssh,
    SshSlowLink,
}

impl TerminalTransport {
    fn from_environment(env: &TerminalEnvironment) -> Self {
        if env.has("AXON_TUI_SSH_SLOW_LINK") {
            return Self::SshSlowLink;
        }
        if env.has("SSH_CONNECTION") || env.has("SSH_TTY") {
            return Self::Ssh;
        }
        Self::Local
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh => "ssh",
            Self::SshSlowLink => "ssh_slow_link",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalEnvironment {
    vars: Vec<(&'static str, String)>,
}

impl TerminalEnvironment {
    pub(crate) fn from_env() -> Self {
        Self::from_pairs(
            TERMINAL_ENV_KEYS
                .iter()
                .filter_map(|key| std::env::var(key).ok().map(|value| (*key, value))),
        )
    }

    pub(crate) fn from_pairs<I, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (&'static str, V)>,
        V: Into<String>,
    {
        Self {
            vars: pairs
                .into_iter()
                .map(|(key, value)| (key, value.into()))
                .collect(),
        }
    }

    fn has(&self, key: &str) -> bool {
        self.vars
            .iter()
            .any(|(name, value)| *name == key && !value.trim().is_empty())
    }

    fn lower(&self, key: &str) -> String {
        self.vars
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.to_ascii_lowercase())
            .unwrap_or_default()
    }
}

const TERMINAL_ENV_KEYS: &[&str] = &[
    "TERM",
    "TERM_PROGRAM",
    "WT_SESSION",
    "WSL_DISTRO_NAME",
    "WEZTERM_EXECUTABLE",
    "ALACRITTY_SOCKET",
    "GHOSTTY_RESOURCES_DIR",
    "ITERM_SESSION_ID",
    "TMUX",
    "STY",
    "SSH_CONNECTION",
    "SSH_TTY",
    "VSCODE_INJECTION",
    "AXON_TUI_SSH_SLOW_LINK",
];

fn running_in_vscode_terminal(env: &TerminalEnvironment, term_program: &str) -> bool {
    term_program.contains("vscode") || env.has("VSCODE_INJECTION")
}

#[cfg(test)]
mod tests {
    use super::{TerminalCompatibilityPolicy, TerminalEnvironment};

    #[test]
    fn windows_terminal_supports_osc8_but_not_osc52_copy() {
        let policy =
            TerminalCompatibilityPolicy::from_environment(&TerminalEnvironment::from_pairs([
                ("WT_SESSION", "1"),
                ("TERM", "xterm-256color"),
            ]));

        let snapshot = policy.snapshot();
        assert_eq!(snapshot.profile, "windows_terminal");
        assert!(snapshot.osc8_hyperlinks);
        assert_eq!(snapshot.hyperlink_mode, "osc8");
        assert!(!snapshot.osc52_clipboard);
        assert_eq!(snapshot.copy_mode, "copy_fail_silent");
    }

    #[test]
    fn tmux_and_screen_default_to_plain_text_and_keyboard_only() {
        for (key, profile) in [("TMUX", "tmux"), ("STY", "screen")] {
            let snapshot =
                TerminalCompatibilityPolicy::from_environment(&TerminalEnvironment::from_pairs([
                    (key, "active"),
                ]))
                .snapshot();

            assert_eq!(snapshot.profile, profile);
            assert!(!snapshot.osc8_hyperlinks);
            assert_eq!(snapshot.hyperlink_mode, "plain_text");
            assert_eq!(snapshot.mouse_mode, "keyboard_only");
        }
    }

    #[test]
    fn ssh_slow_link_uses_viewport_reflow_and_no_protocol_clipboard() {
        let snapshot =
            TerminalCompatibilityPolicy::from_environment(&TerminalEnvironment::from_pairs([
                ("TERM_PROGRAM", "WezTerm"),
                ("AXON_TUI_SSH_SLOW_LINK", "1"),
            ]))
            .snapshot();

        assert_eq!(snapshot.profile, "wezterm");
        assert!(snapshot.slow_link);
        assert_eq!(snapshot.transport, "ssh_slow_link");
        assert_eq!(snapshot.resize_mode, "slow_link_render_viewport_reflow");
        assert!(!snapshot.osc52_clipboard);
    }
}
