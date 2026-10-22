//! ComposerDock 产品级状态聚合模型。

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use crate::r#loop::agent_tui::{
    bottom_view::BottomView, commands::AgentTuiCommandSuggestion, composer_popup::ComposerPopupItem,
};
use crate::surface::subagent_activity::SubagentActivitySnapshot;

pub(crate) const COMPOSER_DOCK_SEPARATOR_HEIGHT: u16 = 1;
pub(crate) const COMPOSER_DOCK_MIN_INPUT_HEIGHT: u16 = 3;
pub(crate) const COMPOSER_DOCK_WORKING_STATUS_TOP_PADDING: u16 = 1;
pub(crate) const COMPOSER_DOCK_WORKING_STATUS_HEIGHT: u16 =
    COMPOSER_DOCK_WORKING_STATUS_TOP_PADDING.saturating_add(1);
pub(crate) const DOCK_REGISTRY: [DockKind; 7] = [
    DockKind::Question,
    DockKind::Followup,
    DockKind::Revert,
    DockKind::Skills,
    DockKind::ExternalEditor,
    DockKind::ToolOutputViewer,
    DockKind::ShortcutHelp,
];

const DOCK_PROGRESS_CLOSED: u16 = 0;
const DOCK_PROGRESS_OPEN: u16 = 100;
const WORKING_STATUS_REPLY_LABEL: &str = "生成回复中";
const WORKING_STATUS_INTERRUPT_LABEL: &str = "按 ESC 中断";
const WORKING_STATUS_QUEUE_STEER_LABEL: &str = "已有待发送消息 · 按 ESC 立即发送";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockState {
    Hidden,
    Opening,
    Open,
    Closing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockAnimation {
    progress: u16,
    started_at: Option<Instant>,
}

impl DockAnimation {
    pub(crate) const fn closed() -> Self {
        Self {
            progress: DOCK_PROGRESS_CLOSED,
            started_at: None,
        }
    }

    pub(crate) const fn open() -> Self {
        Self {
            progress: DOCK_PROGRESS_OPEN,
            started_at: None,
        }
    }

    pub(crate) fn opening(now: Instant) -> Self {
        Self {
            progress: DOCK_PROGRESS_CLOSED,
            started_at: Some(now),
        }
    }

    pub(crate) fn closing(now: Instant) -> Self {
        Self {
            progress: DOCK_PROGRESS_OPEN,
            started_at: Some(now),
        }
    }

    fn advance_opening(&mut self, now: Instant, duration: std::time::Duration) -> bool {
        let progress = eased_progress(self.started_at, now, duration);
        let changed = self.progress != progress;
        self.progress = progress;
        changed
    }

    fn advance_closing(&mut self, now: Instant, duration: std::time::Duration) -> bool {
        let progress =
            DOCK_PROGRESS_OPEN.saturating_sub(eased_progress(self.started_at, now, duration));
        let changed = self.progress != progress;
        self.progress = progress;
        changed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DockLifecycle {
    state: DockState,
    animation: DockAnimation,
    desired_height: u16,
}

impl DockLifecycle {
    pub(crate) const fn hidden() -> Self {
        Self {
            state: DockState::Hidden,
            animation: DockAnimation::closed(),
            desired_height: 0,
        }
    }

    pub(crate) fn opening(desired_height: u16) -> Self {
        Self::opening_at(desired_height, Instant::now())
    }

    pub(crate) fn opening_at(desired_height: u16, now: Instant) -> Self {
        Self {
            state: DockState::Opening,
            animation: DockAnimation::opening(now),
            desired_height,
        }
    }

    pub(crate) fn open(desired_height: u16) -> Self {
        Self {
            state: DockState::Open,
            animation: DockAnimation::open(),
            desired_height,
        }
    }

    pub(crate) fn closing_at(desired_height: u16, now: Instant) -> Self {
        Self {
            state: DockState::Closing,
            animation: DockAnimation::closing(now),
            desired_height,
        }
    }

    pub(crate) const fn state(self) -> DockState {
        self.state
    }

    pub(crate) const fn visible_height(self) -> u16 {
        match self.state {
            DockState::Hidden => 0,
            DockState::Opening | DockState::Open | DockState::Closing => self
                .desired_height
                .saturating_mul(self.animation.progress)
                .saturating_div(DOCK_PROGRESS_OPEN),
        }
    }

    pub(crate) fn request_close_at(&mut self, now: Instant) -> DockCloseResult {
        match self.state {
            DockState::Hidden => DockCloseResult::NoDock,
            DockState::Closing => DockCloseResult::AlreadyClosing,
            DockState::Opening | DockState::Open => {
                *self = Self::closing_at(self.desired_height, now);
                DockCloseResult::StartedClosing
            }
        }
    }

    pub(crate) fn advance(&mut self, now: Instant, duration: std::time::Duration) -> bool {
        match self.state {
            DockState::Opening => {
                let changed = self.animation.advance_opening(now, duration);
                if self.animation.progress >= DOCK_PROGRESS_OPEN {
                    *self = Self::open(self.desired_height);
                    return true;
                }
                changed
            }
            DockState::Closing => {
                let changed = self.animation.advance_closing(now, duration);
                if self.animation.progress == DOCK_PROGRESS_CLOSED {
                    *self = Self::hidden();
                    return true;
                }
                changed
            }
            DockState::Hidden | DockState::Open => false,
        }
    }
}

fn eased_progress(started_at: Option<Instant>, now: Instant, duration: std::time::Duration) -> u16 {
    if duration.is_zero() {
        return DOCK_PROGRESS_OPEN;
    }
    let elapsed = started_at.map_or(duration, |started_at| {
        now.saturating_duration_since(started_at)
    });
    let elapsed_ms = elapsed.as_millis().min(duration.as_millis());
    let duration_ms = duration.as_millis().max(1);
    let progress = elapsed_ms.saturating_mul(u128::from(DOCK_PROGRESS_OPEN)) / duration_ms;
    smoothstep_percent(u16::try_from(progress).unwrap_or(DOCK_PROGRESS_OPEN))
}

fn smoothstep_percent(progress: u16) -> u16 {
    let progress = u32::from(progress.min(DOCK_PROGRESS_OPEN));
    let eased = progress
        .saturating_mul(progress)
        .saturating_mul(300u32.saturating_sub(progress.saturating_mul(2)));
    u16::try_from(eased / 10_000).unwrap_or(DOCK_PROGRESS_OPEN)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DockCloseResult {
    StartedClosing,
    AlreadyClosing,
    NoDock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DockKind {
    Question,
    Followup,
    Revert,
    Skills,
    ExternalEditor,
    ToolOutputViewer,
    ShortcutHelp,
}

impl DockKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "Question",
            Self::Followup => "Followup",
            Self::Revert => "Revert",
            Self::Skills => "Skills",
            Self::ExternalEditor => "ExternalEditor",
            Self::ToolOutputViewer => "ToolOutputViewer",
            Self::ShortcutHelp => "ShortcutHelp",
        }
    }
}

pub(crate) fn registered_dock_kinds() -> &'static [DockKind] {
    &DOCK_REGISTRY
}

pub(crate) trait Dock {
    fn kind(&self) -> DockKind;

    fn lifecycle(&self) -> DockLifecycle;

    fn visible_height(&self) -> u16 {
        self.lifecycle().visible_height()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerDockDraft<'a> {
    pub(crate) text: &'a str,
    pub(crate) elements: Vec<ComposerDockDraftElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerDockDraftElement {
    pub(crate) range: Range<usize>,
    pub(crate) kind: ComposerDockDraftElementKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComposerDockDraftElementKind {
    Mention(char),
    File,
    Image,
}

pub(crate) enum ComposerDockPanel<'a> {
    BottomView {
        view: &'a BottomView,
        lifecycle: DockLifecycle,
    },
}

impl<'a> ComposerDockPanel<'a> {
    pub(crate) fn bottom_view(view: &'a BottomView, lifecycle: DockLifecycle) -> Self {
        Self::BottomView { view, lifecycle }
    }
}

impl Dock for ComposerDockPanel<'_> {
    fn kind(&self) -> DockKind {
        let kind = match self {
            Self::BottomView { view, .. } => view.kind(),
        };
        debug_assert!(registered_dock_kinds().contains(&kind));
        kind
    }

    fn lifecycle(&self) -> DockLifecycle {
        match self {
            Self::BottomView { lifecycle, .. } => *lifecycle,
        }
    }
}

pub(crate) struct ComposerDockViewModel<'a> {
    pub(crate) draft: ComposerDockDraft<'a>,
    pub(crate) command_suggestions: &'a [AgentTuiCommandSuggestion],
    pub(crate) popup_items: &'a [ComposerPopupItem],
    pub(crate) suggestion_title: &'a str,
    pub(crate) selected_command_suggestion: usize,
    pub(crate) queued_preview: Option<String>,
    pub(crate) working_status: Option<WorkingStatusViewModel>,
    pub(crate) draft_scroll_offset: u16,
    pub(crate) panel: Option<ComposerDockPanel<'a>>,
    pub(crate) subagent_activity: SubagentActivitySnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkingStatusTone {
    Active,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkingStatusViewModel {
    /// 当前 provider 阶段的完整主文案。
    pub(crate) primary: String,
    /// 状态前缀。
    pub(crate) prefix: &'static str,
    /// 视觉语义。
    pub(crate) tone: WorkingStatusTone,
    /// 主文案是否使用低强调扫光动画。
    pub(crate) animated: bool,
    /// 运行中保留在输入区的操作提示，Header 只承担图标化状态入口。
    pub(crate) elapsed: Option<Duration>,
    pub(crate) queued: bool,
}

impl WorkingStatusViewModel {
    pub(crate) fn generating(route: String, elapsed: Option<Duration>, queued: bool) -> Self {
        Self {
            primary: format!("{route} {WORKING_STATUS_REPLY_LABEL}"),
            prefix: "◌ ",
            tone: WorkingStatusTone::Active,
            animated: true,
            elapsed,
            queued,
        }
    }

    pub(crate) fn primary_label(&self) -> &str {
        &self.primary
    }

    pub(crate) fn detail_label(&self) -> String {
        self.details().join(" · ")
    }

    fn details(&self) -> Vec<String> {
        let mut details = Vec::new();
        if let Some(elapsed) = self.elapsed {
            details.push(axon_display::format_duration_millis_compact(
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            ));
        }
        details.push(if self.queued {
            WORKING_STATUS_QUEUE_STEER_LABEL.to_owned()
        } else {
            WORKING_STATUS_INTERRUPT_LABEL.to_owned()
        });
        details
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::subagent_activity::{
        SubagentActivityRow, SubagentActivitySnapshot, SubagentActivityStatus,
    };
    use crate::widgets::DockRenderGateway;

    #[test]
    fn working_status_label_includes_elapsed_and_queue_action() {
        let status = WorkingStatusViewModel::generating(
            "cpa/gpt-5.4".to_owned(),
            Some(std::time::Duration::from_secs(170)),
            true,
        );

        assert_eq!(
            format!("{}（{}）", status.primary_label(), status.detail_label()),
            "cpa/gpt-5.4 生成回复中（2m 50s · 已有待发送消息 · 按 ESC 立即发送）"
        );
        assert_eq!(status.primary_label(), "cpa/gpt-5.4 生成回复中");
        assert_eq!(
            status.detail_label(),
            "2m 50s · 已有待发送消息 · 按 ESC 立即发送"
        );
    }

    #[test]
    fn dock_registry_contains_all_target_dock_kinds_once() {
        let mut kinds = DOCK_REGISTRY.to_vec();
        kinds.sort();
        kinds.dedup();

        assert_eq!(kinds.len(), DOCK_REGISTRY.len());
        assert_eq!(DOCK_REGISTRY.len(), 7);
        assert!(DOCK_REGISTRY.contains(&DockKind::ToolOutputViewer));
        assert!(DOCK_REGISTRY.contains(&DockKind::ShortcutHelp));
    }

    #[test]
    fn dock_lifecycle_hidden_panel_reserves_no_height() {
        assert_eq!(DockLifecycle::hidden().visible_height(), 0);
    }

    #[test]
    fn dock_lifecycle_open_panel_uses_desired_height() {
        let lifecycle = DockLifecycle::open(6);

        assert_eq!(lifecycle.state(), DockState::Open);
        assert_eq!(lifecycle.visible_height(), 6);
    }

    #[test]
    fn dock_lifecycle_opening_advances_with_eased_progress() {
        let started_at = std::time::Instant::now();
        let mut lifecycle = DockLifecycle::opening_at(8, started_at);

        assert_eq!(lifecycle.visible_height(), 0);
        assert!(lifecycle.advance(
            started_at + std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(40)
        ));
        assert_eq!(lifecycle.state(), DockState::Opening);
        assert_eq!(lifecycle.visible_height(), 4);
        assert!(lifecycle.advance(
            started_at + std::time::Duration::from_millis(40),
            std::time::Duration::from_millis(40)
        ));
        assert_eq!(lifecycle.state(), DockState::Open);
        assert_eq!(lifecycle.visible_height(), 8);
    }

    #[test]
    fn dock_lifecycle_closing_advances_to_hidden() {
        let started_at = std::time::Instant::now();
        let mut lifecycle = DockLifecycle::closing_at(8, started_at);

        assert_eq!(lifecycle.visible_height(), 8);
        assert!(lifecycle.advance(
            started_at + std::time::Duration::from_millis(40),
            std::time::Duration::from_millis(40)
        ));
        assert_eq!(lifecycle.state(), DockState::Hidden);
        assert_eq!(lifecycle.visible_height(), 0);
    }

    #[test]
    fn dock_lifecycle_zero_duration_motion_settles_immediately() {
        let now = std::time::Instant::now();
        let mut lifecycle = DockLifecycle::opening_at(8, now);

        assert!(lifecycle.advance(now, std::time::Duration::ZERO));
        assert_eq!(lifecycle.state(), DockState::Open);
        assert_eq!(lifecycle.visible_height(), 8);
    }

    #[test]
    fn dock_lifecycle_repeated_close_is_idempotent() {
        let now = std::time::Instant::now();
        let mut lifecycle = DockLifecycle::open(5);

        assert_eq!(
            lifecycle.request_close_at(now),
            DockCloseResult::StartedClosing
        );
        let closing = lifecycle;
        assert_eq!(
            lifecycle.request_close_at(now + std::time::Duration::from_millis(1)),
            DockCloseResult::AlreadyClosing
        );

        assert_eq!(lifecycle, closing);
        assert_eq!(lifecycle.state(), DockState::Closing);
    }

    #[test]
    fn subagent_activity_height_reserves_separator_above_rows() {
        let view_model = ComposerDockViewModel {
            draft: ComposerDockDraft {
                text: "",
                elements: Vec::new(),
            },
            command_suggestions: &[],
            popup_items: &[],
            suggestion_title: "",
            selected_command_suggestion: 0,
            queued_preview: None,
            working_status: None,
            draft_scroll_offset: 0,
            panel: None,
            subagent_activity: SubagentActivitySnapshot {
                rows: vec![SubagentActivityRow {
                    agent_id: "subagent:/root".to_owned(),
                    label: "/root".to_owned(),
                    status: SubagentActivityStatus::Running,
                    activity: "生成中".to_owned(),
                    elapsed: std::time::Duration::from_secs(1),
                    usage: None,
                    cache_hit_rate_basis_points: None,
                    model: None,
                    summary: None,
                }],
                max_visible_rows: 4,
                animation_frame: 0,
            },
        };

        let area = ratatui::layout::Rect::new(0, 0, 80, 8);
        assert_eq!(
            DockRenderGateway::partition(&view_model, area)
                .input()
                .height,
            5
        );
    }
}
