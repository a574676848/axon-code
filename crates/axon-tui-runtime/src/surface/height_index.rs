//! RenderViewport 高度索引。
//!
//! surface 层只定义行高投影、可见窗口和行级锚点语义；具体 markdown/diff 渲染缓存由外层实现
//! `RenderHeightProvider` 注入，避免 surface 反向依赖 widget。

use std::{cell::RefCell, collections::HashMap};

use crate::{
    surface::visual_density::{VISUAL_NOISE_BUDGET, cell_spacing_rows},
    surface::{cell::RenderCell, cell_store::RenderCellList, viewport_anchor::ScrollAnchor},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveTranscriptTailKey {
    pub(crate) revision: u64,
    pub(crate) is_stream_continuation: bool,
    pub(crate) transcript_animation_tick: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LiveTranscriptTail<'a> {
    pub(crate) lines: &'a [crate::surface::cell::RenderCellRecord],
    pub(crate) key: LiveTranscriptTailKey,
}

pub(crate) trait RenderHeightProvider {
    fn desired_height(&self, cell: &dyn RenderCell, expanded: bool, width: u16) -> u16;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TranscriptViewportHeightKey {
    cell_id: u64,
    revision: u64,
    width: u16,
    transcript_key: u64,
    expanded: bool,
}

impl TranscriptViewportHeightKey {
    fn from_cell(cell: &dyn RenderCell, width: u16, expanded: bool) -> Self {
        Self {
            cell_id: cell.id().get().get(),
            revision: cell.revision(),
            width,
            transcript_key: cell.transcript_key(),
            expanded,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptViewportHeightCache {
    heights: RefCell<HashMap<TranscriptViewportHeightKey, u16>>,
    keys_by_cell_id: RefCell<HashMap<u64, Vec<TranscriptViewportHeightKey>>>,
}

impl TranscriptViewportHeightCache {
    pub(crate) fn desired_height(
        &self,
        cell: &dyn RenderCell,
        expanded: bool,
        width: u16,
        heights: &impl RenderHeightProvider,
    ) -> u16 {
        let key = TranscriptViewportHeightKey::from_cell(cell, width, expanded);
        if let Some(height) = self.heights.borrow().get(&key).copied() {
            return height;
        }

        let height = heights.desired_height(cell, expanded, width).max(1);
        let mut cached_heights = self.heights.borrow_mut();
        let mut keys_by_cell_id = self.keys_by_cell_id.borrow_mut();
        let cell_keys = keys_by_cell_id.entry(key.cell_id).or_default();
        cell_keys.retain(|cached_key| {
            let stale = cached_key.revision != key.revision;
            if stale {
                cached_heights.remove(cached_key);
            }
            !stale
        });
        if cached_heights.insert(key, height).is_none() {
            cell_keys.push(key);
        }
        height
    }

    pub(crate) fn remove_cells(&self, cell_ids: &[crate::surface::cell_id::CellId]) {
        if cell_ids.is_empty() {
            return;
        }

        let mut cached_heights = self.heights.borrow_mut();
        let mut keys_by_cell_id = self.keys_by_cell_id.borrow_mut();
        for cell_id in cell_ids {
            let Some(keys) = keys_by_cell_id.remove(&cell_id.get().get()) else {
                continue;
            };
            for key in keys {
                cached_heights.remove(&key);
            }
        }
    }

    pub(crate) fn clear(&self) {
        self.heights.borrow_mut().clear();
        self.keys_by_cell_id.borrow_mut().clear();
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.heights.borrow().len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptViewportCellMetrics {
    pub(crate) transcript_index: Option<usize>,
    pub(crate) render_id: u64,
    pub(crate) revision: u64,
    pub(crate) top: usize,
    pub(crate) height: usize,
}

impl TranscriptViewportCellMetrics {
    pub(crate) fn bottom(&self) -> usize {
        self.top.saturating_add(self.height)
    }
}

#[derive(Default)]
pub(crate) struct TranscriptViewportHeightIndex<'a> {
    items: Vec<TranscriptViewportItem<'a>>,
    cells: Vec<TranscriptViewportCellMetrics>,
    cell_by_id: HashMap<u64, usize>,
    content_height: usize,
}

impl<'a> TranscriptViewportHeightIndex<'a> {
    #[cfg(test)]
    pub(crate) fn build(
        lines: &'a RenderCellList,
        selected_cell: Option<usize>,
        selected_line: Option<usize>,
        active_tail: Option<LiveTranscriptTail<'a>>,
        width: u16,
        heights: &impl RenderHeightProvider,
    ) -> Self {
        Self::build_with_cache(
            lines,
            selected_cell,
            selected_line,
            active_tail,
            width,
            heights,
            None,
        )
    }

    pub(crate) fn build_with_cache(
        lines: &'a RenderCellList,
        selected_cell: Option<usize>,
        selected_line: Option<usize>,
        active_tail: Option<LiveTranscriptTail<'a>>,
        width: u16,
        heights: &impl RenderHeightProvider,
        height_cache: Option<&TranscriptViewportHeightCache>,
    ) -> Self {
        let mut index = Self::default();
        let mut previous_line: Option<&dyn RenderCell> = None;
        for (transcript_index, line) in lines.iter_cells().enumerate() {
            index.push_spacing_if_needed(previous_line, line);
            let selected =
                selected_cell == Some(transcript_index) || selected_line == Some(transcript_index);
            index.push_cell(
                Some(transcript_index),
                line,
                selected,
                width,
                heights,
                height_cache,
            );
            previous_line = Some(line);
        }
        if let Some(active_tail) = active_tail {
            index.push_active_tail_spacing_if_needed(previous_line, active_tail.key);
            for line in active_tail.lines {
                index.push_spacing_if_needed(previous_line, line);
                index.push_cell(None, line, false, width, heights, height_cache);
                previous_line = Some(line);
            }
        }
        index
    }

    pub(crate) fn content_height(&self) -> usize {
        self.content_height
    }

    pub(crate) fn max_offset_rows(&self, viewport_height: u16) -> usize {
        self.content_height
            .saturating_sub(usize::from(viewport_height.max(1)))
    }

    pub(crate) fn cell_tops(&self) -> Vec<usize> {
        self.cells.iter().map(|cell| cell.top).collect()
    }

    pub(crate) fn cell_metrics(&self) -> &[TranscriptViewportCellMetrics] {
        &self.cells
    }

    #[cfg(test)]
    fn items(&self) -> &[TranscriptViewportItem<'a>] {
        &self.items
    }

    pub(crate) fn visible_items(
        &self,
        top_row: usize,
        height: u16,
    ) -> &[TranscriptViewportItem<'a>] {
        if self.items.is_empty() {
            return &self.items;
        }
        let bottom_row = top_row.saturating_add(usize::from(height));
        let start = self.items.partition_point(|item| item.bottom() <= top_row);
        let end = start + self.items[start..].partition_point(|item| item.top < bottom_row);
        &self.items[start..end]
    }

    pub(crate) fn capture_anchor(&self, row: usize) -> Option<ScrollAnchor> {
        if self.cells.is_empty() {
            return None;
        }
        let insertion = self.cells.partition_point(|cell| cell.top <= row);
        if let Some(cell) = insertion
            .checked_sub(1)
            .and_then(|index| self.cells.get(index))
            .filter(|cell| row < cell.bottom())
        {
            return Some(ScrollAnchor {
                cell_id: cell.render_id,
                row_offset_in_cell: row.saturating_sub(cell.top),
            });
        }
        if let Some(next_cell) = self.cells.get(insertion) {
            return Some(ScrollAnchor {
                cell_id: next_cell.render_id,
                row_offset_in_cell: 0,
            });
        }
        let last_cell = self.cells.last()?;
        Some(ScrollAnchor {
            cell_id: last_cell.render_id,
            row_offset_in_cell: last_cell.height.saturating_sub(1),
        })
    }

    pub(crate) fn anchor_row(&self, anchor: ScrollAnchor) -> Option<usize> {
        let cell = self.cells.get(*self.cell_by_id.get(&anchor.cell_id)?)?;
        Some(
            cell.top
                .saturating_add(anchor.row_offset_in_cell.min(cell.height.saturating_sub(1))),
        )
    }

    fn push_spacing_if_needed(
        &mut self,
        previous: Option<&dyn RenderCell>,
        current: &dyn RenderCell,
    ) {
        self.push_spacing(cell_spacing_rows(previous, current));
    }

    fn push_spacing(&mut self, height: u16) {
        if height == 0 {
            return;
        }
        if self
            .items
            .last()
            .is_some_and(TranscriptViewportItem::is_spacing)
        {
            return;
        }
        self.push_item(TranscriptViewportItemKind::Spacing, usize::from(height));
    }

    fn push_active_tail_spacing_if_needed(
        &mut self,
        previous: Option<&dyn RenderCell>,
        key: LiveTranscriptTailKey,
    ) {
        if previous.is_some() && !key.is_stream_continuation {
            self.push_spacing(VISUAL_NOISE_BUDGET.turn_separator_rows);
        }
    }

    fn push_cell(
        &mut self,
        transcript_index: Option<usize>,
        cell: &'a dyn RenderCell,
        selected: bool,
        width: u16,
        heights: &impl RenderHeightProvider,
        height_cache: Option<&TranscriptViewportHeightCache>,
    ) {
        let height = usize::from(height_cache.map_or_else(
            || heights.desired_height(cell, selected, width).max(1),
            |cache| cache.desired_height(cell, selected, width, heights),
        ));
        let metrics = TranscriptViewportCellMetrics {
            transcript_index,
            render_id: cell.id().get().get(),
            revision: cell.revision(),
            top: self.content_height,
            height,
        };
        let metrics_index = self.cells.len();
        self.cell_by_id.insert(metrics.render_id, metrics_index);
        self.cells.push(metrics);
        self.push_item(TranscriptViewportItemKind::Cell { cell, selected }, height);
    }

    fn push_item(&mut self, kind: TranscriptViewportItemKind<'a>, height: usize) {
        let top = self.content_height;
        self.content_height = self.content_height.saturating_add(height);
        self.items
            .push(TranscriptViewportItem { top, height, kind });
    }
}

pub(crate) struct TranscriptViewportItem<'a> {
    pub(crate) top: usize,
    pub(crate) height: usize,
    pub(crate) kind: TranscriptViewportItemKind<'a>,
}

impl TranscriptViewportItem<'_> {
    pub(crate) fn bottom(&self) -> usize {
        self.top.saturating_add(self.height)
    }

    pub(crate) fn is_selected(&self) -> bool {
        matches!(
            self.kind,
            TranscriptViewportItemKind::Cell { selected: true, .. }
        )
    }

    fn is_spacing(&self) -> bool {
        matches!(self.kind, TranscriptViewportItemKind::Spacing)
    }
}

pub(crate) enum TranscriptViewportItemKind<'a> {
    Spacing,
    Cell {
        cell: &'a dyn RenderCell,
        selected: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        LiveTranscriptTail, LiveTranscriptTailKey, RenderHeightProvider,
        TranscriptViewportHeightCache, TranscriptViewportHeightIndex,
    };
    use crate::surface::{
        cell::{RenderCell, RenderCellRecord},
        cell_store::RenderCellList,
        viewport_anchor::ScrollAnchor,
    };

    #[derive(Default)]
    struct TestHeights(std::cell::Cell<u64>);

    impl RenderHeightProvider for TestHeights {
        fn desired_height(&self, cell: &dyn RenderCell, _expanded: bool, _width: u16) -> u16 {
            self.0.set(self.0.get().saturating_add(1));
            u16::try_from(cell.summary().text.lines().count().max(1)).unwrap_or(u16::MAX)
        }
    }

    #[derive(Default)]
    struct RecordingHeights {
        calls: std::cell::Cell<u64>,
        expanded_seen: std::cell::Cell<bool>,
    }

    impl RenderHeightProvider for RecordingHeights {
        fn desired_height(&self, _cell: &dyn RenderCell, expanded: bool, _width: u16) -> u16 {
            self.calls.set(self.calls.get().saturating_add(1));
            self.expanded_seen.set(self.expanded_seen.get() || expanded);
            1
        }
    }

    #[test]
    fn height_index_records_stable_cell_metrics() {
        let lines = cell_list(vec![
            agent_cell("metrics:first", "第一段"),
            RenderCellRecord::user("第二段"),
        ]);
        let heights = TestHeights::default();

        let index = TranscriptViewportHeightIndex::build(&lines, None, None, None, 80, &heights);

        assert_eq!(index.cell_metrics().len(), 2);
        assert_eq!(index.cell_metrics()[0].transcript_index, Some(0));
        assert_eq!(index.cell_metrics()[0].render_id, lines[0].render_id());
        assert_eq!(index.cell_metrics()[0].revision, lines[0].render_revision());
        assert_eq!(index.cell_metrics()[0].top, 0);
        assert!(index.cell_metrics()[0].height >= 1);
        assert!(index.cell_metrics()[1].top >= index.cell_metrics()[0].bottom());
    }

    #[test]
    fn row_anchor_restores_inside_same_cell_after_prepend() {
        let old_prefix = agent_cell("anchor:initial-prefix", "初始前置");
        let target = agent_cell("anchor:target", "target row one\nrow two\nrow three");
        let heights = TestHeights::default();
        let old_lines = cell_list(vec![old_prefix.clone(), target.clone()]);
        let old_index =
            TranscriptViewportHeightIndex::build(&old_lines, None, None, None, 80, &heights);
        let target_top = old_index.cell_metrics()[1].top;

        let anchor = old_index.capture_anchor(target_top + 1).unwrap();

        let new_prefix = agent_cell("anchor:inserted-prefix", "新增前置\n更多行\n更多行");
        let new_lines = cell_list(vec![new_prefix, old_prefix, target.clone()]);
        let new_index =
            TranscriptViewportHeightIndex::build(&new_lines, None, None, None, 80, &heights);
        let restored_row = new_index.anchor_row(anchor).unwrap();

        assert_eq!(
            anchor,
            ScrollAnchor {
                cell_id: target.render_id(),
                row_offset_in_cell: 1
            }
        );
        assert_eq!(restored_row, new_index.cell_metrics()[2].top + 1);
    }

    #[test]
    fn spacing_row_anchor_uses_next_cell_boundary() {
        let lines = cell_list(vec![
            agent_cell("spacing:tool-done", "完成工具调用"),
            RenderCellRecord::user("继续"),
        ]);
        let heights = TestHeights::default();
        let index = TranscriptViewportHeightIndex::build(&lines, None, None, None, 80, &heights);
        let spacing_row = index.cell_metrics()[0].bottom();

        let anchor = index.capture_anchor(spacing_row).unwrap();

        assert_eq!(anchor.cell_id, lines[1].render_id());
        assert_eq!(anchor.row_offset_in_cell, 0);
    }

    #[test]
    fn active_tail_cells_use_regular_spacing_policy() {
        let lines = RenderCellList::default();
        let reasoning =
            RenderCellRecord::reasoning_with_data(crate::surface::cell::RenderReasoningCell {
                id: "reasoning:test:active-spacing".to_owned(),
                text: "分析路径".to_owned(),
                done: false,
                elapsed_ms: None,
            });
        let exec = RenderCellRecord::exec_with_data(
            crate::surface::cell_event_data::AgentRenderExecCell::running_with_id(
                "exec:test:active-spacing",
                "shell",
                "run",
                None,
            ),
            crate::surface::cell::RenderCellAction::Running,
        );
        let active_lines = vec![reasoning, exec];
        let active_tail = LiveTranscriptTail {
            lines: &active_lines,
            key: LiveTranscriptTailKey {
                revision: 1,
                is_stream_continuation: false,
                transcript_animation_tick: None,
            },
        };
        let heights = TestHeights::default();

        let index = TranscriptViewportHeightIndex::build(
            &lines,
            None,
            None,
            Some(active_tail),
            80,
            &heights,
        );

        assert_eq!(index.cell_metrics().len(), 2);
        assert_eq!(
            index.cell_metrics()[1].top,
            index.cell_metrics()[0].bottom() + 1,
            "active reasoning 与后续工具 cell 之间应保留 1 行空白"
        );
    }

    #[test]
    fn visible_items_uses_row_range_without_returning_full_history() {
        let lines = cell_list(
            (0..24)
                .map(|index| agent_cell(format!("visible:{index}"), format!("line-{index}")))
                .collect(),
        );
        let heights = TestHeights::default();
        let index = TranscriptViewportHeightIndex::build(&lines, None, None, None, 80, &heights);

        let visible = index.visible_items(8, 4);

        assert!(!visible.is_empty());
        assert!(visible.len() < index.items().len());
        assert!(
            visible
                .iter()
                .all(|item| item.bottom() > 8 && item.top < 12)
        );
    }

    #[test]
    fn selected_cell_requests_expanded_height() {
        let lines = cell_list(vec![agent_cell("selected:stable", "稳定内容")]);
        let heights = RecordingHeights::default();

        let _ = TranscriptViewportHeightIndex::build(&lines, Some(0), None, None, 80, &heights);

        assert_eq!(heights.calls.get(), 1);
        assert!(heights.expanded_seen.get());
    }

    #[test]
    fn anchor_capture_resolves_scaled_indexes() {
        for cell_count in [100usize, 1_000, 5_000] {
            let lines = cell_list(
                (0..cell_count)
                    .map(|index| {
                        agent_cell(
                            format!("scaled:{index}"),
                            format!("row-{index}-a\nrow-{index}-b\nrow-{index}-c"),
                        )
                    })
                    .collect(),
            );
            let heights = TestHeights::default();
            let index =
                TranscriptViewportHeightIndex::build(&lines, None, None, None, 80, &heights);
            let middle = cell_count / 2;
            let row = index.cell_metrics()[middle].top + 1;

            let anchor = index.capture_anchor(row).unwrap();

            assert_eq!(anchor.cell_id, lines[middle].render_id());
            assert_eq!(anchor.row_offset_in_cell, 1);
            assert_eq!(index.anchor_row(anchor), Some(row));
        }
    }

    #[test]
    fn height_cache_reclaims_stale_revisions_and_evicted_cells() {
        let cache = TranscriptViewportHeightCache::default();
        let heights = TestHeights::default();
        let mut active = agent_cell("height-cache-active", "第一版");
        let retained = agent_cell("height-cache-retained", "保留");

        assert_eq!(cache.desired_height(&active, false, 80, &heights), 1);
        active.bump_render_revision();
        assert_eq!(cache.desired_height(&active, false, 80, &heights), 1);
        assert_eq!(cache.desired_height(&retained, false, 80, &heights), 1);
        assert_eq!(cache.entry_count(), 2);

        cache.remove_cells(&[active.id()]);

        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.desired_height(&retained, false, 80, &heights), 1);
        assert_eq!(heights.0.get(), 3);
    }

    fn cell_list(records: Vec<RenderCellRecord>) -> RenderCellList {
        RenderCellList::from_records(records)
    }

    fn agent_cell(id: impl Into<String>, text: impl Into<String>) -> RenderCellRecord {
        RenderCellRecord::agent_message(format!("assistant:test:{}", id.into()), text)
    }
}
