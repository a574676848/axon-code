//! Agent TUI stable/tail 双区流式控制器。
//!
//! 本模块只承载流式核心状态和调度策略；生产状态容器负责持有 assistant/plan
//! 两条 controller，并接入 transcript consolidation。

use std::collections::VecDeque;
use std::time::Instant;

use ratatui::text::Line;

const STREAM_QUEUED_LINE_CAPACITY: usize = 256;

pub(crate) mod chunking;
pub(crate) mod commit_tick;
pub(crate) mod controller;
mod streaming_markdown_boundary;
mod streaming_markdown_inline_boundary;
pub(crate) mod table_holdback;

#[derive(Debug, Clone)]
struct QueuedStreamLine {
    line: Line<'static>,
    queued_at: Instant,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct StreamSourceCollector {
    pending: String,
}

impl StreamSourceCollector {
    fn push_delta(&mut self, delta: &str) {
        self.pending.push_str(delta);
    }

    fn take_complete_source(&mut self) -> Option<String> {
        let newline = self.pending.rfind('\n')?;
        let stable = self.pending[..=newline].to_owned();
        self.pending = self.pending[newline + 1..].to_owned();
        (!stable.is_empty()).then_some(stable)
    }

    fn pending_source(&self) -> &str {
        &self.pending
    }

    fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct StreamState {
    collector: StreamSourceCollector,
    queued_lines: VecDeque<QueuedStreamLine>,
    has_seen_delta: bool,
}

impl StreamState {
    fn push_delta(&mut self, delta: &str) {
        if !delta.is_empty() {
            self.has_seen_delta = true;
        }
        self.collector.push_delta(delta);
    }

    fn enqueue_lines(&mut self, lines: impl IntoIterator<Item = Line<'static>>, now: Instant) {
        self.queued_lines
            .extend(lines.into_iter().map(|line| QueuedStreamLine {
                line,
                queued_at: now,
            }));
        while self.queued_lines.len() > STREAM_QUEUED_LINE_CAPACITY {
            let _ = self.queued_lines.pop_front();
        }
    }

    fn drain_one(&mut self) -> Vec<Line<'static>> {
        self.queued_lines
            .pop_front()
            .map(|line| vec![line.line])
            .unwrap_or_default()
    }

    fn drain_batch(&mut self, max_lines: usize) -> Vec<Line<'static>> {
        let mut lines = Vec::with_capacity(max_lines.min(self.queued_lines.len()));
        for _ in 0..max_lines {
            let Some(line) = self.queued_lines.pop_front() else {
                break;
            };
            lines.push(line.line);
        }
        lines
    }

    fn queued_len(&self) -> usize {
        self.queued_lines.len()
    }

    fn oldest_queued_age(&self, now: Instant) -> Option<std::time::Duration> {
        self.queued_lines
            .front()
            .map(|line| now.saturating_duration_since(line.queued_at))
    }

    fn clear(&mut self) {
        self.collector = StreamSourceCollector::default();
        self.queued_lines.clear();
        self.has_seen_delta = false;
    }
}
