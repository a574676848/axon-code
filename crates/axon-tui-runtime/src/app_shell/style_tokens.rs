//! TUI 视觉 token、palette fallback、wrap 策略与终端文本策略。
//!
//! 本模块是 TUI 颜色事实源：语义 transcript role、syntax role 与 ansi16/ansi256/true-color/no-color palette 均集中在这里。

use ratatui::style::Color;
use std::sync::OnceLock;

pub(crate) const TEXT_PRIMARY: Color = Color::Rgb(205, 214, 244);
pub(crate) const TEXT_SECONDARY: Color = Color::Rgb(171, 178, 189);
pub(crate) const TEXT_MUTED: Color = Color::Rgb(119, 130, 150);
pub(crate) const ACCENT_GREEN: Color = Color::Rgb(123, 196, 154);
const ACCENT_MINT_BASE_RGB: (u8, u8, u8) = (166, 227, 161);
const ACCENT_MINT_SATURATION_PERCENT: u16 = 85;
const PERCENT_SCALE: u16 = 100;
const PERCENT_ROUNDING_OFFSET: u16 = 50;
const RGB_LUMA_WEIGHTS: (u16, u16, u16) = (77, 150, 29);
const RGB_LUMA_SCALE: u16 = 256;
pub(crate) const ACCENT_MINT: Color = soften_rgb(ACCENT_MINT_BASE_RGB);
pub(crate) const ACCENT_RED: Color = Color::Rgb(218, 133, 133);
const ACCENT_WARNING: Color = Color::Rgb(249, 226, 175);
const INTERRUPTED_ORANGE: Color = Color::Rgb(217, 133, 94);
const ACCENT_MINT_ANSI256: Color = Color::Indexed(151);
const INTERRUPTED_ORANGE_ANSI256: Color = Color::Indexed(173);
const TEXT_PRIMARY_ANSI256: Color = Color::Indexed(189);
const TEXT_SECONDARY_ANSI256: Color = Color::Indexed(249);
const ACCENT_MINT_ANSI16: Color = Color::LightGreen;
pub(crate) const MESSAGE_INITIAL_PREFIX: &str = "• ";
pub(crate) const MESSAGE_CONTINUATION_PREFIX: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalPaletteDepth {
    NoColor,
    Ansi16,
    Ansi256,
    TrueColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptColorRole {
    User,
    Assistant,
    ToolRunning,
    ToolCompleted,
    ToolFailed,
    DiffAdded,
    DiffRemoved,
    DiffHunk,
    Error,
    Interrupted,
    Notice,
    Warning,
    Muted,
    Accent,
    Loading,
    ToolInput,
    SubagentActive,
    InlineHighlight,
    McpIdentifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptPalette {
    pub(crate) user: Color,
    pub(crate) assistant: Color,
    pub(crate) tool_running: Color,
    pub(crate) tool_completed: Color,
    pub(crate) tool_failed: Color,
    pub(crate) diff_added: Color,
    pub(crate) diff_removed: Color,
    pub(crate) diff_hunk: Color,
    pub(crate) error: Color,
    pub(crate) interrupted: Color,
    pub(crate) notice: Color,
    pub(crate) warning: Color,
    pub(crate) muted: Color,
    pub(crate) accent: Color,
    pub(crate) loading: Color,
    pub(crate) tool_input: Color,
    pub(crate) subagent_active: Color,
    pub(crate) inline_highlight: Color,
    pub(crate) mcp_identifier: Color,
}

impl TranscriptPalette {
    pub(crate) const TRUE_COLOR: Self = Self {
        user: ACCENT_MINT,
        assistant: TEXT_PRIMARY,
        tool_running: ACCENT_MINT,
        tool_completed: ACCENT_GREEN,
        tool_failed: ACCENT_RED,
        diff_added: Color::Green,
        diff_removed: Color::Red,
        diff_hunk: TEXT_MUTED,
        error: ACCENT_RED,
        interrupted: INTERRUPTED_ORANGE,
        notice: ACCENT_MINT,
        warning: ACCENT_WARNING,
        muted: TEXT_MUTED,
        accent: ACCENT_MINT,
        loading: ACCENT_MINT,
        tool_input: TEXT_SECONDARY,
        subagent_active: Color::Magenta,
        inline_highlight: ACCENT_MINT,
        mcp_identifier: ACCENT_MINT,
    };

    pub(crate) const ANSI256: Self = Self {
        user: ACCENT_MINT_ANSI256,
        assistant: TEXT_PRIMARY_ANSI256,
        tool_running: ACCENT_MINT_ANSI256,
        tool_completed: Color::Indexed(114),
        tool_failed: Color::Indexed(174),
        diff_added: Color::Green,
        diff_removed: Color::Red,
        diff_hunk: Color::Indexed(103),
        error: Color::Indexed(174),
        interrupted: INTERRUPTED_ORANGE_ANSI256,
        notice: ACCENT_MINT_ANSI256,
        warning: Color::Indexed(223),
        muted: Color::Indexed(103),
        accent: ACCENT_MINT_ANSI256,
        loading: ACCENT_MINT_ANSI256,
        tool_input: TEXT_SECONDARY_ANSI256,
        subagent_active: Color::Magenta,
        inline_highlight: ACCENT_MINT_ANSI256,
        mcp_identifier: ACCENT_MINT_ANSI256,
    };

    pub(crate) const ANSI16: Self = Self {
        user: ACCENT_MINT_ANSI16,
        assistant: Color::Gray,
        tool_running: ACCENT_MINT_ANSI16,
        tool_completed: Color::Green,
        tool_failed: Color::Red,
        diff_added: Color::Green,
        diff_removed: Color::Red,
        diff_hunk: Color::DarkGray,
        error: Color::Red,
        interrupted: Color::Yellow,
        notice: ACCENT_MINT_ANSI16,
        warning: Color::Yellow,
        muted: Color::DarkGray,
        accent: ACCENT_MINT_ANSI16,
        loading: ACCENT_MINT_ANSI16,
        tool_input: Color::Gray,
        subagent_active: Color::Magenta,
        inline_highlight: ACCENT_MINT_ANSI16,
        mcp_identifier: ACCENT_MINT_ANSI16,
    };

    pub(crate) const NO_COLOR: Self = Self {
        user: Color::Reset,
        assistant: Color::Reset,
        tool_running: Color::Reset,
        tool_completed: Color::Reset,
        tool_failed: Color::Reset,
        diff_added: Color::Reset,
        diff_removed: Color::Reset,
        diff_hunk: Color::Reset,
        error: Color::Reset,
        interrupted: Color::Reset,
        notice: Color::Reset,
        warning: Color::Reset,
        muted: Color::Gray,
        accent: Color::Reset,
        loading: Color::Reset,
        tool_input: Color::Reset,
        subagent_active: Color::Reset,
        inline_highlight: Color::Reset,
        mcp_identifier: Color::Reset,
    };

    pub(crate) const fn color(self, role: TranscriptColorRole) -> Color {
        match role {
            TranscriptColorRole::User => self.user,
            TranscriptColorRole::Assistant => self.assistant,
            TranscriptColorRole::ToolRunning => self.tool_running,
            TranscriptColorRole::ToolCompleted => self.tool_completed,
            TranscriptColorRole::ToolFailed => self.tool_failed,
            TranscriptColorRole::DiffAdded => self.diff_added,
            TranscriptColorRole::DiffRemoved => self.diff_removed,
            TranscriptColorRole::DiffHunk => self.diff_hunk,
            TranscriptColorRole::Error => self.error,
            TranscriptColorRole::Interrupted => self.interrupted,
            TranscriptColorRole::Notice => self.notice,
            TranscriptColorRole::Warning => self.warning,
            TranscriptColorRole::Muted => self.muted,
            TranscriptColorRole::Accent => self.accent,
            TranscriptColorRole::Loading => self.loading,
            TranscriptColorRole::ToolInput => self.tool_input,
            TranscriptColorRole::SubagentActive => self.subagent_active,
            TranscriptColorRole::InlineHighlight => self.inline_highlight,
            TranscriptColorRole::McpIdentifier => self.mcp_identifier,
        }
    }
}

static TRANSCRIPT_PALETTE: OnceLock<TranscriptPalette> = OnceLock::new();

pub(crate) fn transcript_palette() -> TranscriptPalette {
    *TRANSCRIPT_PALETTE.get_or_init(transcript_palette_from_env)
}

pub(crate) fn transcript_palette_for_depth(depth: TerminalPaletteDepth) -> TranscriptPalette {
    match depth {
        TerminalPaletteDepth::NoColor => TranscriptPalette::NO_COLOR,
        TerminalPaletteDepth::Ansi16 => TranscriptPalette::ANSI16,
        TerminalPaletteDepth::Ansi256 => TranscriptPalette::ANSI256,
        TerminalPaletteDepth::TrueColor => TranscriptPalette::TRUE_COLOR,
    }
}

const fn soften_rgb((red, green, blue): (u8, u8, u8)) -> Color {
    let luma = (red as u16 * RGB_LUMA_WEIGHTS.0
        + green as u16 * RGB_LUMA_WEIGHTS.1
        + blue as u16 * RGB_LUMA_WEIGHTS.2)
        / RGB_LUMA_SCALE;
    Color::Rgb(
        soften_rgb_channel(red, luma),
        soften_rgb_channel(green, luma),
        soften_rgb_channel(blue, luma),
    )
}

const fn soften_rgb_channel(channel: u8, luma: u16) -> u8 {
    let softened = channel as u16 * ACCENT_MINT_SATURATION_PERCENT
        + luma * (PERCENT_SCALE - ACCENT_MINT_SATURATION_PERCENT)
        + PERCENT_ROUNDING_OFFSET;
    // 原通道与 luma 的凸组合始终位于 u8 范围内。
    (softened / PERCENT_SCALE) as u8
}

fn transcript_palette_from_env() -> TranscriptPalette {
    transcript_palette_for_depth(terminal_palette_depth())
}

pub(crate) fn terminal_palette_depth() -> TerminalPaletteDepth {
    if std::env::var_os("NO_COLOR").is_some() {
        return TerminalPaletteDepth::NoColor;
    }
    let colorterm = std::env::var("COLORTERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return TerminalPaletteDepth::TrueColor;
    }
    let term = std::env::var("TERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if term.contains("256color") {
        TerminalPaletteDepth::Ansi256
    } else {
        TerminalPaletteDepth::Ansi16
    }
}
