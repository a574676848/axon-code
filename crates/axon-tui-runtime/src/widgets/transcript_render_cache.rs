//! Transcript 渲染缓存。
//!
//! 目标态参考 Codex 的 HistoryCell 思路：每个 transcript cell 暴露稳定 id 与 revision，
//! 渲染层按 cell_id/revision/width/expanded/transcript_key 取缓存，避免每帧重复 Markdown 解析和换行。

use std::{
    borrow::Cow,
    cell::RefCell,
    collections::{HashMap, HashSet},
    mem::size_of,
    sync::Arc,
    time::Instant,
};

#[cfg(test)]
use ratatui::text::Line;

use crate::{
    surface::{
        cell::RenderCell,
        cell_id::CellId,
        render_cache::{CellCacheKey, ResizeReflowCacheDelta},
        render_policy::RenderPolicy,
        telemetry::{TuiRenderProfilerDelta, duration_micros},
    },
    widgets::{
        history_cell::render_cell_contract_probe,
        terminal_hyperlinks::HyperlinkLine,
        transcript_render_layout::{
            physical_lines_height, render_cell_record, render_record_for_cache,
        },
    },
};

#[cfg(test)]
use crate::widgets::terminal_hyperlinks::visible_lines;

const MAX_CACHE_ENTRIES: usize = 512;
const MAX_CACHE_MEMORY_BYTES: usize = 50 * 1024 * 1024; // 50 MB

#[derive(Debug, Default, Clone)]
pub(crate) struct TranscriptRenderCache {
    entries: RefCell<HashMap<CellCacheKey, CachedCellRender>>,
    access_epoch: RefCell<u64>,
    current_memory_bytes: RefCell<usize>,
    metrics: RefCell<TranscriptRenderCacheMetrics>,
}

#[derive(Debug, Clone)]
struct CachedCellRender {
    lines: Arc<[HyperlinkLine]>,
    height: u16,
    last_access: u64,
}

impl CachedCellRender {
    /// 估算此缓存条目占用的内存字节数。
    ///
    /// 包括：
    /// - 每个 HyperlinkLine、Line、Span 和 hyperlink 容器的已分配容量
    /// - owned Span 与 hyperlink destination 字符串的已分配容量
    /// - Arc 与缓存条目的固定开销
    fn estimated_size_bytes(&self) -> usize {
        let mut size = size_of::<Self>()
            .saturating_add(size_of::<usize>().saturating_mul(2))
            .saturating_add(self.lines.len().saturating_mul(size_of::<HyperlinkLine>()));
        for hyperlink_line in self.lines.iter() {
            size = size.saturating_add(
                hyperlink_line
                    .line
                    .spans
                    .capacity()
                    .saturating_mul(size_of::<ratatui::text::Span<'static>>()),
            );
            for span in &hyperlink_line.line.spans {
                if let Cow::Owned(content) = &span.content {
                    size = size.saturating_add(content.capacity());
                }
            }
            size = size.saturating_add(hyperlink_line.hyperlinks.capacity().saturating_mul(
                size_of::<crate::widgets::terminal_hyperlinks::TerminalHyperlink>(),
            ));
            for link in &hyperlink_line.hyperlinks {
                size = size.saturating_add(link.destination.capacity());
            }
        }
        size
    }
}

/// Transcript 渲染缓存统计增量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TranscriptRenderCacheMetrics {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) current_entries: u64,
    pub(crate) current_memory_bytes: u64,
    pub(crate) evictions_by_count: u64,
    pub(crate) evictions_by_memory: u64,
    pub(crate) max_entry_bytes: u64,
    pub(crate) resize_reflow: ResizeReflowCacheDelta,
    pub(crate) render_profiler: TuiRenderProfilerDelta,
}

impl TranscriptRenderCache {
    #[cfg(test)]
    pub(crate) fn render_cell_lines(
        &self,
        line: &dyn RenderCell,
        expanded: bool,
        width: u16,
    ) -> Vec<Line<'static>> {
        visible_lines(&self.render_cell_hyperlink_lines(line, expanded, width))
    }

    pub(crate) fn render_cell_hyperlink_lines(
        &self,
        line: &dyn RenderCell,
        expanded: bool,
        width: u16,
    ) -> Arc<[HyperlinkLine]> {
        let (key, expanded) = Self::cache_key(line, expanded, width);
        if let Some(cached) = self.cached_entry(&key) {
            return cached.lines;
        }
        self.record_miss();
        let started_at = Instant::now();
        let record = render_cell_record(line);
        let lines = Arc::from(render_record_for_cache(record, expanded, width));
        let height = self.insert_rendered(key, Arc::clone(&lines));
        self.record_render_profiler(started_at.elapsed(), height);
        lines
    }

    pub(crate) fn desired_height(&self, line: &dyn RenderCell, expanded: bool, width: u16) -> u16 {
        let (key, expanded) = Self::cache_key(line, expanded, width);
        if let Some(cached) = self.cached_entry(&key) {
            return cached.height;
        }
        self.record_miss();
        let started_at = Instant::now();
        render_cell_contract_probe(line, width);
        let record = render_cell_record(line);
        let lines = Arc::from(render_record_for_cache(record, expanded, width));
        let height = self.insert_rendered(key, lines);
        self.record_render_profiler(started_at.elapsed(), height);
        height
    }

    /// 注入流式 controller 已完成的唯一 Markdown 布局权威。
    pub(crate) fn prime_streaming_layout(
        &self,
        line: &dyn RenderCell,
        width: u16,
        lines: Arc<[HyperlinkLine]>,
    ) {
        let (key, expanded) = Self::cache_key(line, false, width);
        debug_assert!(!expanded, "streaming message 不应进入 expanded 布局");
        self.remove_stale_streaming_revisions(key);
        self.insert_rendered(key, lines);
    }

    pub(crate) fn compact_for_resize(&self, target_width: u16) -> ResizeReflowCacheDelta {
        let mut entries = self.entries.borrow_mut();
        let mut current_memory = self.current_memory_bytes.borrow_mut();

        let before = entries.len();
        let removed_memory: usize = entries
            .iter()
            .filter(|(key, _)| !key.within_resize_tolerance(target_width))
            .map(|(_, value)| value.estimated_size_bytes())
            .sum();

        entries.retain(|key, _| key.within_resize_tolerance(target_width));
        let retained = entries.len();

        *current_memory = current_memory.saturating_sub(removed_memory);

        let delta = ResizeReflowCacheDelta {
            events: 1,
            retained_entries: u64::try_from(retained).unwrap_or(u64::MAX),
            evicted_entries: u64::try_from(before.saturating_sub(retained)).unwrap_or(u64::MAX),
        };
        self.record_resize_reflow(delta);
        delta
    }

    fn cache_key(line: &dyn RenderCell, expanded: bool, width: u16) -> (CellCacheKey, bool) {
        let decision = RenderPolicy::decide(line);
        let expanded = decision.render_expanded(expanded);
        (CellCacheKey::from_cell(line, expanded, width), expanded)
    }

    fn cached_entry(&self, key: &CellCacheKey) -> Option<CachedCellRender> {
        let access = self.next_access_epoch();
        let mut entries = self.entries.borrow_mut();
        let cached = entries.get_mut(key)?;
        cached.last_access = access;
        self.record_hit();
        Some(cached.clone())
    }

    fn insert_rendered(&self, key: CellCacheKey, lines: Arc<[HyperlinkLine]>) -> u16 {
        let height = physical_lines_height(&lines);
        let cached = CachedCellRender {
            lines,
            height,
            last_access: self.next_access_epoch(),
        };
        let entry_size = cached.estimated_size_bytes();

        if entry_size > MAX_CACHE_MEMORY_BYTES {
            self.remove_entry(&key);
            self.record_max_entry_size(entry_size);
            return height;
        }

        self.remove_entry(&key);
        self.evict_if_needed(entry_size);

        let mut entries = self.entries.borrow_mut();
        entries.insert(key, cached);
        drop(entries);

        let mut current_memory = self.current_memory_bytes.borrow_mut();
        *current_memory = current_memory.saturating_add(entry_size);
        drop(current_memory);

        self.record_max_entry_size(entry_size);

        height
    }

    pub(crate) fn remove_cells(&self, cell_ids: &[CellId]) {
        if cell_ids.is_empty() {
            return;
        }

        let cell_ids = cell_ids.iter().copied().collect::<HashSet<_>>();
        let reclaimed = {
            let mut entries = self.entries.borrow_mut();
            let mut reclaimed = 0usize;
            entries.retain(|key, entry| {
                let removed = cell_ids.contains(&key.cell_id);
                if removed {
                    reclaimed = reclaimed.saturating_add(entry.estimated_size_bytes());
                }
                !removed
            });
            reclaimed
        };
        if reclaimed > 0 {
            let mut current_memory = self.current_memory_bytes.borrow_mut();
            *current_memory = current_memory.saturating_sub(reclaimed);
        }
    }

    fn remove_entry(&self, key: &CellCacheKey) {
        let replaced = self.entries.borrow_mut().remove(key);
        if let Some(replaced) = replaced {
            let mut current_memory = self.current_memory_bytes.borrow_mut();
            *current_memory = current_memory.saturating_sub(replaced.estimated_size_bytes());
        }
    }

    fn remove_stale_streaming_revisions(&self, current: CellCacheKey) {
        let reclaimed = {
            let mut entries = self.entries.borrow_mut();
            let mut reclaimed = 0usize;
            entries.retain(|key, entry| {
                let stale = key.cell_id == current.cell_id && key.revision != current.revision;
                if stale {
                    reclaimed = reclaimed.saturating_add(entry.estimated_size_bytes());
                }
                !stale
            });
            reclaimed
        };
        if reclaimed > 0 {
            let mut current_memory = self.current_memory_bytes.borrow_mut();
            *current_memory = current_memory.saturating_sub(reclaimed);
        }
    }

    fn next_access_epoch(&self) -> u64 {
        let mut access_epoch = self.access_epoch.borrow_mut();
        *access_epoch = access_epoch.wrapping_add(1);
        *access_epoch
    }

    fn evict_if_needed(&self, incoming_size: usize) {
        let mut entries = self.entries.borrow_mut();
        let mut current_memory = self.current_memory_bytes.borrow_mut();

        let needs_eviction_by_count = entries.len().saturating_add(1) > MAX_CACHE_ENTRIES;
        let needs_eviction_by_memory =
            current_memory.saturating_add(incoming_size) > MAX_CACHE_MEMORY_BYTES;

        if !needs_eviction_by_count && !needs_eviction_by_memory {
            return;
        }

        let mut by_access: Vec<_> = entries
            .iter()
            .map(|(key, value)| (*key, value.last_access, value.estimated_size_bytes()))
            .collect();
        by_access.sort_by_key(|(_, last_access, _)| *last_access);

        let target_entries = if needs_eviction_by_count {
            MAX_CACHE_ENTRIES.saturating_mul(3) / 4
        } else {
            entries.len()
        };
        let target_memory = if needs_eviction_by_memory {
            MAX_CACHE_MEMORY_BYTES.saturating_mul(3) / 4
        } else {
            usize::MAX
        };
        let mut projected_memory = current_memory.saturating_add(incoming_size);

        let mut evictions_by_count = 0u64;
        let mut evictions_by_memory = 0u64;

        for (key, _, size) in by_access {
            let should_evict_for_count = entries.len() > target_entries;
            let should_evict_for_memory =
                needs_eviction_by_memory && projected_memory > target_memory;

            if !should_evict_for_count && !should_evict_for_memory {
                break;
            }

            if entries.remove(&key).is_some() {
                *current_memory = current_memory.saturating_sub(size);
                projected_memory = projected_memory.saturating_sub(size);
                if should_evict_for_count {
                    evictions_by_count = evictions_by_count.saturating_add(1);
                }
                if should_evict_for_memory {
                    evictions_by_memory = evictions_by_memory.saturating_add(1);
                }
            }
        }

        drop(entries);
        drop(current_memory);

        if evictions_by_count > 0 {
            self.record_evictions_by_count(evictions_by_count);
        }
        if evictions_by_memory > 0 {
            self.record_evictions_by_memory(evictions_by_memory);
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.entries.borrow().len()
    }

    pub(crate) fn take_metrics_delta(&self) -> TranscriptRenderCacheMetrics {
        let entries = self.entries.borrow();
        let current_memory = self.current_memory_bytes.borrow();

        let mut metrics = std::mem::take(&mut *self.metrics.borrow_mut());

        metrics.current_entries = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        metrics.current_memory_bytes = u64::try_from(*current_memory).unwrap_or(u64::MAX);

        metrics
    }

    fn record_hit(&self) {
        let mut metrics = self.metrics.borrow_mut();
        metrics.hits = metrics.hits.saturating_add(1);
    }

    fn record_miss(&self) {
        let mut metrics = self.metrics.borrow_mut();
        metrics.misses = metrics.misses.saturating_add(1);
    }

    fn record_evictions_by_count(&self, count: u64) {
        let mut metrics = self.metrics.borrow_mut();
        metrics.evictions_by_count = metrics.evictions_by_count.saturating_add(count);
    }

    fn record_evictions_by_memory(&self, count: u64) {
        let mut metrics = self.metrics.borrow_mut();
        metrics.evictions_by_memory = metrics.evictions_by_memory.saturating_add(count);
    }

    fn record_max_entry_size(&self, size: usize) {
        let mut metrics = self.metrics.borrow_mut();
        let size_u64 = u64::try_from(size).unwrap_or(u64::MAX);
        metrics.max_entry_bytes = metrics.max_entry_bytes.max(size_u64);
    }

    fn record_resize_reflow(&self, delta: ResizeReflowCacheDelta) {
        let mut metrics = self.metrics.borrow_mut();
        metrics.resize_reflow = metrics.resize_reflow.merge(delta);
    }

    fn record_render_profiler(&self, latency: std::time::Duration, height: u16) {
        let latency_us = duration_micros(latency);
        let mut metrics = self.metrics.borrow_mut();
        let profiler = &mut metrics.render_profiler;
        profiler.cell_samples = profiler.cell_samples.saturating_add(1);
        profiler.total_render_us = profiler.total_render_us.saturating_add(latency_us);
        profiler.max_render_us = profiler.max_render_us.max(latency_us);
        profiler.rendered_cell_rows = profiler
            .rendered_cell_rows
            .saturating_add(u64::from(height));
        if latency_us < 100 {
            profiler.under_100us = profiler.under_100us.saturating_add(1);
        } else if latency_us < 1_000 {
            profiler.under_1ms = profiler.under_1ms.saturating_add(1);
        } else if latency_us < 8_000 {
            profiler.under_8ms = profiler.under_8ms.saturating_add(1);
        } else {
            profiler.at_least_8ms = profiler.at_least_8ms.saturating_add(1);
        }
    }
}

#[cfg(test)]
#[path = "transcript_render_cache_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "transcript_render_cache_cache_budget_tests.rs"]
mod cache_budget_tests;

#[cfg(test)]
#[path = "transcript_render_cache_tests.rs"]
mod tests;
