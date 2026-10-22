//! TUI 动画节奏与 Axon logo 呼吸动效的单点归属。

use std::time::Duration;

pub(crate) const AXON_ACTIVITY_LABEL: &str = "⚡ Axon";
const AXON_LOGO_BREATH_PHASES: usize = 4;
pub(crate) const TERMINAL_TITLE_SPINNER_FRAMES: [&str; 10] =
    ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MotionMode {
    Full,
    Reduced,
}

impl MotionMode {
    pub(crate) fn from_env() -> Self {
        if std::env::var_os("AXON_REDUCED_MOTION").is_some()
            || std::env::var_os("NO_MOTION").is_some()
        {
            Self::Reduced
        } else {
            Self::Full
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptMotionTokens {
    pub(crate) mode: MotionMode,
    pub(crate) spinner: Duration,
    pub(crate) terminal_title: Duration,
    pub(crate) stream_cursor: Duration,
    pub(crate) dock: Duration,
    pub(crate) cell_mutation: Duration,
    pub(crate) jump_bottom: Duration,
    pub(crate) cold_bottom_mount: Duration,
}

impl TranscriptMotionTokens {
    pub(crate) const FULL: Self = Self {
        mode: MotionMode::Full,
        spinner: Duration::from_millis(80),
        terminal_title: Duration::from_millis(120),
        stream_cursor: Duration::from_millis(160),
        dock: Duration::from_millis(40),
        cell_mutation: Duration::from_millis(140),
        jump_bottom: Duration::from_millis(120),
        cold_bottom_mount: Duration::from_millis(160),
    };

    pub(crate) const REDUCED: Self = Self {
        mode: MotionMode::Reduced,
        spinner: Duration::from_millis(160),
        terminal_title: Duration::from_millis(160),
        stream_cursor: Duration::from_millis(160),
        dock: Duration::from_millis(0),
        cell_mutation: Duration::from_millis(0),
        jump_bottom: Duration::from_millis(0),
        cold_bottom_mount: Duration::from_millis(0),
    };

    pub(crate) fn intervals(self) -> [Duration; 7] {
        [
            self.spinner,
            self.terminal_title,
            self.stream_cursor,
            self.dock,
            self.cell_mutation,
            self.jump_bottom,
            self.cold_bottom_mount,
        ]
    }

    pub(crate) fn stays_within_tui_budget(self) -> bool {
        self.intervals()
            .into_iter()
            .all(|interval| interval.is_zero() || interval >= Duration::from_millis(40))
            && self
                .intervals()
                .into_iter()
                .all(|interval| interval <= Duration::from_millis(160))
    }
}

pub(crate) fn transcript_motion_tokens() -> TranscriptMotionTokens {
    match MotionMode::from_env() {
        MotionMode::Full => TranscriptMotionTokens::FULL,
        MotionMode::Reduced => TranscriptMotionTokens::REDUCED,
    }
}

pub(crate) const fn axon_logo_animation_frame_count() -> usize {
    AXON_LOGO_BREATH_PHASES
}

pub(crate) const fn axon_logo_breath_highlight(index: usize) -> bool {
    index % AXON_LOGO_BREATH_PHASES < AXON_LOGO_BREATH_PHASES / 2
}

pub(crate) fn spinner_frame(_index: usize) -> &'static str {
    AXON_ACTIVITY_LABEL
}

pub(crate) fn terminal_title_spinner_frame(index: usize) -> &'static str {
    TERMINAL_TITLE_SPINNER_FRAMES[index % TERMINAL_TITLE_SPINNER_FRAMES.len()]
}
