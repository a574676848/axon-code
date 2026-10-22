//! RenderViewport 滚动状态的单点归属。

use crate::r#loop::agent_tui::output_dock::ToolOutputDock;
use crate::surface::viewport_anchor::ScrollAnchor;

const TRANSCRIPT_VIEW_JUMP_THRESHOLD_MIN_ROWS: usize = 40;
const COLD_BOTTOM_MOUNT_TAIL_ROWS: usize = 10;

/// RenderViewport 与 Dock 视口状态。
#[derive(Debug, Clone)]
pub(crate) struct ViewportState {
    pub(crate) frame_size: ratatui::layout::Size,
    pub(crate) cold_bottom_mount: ColdBottomMount,
    pub(crate) transcript_view_offset_rows: usize,
    pub(crate) max_transcript_view_offset_rows: usize,
    pub(crate) transcript_view_page_rows: usize,
    pub(crate) transcript_view_scroll_state: ViewportScrollState,
    pub(crate) transcript_view_top_anchor: Option<ScrollAnchor>,
    selected_runtime: Option<RuntimeSelection>,
    pub(crate) tool_output_dock: ToolOutputDock,
    pub(crate) transcript_render_cache: crate::widgets::TranscriptRenderCache,
    pub(crate) transcript_height_cache: crate::surface::height_index::TranscriptViewportHeightCache,
}

impl ViewportState {
    pub(crate) fn new() -> Self {
        Self {
            frame_size: ratatui::layout::Size::new(80, 24),
            cold_bottom_mount: ColdBottomMount::default(),
            transcript_view_offset_rows: 0,
            max_transcript_view_offset_rows: 0,
            transcript_view_page_rows: 1,
            transcript_view_scroll_state: ViewportScrollState::BottomAttached,
            transcript_view_top_anchor: None,
            selected_runtime: None,
            tool_output_dock: ToolOutputDock::default(),
            transcript_render_cache: crate::widgets::TranscriptRenderCache::default(),
            transcript_height_cache:
                crate::surface::height_index::TranscriptViewportHeightCache::default(),
        }
    }

    pub(crate) fn handoff_state(&self) -> ViewportHandoffState {
        ViewportHandoffState {
            transcript_view_offset_rows: self.transcript_view_offset_rows,
            max_transcript_view_offset_rows: self.max_transcript_view_offset_rows,
            transcript_view_page_rows: self.transcript_view_page_rows,
            transcript_view_scroll_state: self.transcript_view_scroll_state,
            transcript_view_top_anchor: self.transcript_view_top_anchor,
            selected_runtime: self.selected_runtime.clone(),
        }
    }

    pub(crate) fn restore_handoff_state(&mut self, state: ViewportHandoffState) {
        self.transcript_view_offset_rows = state.transcript_view_offset_rows;
        self.max_transcript_view_offset_rows = state.max_transcript_view_offset_rows;
        self.transcript_view_page_rows = state.transcript_view_page_rows;
        self.transcript_view_scroll_state = state.transcript_view_scroll_state;
        self.transcript_view_top_anchor = state.transcript_view_top_anchor;
        self.selected_runtime = state.selected_runtime;
        self.tool_output_dock = ToolOutputDock::default();
    }

    pub(crate) fn clear_runtime_selection(&mut self) {
        self.selected_runtime = None;
    }

    pub(crate) fn select_runtime_part(&mut self, cell_index: usize, part_id: String) {
        self.selected_runtime = Some(RuntimeSelection::new(cell_index, part_id));
    }

    pub(crate) fn selected_runtime_render_index(&self) -> Option<usize> {
        self.selected_runtime
            .as_ref()
            .map(RuntimeSelection::cell_index)
    }

    pub(crate) fn selected_runtime_part_id(&self) -> Option<&str> {
        self.selected_runtime
            .as_ref()
            .map(RuntimeSelection::part_id)
    }

    pub(crate) fn selected_runtime_selection(&self) -> Option<&RuntimeSelection> {
        self.selected_runtime.as_ref()
    }

    pub(crate) fn rebase_runtime_selection_after_remove(&mut self, removed_index: usize) {
        let Some(selection) = self.selected_runtime.as_mut() else {
            return;
        };
        let Some(next_index) = shift_index_after_remove(selection.cell_index, removed_index) else {
            self.clear_runtime_selection();
            return;
        };
        selection.cell_index = next_index;
    }

    pub(crate) fn clear_handoff_state(&mut self) {
        self.cold_bottom_mount = ColdBottomMount::default();
        self.transcript_view_offset_rows = 0;
        self.max_transcript_view_offset_rows = 0;
        self.transcript_view_page_rows = 1;
        self.transcript_view_scroll_state = ViewportScrollState::BottomAttached;
        self.transcript_view_top_anchor = None;
        self.clear_runtime_selection();
        self.tool_output_dock = ToolOutputDock::default();
    }

    pub(crate) fn reset_transcript_view_to_bottom(&mut self) {
        self.attach_transcript_view_bottom();
    }

    pub(crate) fn detach_transcript_view_at_offset(&mut self, offset_rows: usize) {
        self.transcript_view_offset_rows = offset_rows.min(self.max_transcript_view_offset_rows);
        if self.transcript_view_offset_rows == 0 {
            self.attach_transcript_view_bottom();
        } else {
            self.transcript_view_top_anchor = None;
            self.transcript_view_scroll_state = self.detached_transcript_view_scroll_state();
        }
    }

    pub(crate) fn attach_transcript_view_bottom(&mut self) {
        self.transcript_view_offset_rows = 0;
        self.transcript_view_scroll_state = ViewportScrollState::BottomAttached;
        self.transcript_view_top_anchor = None;
    }

    pub(crate) fn transcript_view_jump_affordance(&self) -> TranscriptViewJumpAffordance {
        let threshold_rows = transcript_view_jump_threshold_rows(self.transcript_view_page_rows);
        let distance_rows = self.transcript_view_offset_rows;
        let overflow = self.max_transcript_view_offset_rows > 0;
        TranscriptViewJumpAffordance {
            available: overflow
                && self.transcript_view_scroll_state == ViewportScrollState::JumpAvailable,
            overflow,
            distance_rows,
            threshold_rows,
        }
    }

    pub(crate) fn restore_transcript_view_bounds(
        &mut self,
        cause: TranscriptViewBoundsCause,
        max_offset_rows: usize,
        page_rows: u16,
        restored_anchor_row: Option<usize>,
        next_top_anchor: Option<ScrollAnchor>,
    ) {
        let previous_max = self.max_transcript_view_offset_rows;
        self.max_transcript_view_offset_rows = max_offset_rows;
        self.transcript_view_page_rows = usize::from(page_rows.max(1));
        if self.transcript_view_scroll_state.follows_tail() {
            self.attach_transcript_view_bottom();
            self.transcript_view_top_anchor = next_top_anchor;
            return;
        }
        if let Some(restored_anchor_row) = restored_anchor_row {
            self.transcript_view_offset_rows = max_offset_rows
                .saturating_sub(restored_anchor_row)
                .min(max_offset_rows);
        } else if cause == TranscriptViewBoundsCause::ContentAppended {
            let growth = max_offset_rows.saturating_sub(previous_max);
            self.transcript_view_offset_rows = self
                .transcript_view_offset_rows
                .saturating_add(growth)
                .min(max_offset_rows);
        } else {
            self.transcript_view_offset_rows =
                self.transcript_view_offset_rows.min(max_offset_rows);
        }
        self.normalize_transcript_view_detached_bounds();
        self.transcript_view_top_anchor = next_top_anchor;
    }

    #[cfg(test)]
    pub(crate) fn active_runtime_projection_start(&self, runtime_line_count: usize) -> usize {
        self.cold_bottom_mount.projection_start(runtime_line_count)
    }

    pub(crate) fn prepare_cold_bottom_mount(&mut self, runtime_line_count: usize) -> bool {
        self.cold_bottom_mount.prepare(runtime_line_count)
    }

    pub(crate) fn complete_cold_bottom_mount_frame(&mut self) -> bool {
        self.cold_bottom_mount.complete_frame()
    }

    pub(crate) const fn cold_bottom_mount_phase(&self) -> ColdBottomMountPhase {
        self.cold_bottom_mount.phase()
    }

    fn detached_transcript_view_scroll_state(&self) -> ViewportScrollState {
        if self.max_transcript_view_offset_rows > 0
            && self.transcript_view_offset_rows
                > transcript_view_jump_threshold_rows(self.transcript_view_page_rows)
        {
            ViewportScrollState::JumpAvailable
        } else {
            ViewportScrollState::UserDetached
        }
    }

    fn normalize_transcript_view_detached_bounds(&mut self) {
        if self.transcript_view_offset_rows == 0 {
            self.attach_transcript_view_bottom();
        } else {
            self.transcript_view_scroll_state = self.detached_transcript_view_scroll_state();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptViewBoundsCause {
    ContentAppended,
    ViewportReflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColdBottomMountPhase {
    NotMounted,
    TailOnly,
    Full,
}

impl ColdBottomMountPhase {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NotMounted => "notMounted",
            Self::TailOnly => "tailOnly",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ColdBottomMount {
    phase: ColdBottomMountPhase,
}

impl ColdBottomMount {
    const fn phase(self) -> ColdBottomMountPhase {
        self.phase
    }

    fn prepare(&mut self, runtime_line_count: usize) -> bool {
        if self.phase != ColdBottomMountPhase::NotMounted {
            return false;
        }
        if runtime_line_count <= COLD_BOTTOM_MOUNT_TAIL_ROWS {
            return false;
        }
        self.phase = ColdBottomMountPhase::TailOnly;
        true
    }

    #[cfg(test)]
    fn projection_start(self, runtime_line_count: usize) -> usize {
        if self.phase == ColdBottomMountPhase::TailOnly {
            runtime_line_count.saturating_sub(COLD_BOTTOM_MOUNT_TAIL_ROWS)
        } else {
            0
        }
    }

    fn complete_frame(&mut self) -> bool {
        if self.phase == ColdBottomMountPhase::Full {
            return false;
        }
        self.phase = ColdBottomMountPhase::Full;
        true
    }
}

impl Default for ColdBottomMount {
    fn default() -> Self {
        Self {
            phase: ColdBottomMountPhase::NotMounted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptViewJumpAffordance {
    pub(crate) available: bool,
    pub(crate) overflow: bool,
    pub(crate) distance_rows: usize,
    pub(crate) threshold_rows: usize,
}

impl Default for TranscriptViewJumpAffordance {
    fn default() -> Self {
        Self {
            available: false,
            overflow: false,
            distance_rows: 0,
            threshold_rows: TRANSCRIPT_VIEW_JUMP_THRESHOLD_MIN_ROWS,
        }
    }
}

/// Session 切换时可安全恢复的 viewport 瞬态状态。
#[derive(Debug, Clone)]
pub(crate) struct ViewportHandoffState {
    transcript_view_offset_rows: usize,
    max_transcript_view_offset_rows: usize,
    transcript_view_page_rows: usize,
    transcript_view_scroll_state: ViewportScrollState,
    transcript_view_top_anchor: Option<ScrollAnchor>,
    selected_runtime: Option<RuntimeSelection>,
}

impl Default for ViewportHandoffState {
    fn default() -> Self {
        Self {
            transcript_view_offset_rows: 0,
            max_transcript_view_offset_rows: 0,
            transcript_view_page_rows: 1,
            transcript_view_scroll_state: ViewportScrollState::BottomAttached,
            transcript_view_top_anchor: None,
            selected_runtime: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeSelection {
    cell_index: usize,
    part_id: String,
}

impl RuntimeSelection {
    fn new(cell_index: usize, part_id: String) -> Self {
        Self {
            cell_index,
            part_id,
        }
    }

    pub(crate) const fn cell_index(&self) -> usize {
        self.cell_index
    }

    pub(crate) fn part_id(&self) -> &str {
        &self.part_id
    }
}

/// RenderViewport 的滚动归属状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ViewportScrollState {
    /// 贴住底部，streaming 内容自动跟随。
    #[default]
    BottomAttached,
    /// 用户主动离开底部，历史加载和 streaming 不抢焦点。
    UserDetached,
    /// 用户已离开底部且距离超过阈值，应展示跳到底部 affordance。
    JumpAvailable,
}

impl ViewportScrollState {
    /// 当前状态是否应该跟随最新 tail。
    #[must_use]
    pub const fn follows_tail(self) -> bool {
        matches!(self, Self::BottomAttached)
    }

    /// 稳定调试/遥测名称。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BottomAttached => "bottomAttached",
            Self::UserDetached => "userDetached",
            Self::JumpAvailable => "jumpAvailable",
        }
    }
}

fn transcript_view_jump_threshold_rows(viewport_rows: usize) -> usize {
    TRANSCRIPT_VIEW_JUMP_THRESHOLD_MIN_ROWS.max(viewport_rows.max(1))
}

fn shift_index_after_remove(index: usize, removed_index: usize) -> Option<usize> {
    if index == removed_index {
        None
    } else if index > removed_index {
        Some(index - 1)
    } else {
        Some(index)
    }
}
