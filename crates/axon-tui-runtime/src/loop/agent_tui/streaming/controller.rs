//! Assistant 和 plan 流式控制器。

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use ratatui::text::Line;

use axon_memory::project_memory_citation_text;

use crate::r#loop::agent_tui::streaming::StreamState;
use crate::r#loop::agent_tui::streaming::streaming_markdown_boundary::{
    IncrementalStreamingMarkdownBoundary, StableSourceUpdate,
};
use crate::r#loop::agent_tui::streaming::table_holdback::{
    TableHoldbackScanner, TableHoldbackState,
};
use crate::surface::cell::{ROLE_AGENT, RenderCellRecord};
#[cfg(test)]
use crate::widgets::render_streaming_markdown_lines;
use crate::widgets::{
    HyperlinkLine, render_message_source_continuation_hyperlink_lines,
    render_message_source_hyperlink_lines, visible_lines,
};

#[derive(Debug, Clone)]
pub(crate) struct StreamFinalize {
    pub(crate) cell: Option<RenderCellRecord>,
}

#[derive(Debug, Clone)]
struct StreamCore {
    state: StreamState,
    width: u16,
    part_id: String,
    raw_source: String,
    raw_input_source: String,
    rendered_lines: Vec<HyperlinkLine>,
    active_layout_cache: Option<Arc<[HyperlinkLine]>>,
    active_projection_dirty: bool,
    active_projection_source_len: usize,
    enqueued_rendered_len: usize,
    emitted_rendered_len: usize,
    stable_boundary: IncrementalStreamingMarkdownBoundary,
    tail_boundary: IncrementalStreamingMarkdownBoundary,
    table_holdback: TableHoldbackScanner,
}

impl StreamCore {
    fn new(width: u16, part_id: impl Into<String>) -> Self {
        Self {
            state: StreamState::default(),
            width: width.max(1),
            part_id: part_id.into(),
            raw_source: String::new(),
            raw_input_source: String::new(),
            rendered_lines: Vec::new(),
            active_layout_cache: None,
            active_projection_dirty: false,
            active_projection_source_len: 0,
            enqueued_rendered_len: 0,
            emitted_rendered_len: 0,
            stable_boundary: IncrementalStreamingMarkdownBoundary::default(),
            tail_boundary: IncrementalStreamingMarkdownBoundary::default(),
            table_holdback: TableHoldbackScanner::new(),
        }
    }

    fn push_delta(&mut self, delta: &str, now: Instant) {
        let mut previous_visible_source = self.raw_source.clone();
        previous_visible_source.push_str(self.state.collector.pending_source());
        self.raw_input_source.push_str(delta);
        let projected_source = project_memory_citation_text(&self.raw_input_source);
        let visible_delta = projected_source
            .strip_prefix(&previous_visible_source)
            .unwrap_or(projected_source.as_str());
        if visible_delta.is_empty() {
            self.mark_active_projection_dirty();
            return;
        }
        self.push_visible_delta(visible_delta, now);
    }

    fn push_visible_delta(&mut self, delta: &str, now: Instant) {
        self.state.push_delta(delta);
        self.active_layout_cache = None;
        if let Some(stable_source) = self.state.collector.take_complete_source() {
            let previous_stable_source_len = self.stable_source_len();
            self.raw_source.push_str(&stable_source);
            self.table_holdback.push_source_chunk(&stable_source);
            let update = self
                .stable_boundary
                .append_source(&self.raw_source, &stable_source);
            self.recompute_rendered_lines(update, previous_stable_source_len);
            self.enqueue_stable_lines(now);
            self.rebuild_tail_boundary();
        } else {
            let pending = self.state.collector.pending_source();
            self.tail_boundary.replace_uncommitted_source(pending);
        }
        self.mark_active_projection_dirty();
    }

    fn set_width(&mut self, width: u16, now: Instant) {
        let width = width.max(1);
        if self.width == width {
            return;
        }
        self.width = width;
        self.active_layout_cache = None;
        self.active_projection_source_len = 0;
        self.rebuild_rendered_lines();
        self.state.queued_lines.clear();
        self.enqueued_rendered_len = self.emitted_rendered_len.min(self.rendered_lines.len());
        self.enqueue_stable_lines(now);
    }

    fn finalize(&mut self) -> StreamFinalize {
        let source = project_memory_citation_text(&self.raw_input_source);
        self.reset();
        if source.is_empty() {
            return StreamFinalize { cell: None };
        }
        StreamFinalize {
            cell: Some(RenderCellRecord::agent_message(
                self.part_id.clone(),
                source,
            )),
        }
    }

    fn on_commit_tick(&mut self) -> Option<RenderCellRecord> {
        let lines = self.state.drain_one();
        self.drain_lines(lines)
    }

    fn on_commit_tick_batch(&mut self, max_lines: usize) -> Option<RenderCellRecord> {
        let lines = self.state.drain_batch(max_lines);
        self.drain_lines(lines)
    }

    fn on_commit_tick_cells(&mut self) -> Vec<RenderCellRecord> {
        let lines = self.state.drain_one();
        self.drain_lines_as_cells(lines)
    }

    fn on_commit_tick_batch_cells(&mut self, max_lines: usize) -> Vec<RenderCellRecord> {
        let lines = self.state.drain_batch(max_lines);
        self.drain_lines_as_cells(lines)
    }

    fn has_live_tail(&self) -> bool {
        self.has_held_back_block()
            || !self.current_tail_source().is_empty()
            || !self.state.collector.is_empty()
    }

    fn queued_lines(&self) -> usize {
        self.state.queued_len()
    }

    fn has_pending_projection(&self) -> bool {
        self.state.queued_len() > 0 || self.active_projection_dirty
    }

    fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.state.oldest_queued_age(now)
    }

    #[cfg(test)]
    fn current_tail_lines(&self) -> Vec<Line<'static>> {
        visible_lines(&self.current_tail_hyperlink_lines())
    }

    fn current_tail_hyperlink_lines(&self) -> Vec<HyperlinkLine> {
        render_message_source_hyperlink_lines(
            &self.current_tail_source(),
            ROLE_AGENT,
            false,
            self.width,
        )
    }

    fn current_tail_source(&self) -> String {
        let pending = self.state.collector.pending_source();
        let visible_len = self.current_tail_source_len();
        pending[..visible_len].to_owned()
    }

    fn current_tail_source_len(&self) -> usize {
        if self.stable_source_len() < self.raw_source.len() {
            return 0;
        }
        self.tail_boundary.stable_source_len()
    }

    fn current_active_source_len(&self) -> usize {
        if self.table_holdback.is_active() && self.stable_source_len() < self.raw_source.len() {
            return self.raw_source.len();
        }
        let stable_source_len = self.stable_source_len();
        let tail_source_len = if stable_source_len == self.raw_source.len() {
            self.current_tail_source_len()
        } else {
            0
        };
        stable_source_len.saturating_add(tail_source_len)
    }

    fn current_visible_text(&self) -> String {
        let stable_source_len = self.stable_source_len();
        let mut source = self.raw_source[..stable_source_len].to_owned();
        if stable_source_len == self.raw_source.len() {
            source.push_str(&self.current_tail_source());
        }
        source
    }

    fn current_active_source(&self) -> String {
        if self.table_holdback.is_active() && self.stable_source_len() < self.raw_source.len() {
            return self.raw_source.clone();
        }
        self.current_visible_text()
    }

    fn recompute_rendered_lines(
        &mut self,
        update: StableSourceUpdate,
        previous_stable_source_len: usize,
    ) {
        if update.stable_source_len <= previous_stable_source_len {
            return;
        }
        if previous_stable_source_len > 0 && update.can_append_stable_layout {
            self.rendered_lines
                .extend(render_message_source_continuation_hyperlink_lines(
                    &self.raw_source[previous_stable_source_len..update.stable_source_len],
                    ROLE_AGENT,
                    false,
                    self.width,
                ));
        } else {
            self.rebuild_rendered_lines();
        }
    }

    fn rebuild_rendered_lines(&mut self) {
        self.rendered_lines = render_message_source_hyperlink_lines(
            &self.raw_source[..self.stable_source_len()],
            ROLE_AGENT,
            false,
            self.width,
        );
    }

    fn enqueue_stable_lines(&mut self, now: Instant) {
        let stable_line_limit = self.rendered_lines.len();
        if stable_line_limit <= self.enqueued_rendered_len {
            return;
        }
        let lines =
            visible_lines(&self.rendered_lines[self.enqueued_rendered_len..stable_line_limit]);
        self.enqueued_rendered_len = stable_line_limit;
        self.state.enqueue_lines(lines, now);
    }

    fn stable_source_len(&self) -> usize {
        self.stable_boundary.stable_source_len()
    }

    fn has_held_back_block(&self) -> bool {
        self.stable_source_len() < self.raw_source.len()
    }

    fn rebuild_tail_boundary(&mut self) {
        self.tail_boundary.reset();
        let pending = self.state.collector.pending_source();
        if !pending.is_empty() {
            self.tail_boundary.replace_uncommitted_source(pending);
        }
    }

    fn drain_lines(&mut self, lines: Vec<Line<'static>>) -> Option<RenderCellRecord> {
        if lines.is_empty() {
            return None;
        }
        self.emitted_rendered_len = self.emitted_rendered_len.saturating_add(lines.len());
        if self.current_active_source_len() <= self.active_projection_source_len {
            return None;
        }
        self.active_projection_dirty = false;
        let source = self.current_active_source();
        self.active_projection_source_len = source.len();
        Some(RenderCellRecord::streaming_agent_message(
            self.part_id.clone(),
            source,
        ))
    }

    fn take_active_projection_cell(&mut self) -> Option<RenderCellRecord> {
        if !std::mem::take(&mut self.active_projection_dirty) {
            return None;
        }
        let source = self.current_active_source();
        self.active_projection_source_len = source.len();
        (!source.trim().is_empty())
            .then(|| RenderCellRecord::streaming_agent_message(self.part_id.clone(), source))
    }

    fn mark_active_projection_dirty(&mut self) {
        self.active_projection_dirty |=
            self.current_active_source_len() > self.active_projection_source_len;
    }

    fn drain_lines_as_cells(&mut self, lines: Vec<Line<'static>>) -> Vec<RenderCellRecord> {
        self.emitted_rendered_len = self.emitted_rendered_len.saturating_add(lines.len());
        lines
            .into_iter()
            .map(|line| {
                RenderCellRecord::streaming_agent_message(self.part_id.clone(), line_text(&line))
            })
            .collect()
    }

    fn reset(&mut self) {
        self.state.clear();
        self.raw_source.clear();
        self.raw_input_source.clear();
        self.rendered_lines.clear();
        self.enqueued_rendered_len = 0;
        self.emitted_rendered_len = 0;
        self.stable_boundary.reset();
        self.tail_boundary.reset();
        self.active_layout_cache = None;
        self.active_projection_dirty = false;
        self.active_projection_source_len = 0;
        self.table_holdback.reset();
    }

    fn active_hyperlink_lines(&mut self) -> Arc<[HyperlinkLine]> {
        if let Some(lines) = self.active_layout_cache.as_ref() {
            return Arc::clone(lines);
        }
        let lines = if self.table_holdback.is_active()
            && self.stable_source_len() < self.raw_source.len()
        {
            let stable_source_len = self.stable_source_len();
            let mut lines = self.rendered_lines.clone();
            if lines.is_empty() {
                render_message_source_hyperlink_lines(
                    &self.raw_source[stable_source_len..],
                    ROLE_AGENT,
                    false,
                    self.width,
                )
            } else {
                if matches!(
                    self.table_holdback.state(),
                    TableHoldbackState::PendingHeader { .. }
                ) {
                    lines.push(HyperlinkLine::new(Line::default()));
                }
                lines.extend(render_message_source_continuation_hyperlink_lines(
                    &self.raw_source[stable_source_len..],
                    ROLE_AGENT,
                    false,
                    self.width,
                ));
                lines
            }
        } else {
            let mut lines = self.rendered_lines.clone();
            if self.stable_source_len() == self.raw_source.len() {
                lines.extend(self.current_tail_hyperlink_lines());
            }
            lines
        };
        let lines = Arc::from(lines);
        self.active_layout_cache = Some(Arc::clone(&lines));
        lines
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AssistantStreamController {
    core: StreamCore,
}

#[derive(Debug, Clone)]
pub(crate) struct PlanStreamController {
    core: StreamCore,
}

impl AssistantStreamController {
    pub(crate) fn new(message_id: impl Into<String>, width: u16, _cwd: impl AsRef<Path>) -> Self {
        Self {
            core: StreamCore::new(width, message_id),
        }
    }

    pub(crate) fn push_delta(&mut self, delta: &str, now: Instant) {
        self.core.push_delta(delta, now);
    }

    pub(crate) fn set_width(&mut self, width: u16, now: Instant) {
        self.core.set_width(width, now);
    }

    pub(crate) fn finalize(&mut self) -> StreamFinalize {
        self.core.finalize()
    }

    pub(crate) fn on_commit_tick(&mut self) -> Option<RenderCellRecord> {
        self.core.on_commit_tick()
    }

    pub(crate) fn on_commit_tick_batch(&mut self, max_lines: usize) -> Option<RenderCellRecord> {
        self.core.on_commit_tick_batch(max_lines)
    }

    pub(crate) fn has_live_tail(&self) -> bool {
        self.core.has_live_tail()
    }

    pub(crate) fn queued_lines(&self) -> usize {
        self.core.queued_lines()
    }

    pub(crate) fn has_pending_projection(&self) -> bool {
        self.core.has_pending_projection()
    }

    pub(crate) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.core.oldest_queued_age(now)
    }

    #[cfg(test)]
    pub(crate) fn current_tail_lines(&self) -> Vec<Line<'static>> {
        self.core.current_tail_lines()
    }

    #[cfg(test)]
    pub(crate) fn current_visible_text(&self) -> String {
        self.core.current_visible_text()
    }

    #[cfg(test)]
    pub(crate) fn current_active_source(&self) -> String {
        self.core.current_active_source()
    }

    pub(crate) fn current_active_hyperlink_lines(&mut self) -> Arc<[HyperlinkLine]> {
        self.core.active_hyperlink_lines()
    }

    pub(crate) fn width(&self) -> u16 {
        self.core.width
    }

    pub(crate) fn take_active_projection_cell(&mut self) -> Option<RenderCellRecord> {
        self.core.take_active_projection_cell()
    }
}

impl PlanStreamController {
    pub(crate) fn new(width: u16, _cwd: impl AsRef<Path>) -> Self {
        Self {
            core: StreamCore::new(width, "plan"),
        }
    }

    #[cfg(test)]
    pub(crate) fn push_delta(&mut self, delta: &str, now: Instant) {
        self.core.push_delta(delta, now);
    }

    pub(crate) fn set_width(&mut self, width: u16, now: Instant) {
        self.core.set_width(width, now);
    }

    pub(crate) fn on_commit_tick(&mut self) -> Vec<RenderCellRecord> {
        self.core.on_commit_tick_cells()
    }

    pub(crate) fn on_commit_tick_batch(&mut self, max_lines: usize) -> Vec<RenderCellRecord> {
        self.core.on_commit_tick_batch_cells(max_lines)
    }

    pub(crate) fn has_live_tail(&self) -> bool {
        self.core.has_live_tail()
    }

    pub(crate) fn queued_lines(&self) -> usize {
        self.core.queued_lines()
    }

    pub(crate) fn oldest_queued_age(&self, now: Instant) -> Option<Duration> {
        self.core.oldest_queued_age(now)
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[cfg(test)]
fn line_texts(lines: &[Line<'static>]) -> Vec<String> {
    lines.iter().map(line_text).collect()
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
