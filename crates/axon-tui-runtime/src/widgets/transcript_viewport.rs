//! Transcript 统一 viewport projection。
//!
//! 主 Transcript 使用这里的 cell metrics、滚动窗口与局部渲染逻辑。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    widgets::{Paragraph, Widget},
};

use crate::{
    surface::{
        cell_store::RenderCellList,
        height_index::{
            LiveTranscriptTail, TranscriptViewportCellMetrics, TranscriptViewportHeightCache,
            TranscriptViewportHeightIndex, TranscriptViewportItem, TranscriptViewportItemKind,
        },
        viewport_anchor::ScrollAnchor,
    },
    widgets::{
        TranscriptRenderCache,
        terminal_hyperlinks::{
            HyperlinkLine, TerminalHyperlinkSurface, record_buffer_hyperlinks, visible_lines,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptViewportMetrics {
    pub(crate) max_offset_rows: usize,
    pub(crate) cell_tops: Vec<usize>,
    pub(crate) cells: Vec<TranscriptViewportCellMetrics>,
    pub(crate) top_anchor: Option<ScrollAnchor>,
    pub(crate) top_anchor_row: Option<usize>,
}

pub(crate) struct TranscriptViewportRequest<'a> {
    pub(crate) lines: &'a RenderCellList,
    pub(crate) height: u16,
    pub(crate) width: u16,
    pub(crate) selected_cell: Option<usize>,
    pub(crate) selected_line: Option<usize>,
    pub(crate) active_tail: Option<LiveTranscriptTail<'a>>,
    pub(crate) cache: &'a TranscriptRenderCache,
    pub(crate) height_cache: &'a TranscriptViewportHeightCache,
}

impl TranscriptViewportMetrics {
    pub(crate) fn capture_anchor(&self, row: usize) -> Option<ScrollAnchor> {
        if self.cells.is_empty() {
            return None;
        }
        if let Some(cell) = self
            .cells
            .iter()
            .find(|cell| cell.top <= row && row < cell.bottom())
        {
            return Some(ScrollAnchor {
                cell_id: cell.render_id,
                row_offset_in_cell: row.saturating_sub(cell.top),
            });
        }
        if let Some(next_cell) = self.cells.iter().find(|cell| row < cell.top) {
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
        let cell = self
            .cells
            .iter()
            .find(|cell| cell.render_id == anchor.cell_id)?;
        Some(
            cell.top
                .saturating_add(anchor.row_offset_in_cell.min(cell.height.saturating_sub(1))),
        )
    }
}

#[cfg(test)]
pub(crate) fn transcript_viewport_metrics(
    lines: &RenderCellList,
    height: u16,
    width: u16,
    selected_cell: Option<usize>,
    selected_line: Option<usize>,
    active_tail: Option<LiveTranscriptTail<'_>>,
    cache: &TranscriptRenderCache,
) -> TranscriptViewportMetrics {
    let height_cache = TranscriptViewportHeightCache::default();
    transcript_viewport_metrics_with_height_cache(TranscriptViewportRequest {
        lines,
        height,
        width,
        selected_cell,
        selected_line,
        active_tail,
        cache,
        height_cache: &height_cache,
    })
}

pub(crate) fn transcript_viewport_metrics_with_height_cache(
    request: TranscriptViewportRequest<'_>,
) -> TranscriptViewportMetrics {
    let height_index = build_transcript_viewport_height_index(
        request.lines,
        request.selected_cell,
        request.selected_line,
        request.active_tail,
        request.width,
        request.cache,
        request.height_cache,
    );
    transcript_viewport_metrics_from_height_index(&height_index, request.height)
}

pub(crate) fn build_transcript_viewport_height_index<'a>(
    lines: &'a RenderCellList,
    selected_cell: Option<usize>,
    selected_line: Option<usize>,
    active_tail: Option<LiveTranscriptTail<'a>>,
    width: u16,
    cache: &TranscriptRenderCache,
    height_cache: &TranscriptViewportHeightCache,
) -> TranscriptViewportHeightIndex<'a> {
    TranscriptViewportHeightIndex::build_with_cache(
        lines,
        selected_cell,
        selected_line,
        active_tail,
        width,
        cache,
        Some(height_cache),
    )
}

pub(crate) fn transcript_viewport_metrics_from_height_index(
    height_index: &TranscriptViewportHeightIndex<'_>,
    height: u16,
) -> TranscriptViewportMetrics {
    let top_anchor = height_index.capture_anchor(0);
    TranscriptViewportMetrics {
        max_offset_rows: height_index.max_offset_rows(height),
        cell_tops: height_index.cell_tops(),
        cells: height_index.cell_metrics().to_vec(),
        top_anchor,
        top_anchor_row: top_anchor.and_then(|anchor| height_index.anchor_row(anchor)),
    }
}

pub(crate) fn render_transcript_viewport_with_height_index(
    height_index: TranscriptViewportHeightIndex<'_>,
    offset_rows: usize,
    bottom_align: bool,
    cache: &TranscriptRenderCache,
    area: Rect,
    buf: &mut Buffer,
    hyperlinks: Option<&mut TerminalHyperlinkSurface>,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    TranscriptViewportWindow::new(height_index, offset_rows, bottom_align, cache)
        .render(area, buf, hyperlinks);
}

struct TranscriptViewportWindow<'a, 'cache> {
    height_index: TranscriptViewportHeightIndex<'a>,
    offset_rows: usize,
    bottom_align: bool,
    cache: &'cache TranscriptRenderCache,
}

impl<'a, 'cache> TranscriptViewportWindow<'a, 'cache> {
    fn new(
        height_index: TranscriptViewportHeightIndex<'a>,
        offset_rows: usize,
        bottom_align: bool,
        cache: &'cache TranscriptRenderCache,
    ) -> Self {
        Self {
            height_index,
            offset_rows,
            bottom_align,
            cache,
        }
    }

    fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        mut hyperlinks: Option<&mut TerminalHyperlinkSurface>,
    ) {
        let top_row = self.top_row(area.height);
        let target_top = area.y.saturating_add(self.bottom_padding(area.height));
        for item in self.height_index.visible_items(top_row, area.height) {
            self.render_visible_item(
                item,
                top_row,
                target_top,
                area,
                buf,
                hyperlinks.as_deref_mut(),
            );
        }
    }
}

impl TranscriptViewportWindow<'_, '_> {
    fn top_row(&self, viewport_height: u16) -> usize {
        let max_offset_rows = self.height_index.max_offset_rows(viewport_height);
        max_offset_rows.saturating_sub(self.offset_rows.min(max_offset_rows))
    }

    fn bottom_padding(&self, viewport_height: u16) -> u16 {
        if !self.bottom_align {
            return 0;
        }
        let viewport_height = usize::from(viewport_height);
        let padding = viewport_height.saturating_sub(self.height_index.content_height());
        u16::try_from(padding).unwrap_or(u16::MAX)
    }

    fn render_visible_item(
        &self,
        item: &TranscriptViewportItem,
        top_row: usize,
        target_top: u16,
        area: Rect,
        buf: &mut Buffer,
        hyperlinks: Option<&mut TerminalHyperlinkSurface>,
    ) {
        let item_visible_top = item.top.max(top_row);
        let source_offset = item_visible_top.saturating_sub(item.top);
        let target_y = target_top.saturating_add(
            u16::try_from(item_visible_top.saturating_sub(top_row)).unwrap_or(u16::MAX),
        );
        let draw_height = item
            .bottom()
            .min(top_row.saturating_add(usize::from(area.height)))
            .saturating_sub(item_visible_top);
        if draw_height == 0 || matches!(item.kind, TranscriptViewportItemKind::Spacing) {
            return;
        }
        let TranscriptViewportItemKind::Cell { cell, .. } = item.kind else {
            return;
        };
        let expanded = item.is_selected();
        let lines = self
            .cache
            .render_cell_hyperlink_lines(cell, expanded, area.width);
        render_cell_slice(
            &lines,
            u16::try_from(source_offset).unwrap_or(u16::MAX),
            area,
            target_y,
            u16::try_from(draw_height).unwrap_or(u16::MAX),
            buf,
            hyperlinks,
        );
    }
}

fn render_cell_slice(
    lines: &[HyperlinkLine],
    source_offset: u16,
    area: Rect,
    target_y: u16,
    draw_height: u16,
    buf: &mut Buffer,
    hyperlinks: Option<&mut TerminalHyperlinkSurface>,
) {
    if source_offset == 0 {
        let draw_area = Rect::new(area.x, target_y, area.width, draw_height);
        render_cell_lines(
            lines,
            draw_area,
            buf,
            usize::from(source_offset),
            hyperlinks,
        );
        return;
    }
    let draw_area = Rect::new(area.x, target_y, area.width, draw_height);
    render_cell_lines_scrolled(lines, draw_area, buf, source_offset, hyperlinks);
}

fn render_cell_lines_scrolled(
    lines: &[HyperlinkLine],
    area: Rect,
    buf: &mut Buffer,
    source_offset: u16,
    hyperlinks: Option<&mut TerminalHyperlinkSurface>,
) {
    Paragraph::new(visible_lines(lines))
        .scroll((source_offset, 0))
        .render(area, buf);
    if let Some(hyperlinks) = hyperlinks {
        record_buffer_hyperlinks(hyperlinks, area, lines, usize::from(source_offset));
    }
}

fn render_cell_lines(
    lines: &[HyperlinkLine],
    area: Rect,
    buf: &mut Buffer,
    scroll_rows: usize,
    hyperlinks: Option<&mut TerminalHyperlinkSurface>,
) {
    Paragraph::new(visible_lines(lines)).render(area, buf);
    if let Some(hyperlinks) = hyperlinks {
        record_buffer_hyperlinks(hyperlinks, area, lines, scroll_rows);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use axon_protocol::diff::FileChange;

    use super::{
        build_transcript_viewport_height_index, render_transcript_viewport_with_height_index,
        transcript_viewport_metrics,
    };

    use crate::{
        surface::{
            cell::{RenderCellMode, RenderCellRecord},
            cell_diff_data::AgentRenderDiffCell,
            cell_store::RenderCellList,
            height_index::TranscriptViewportHeightCache,
            viewport_anchor::ScrollAnchor,
        },
        widgets::TranscriptRenderCache,
    };
    use ratatui::{buffer::Buffer, layout::Rect};

    #[test]
    fn viewport_metrics_and_render_share_wrapped_cell_projection() {
        let lines = cell_list(vec![
            agent_cell("wrapped-response", "long response wraps across rows"),
            RenderCellRecord::user("next task"),
        ]);
        let cache = TranscriptRenderCache::default();
        let height_cache = TranscriptViewportHeightCache::default();
        let metrics = transcript_viewport_metrics(&lines, 2, 8, None, None, None, &cache);
        let area = Rect::new(0, 0, 8, 2);
        let mut buffer = Buffer::empty(area);

        let height_index = build_transcript_viewport_height_index(
            &lines,
            None,
            None,
            None,
            area.width,
            &cache,
            &height_cache,
        );
        render_transcript_viewport_with_height_index(
            height_index,
            metrics.max_offset_rows,
            false,
            &cache,
            area,
            &mut buffer,
            None,
        );

        assert!(metrics.max_offset_rows > 0);
        assert_eq!(metrics.cell_tops.len(), 2);
        assert_eq!(metrics.cells.len(), 2);
        assert_eq!(metrics.cell_tops[0], metrics.cells[0].top);
        assert_eq!(metrics.cell_tops[1], metrics.cells[1].top);
        assert_eq!(
            metrics.top_anchor.map(|anchor| anchor.cell_id),
            Some(lines[0].render_id())
        );
        assert_eq!(metrics.top_anchor_row, Some(0));
        assert!(metrics.cell_tops[1] > 1);
        assert!(buffer.content().iter().any(|cell| cell.symbol() != " "));
    }

    fn exec_line(tool_name: &str) -> RenderCellRecord {
        use crate::surface::cell::RenderCellKind;
        use crate::surface::cell_event_data::AgentRenderExecCell;
        let line = RenderCellRecord::exec_with_data(
            AgentRenderExecCell::running_with_id(
                format!("exec:test:{tool_name}"),
                tool_name.to_owned(),
                format!("provider:test:{tool_name}"),
                None,
            ),
            crate::surface::cell::RenderCellAction::Called,
        );
        assert!(
            matches!(line.mode, RenderCellMode::Cell(RenderCellKind::EXEC)),
            "exec line must use Cell(Exec) mode"
        );
        line
    }

    #[test]
    fn agent_answer_after_tool_cell_inserts_turn_separator() {
        let lines = cell_list(vec![
            exec_line("read_file"),
            agent_cell("after-tool", "完成"),
        ]);
        let cache = TranscriptRenderCache::default();
        let metrics = transcript_viewport_metrics(&lines, 32, 80, None, None, None, &cache);
        // 工具 cell 之后接 agent 文字回复，必须留 1 行空。
        assert_eq!(metrics.cell_tops.len(), 2);
        assert!(
            metrics.cell_tops[1] >= metrics.cell_tops[0] + 2,
            "agent 文字回复前应至少有 1 行间距，cell_tops={:?}",
            metrics.cell_tops
        );
    }

    #[test]
    fn consecutive_agent_messages_skip_separator() {
        let lines = cell_list(vec![
            agent_cell("paragraph-1", "段落一"),
            agent_cell("paragraph-2", "段落二"),
        ]);
        let cache = TranscriptRenderCache::default();
        let metrics = transcript_viewport_metrics(&lines, 32, 80, None, None, None, &cache);
        // 同 turn 连续 agent 文字不重复插空行。
        assert!(
            metrics.cell_tops[1] < metrics.cell_tops[0] + 2,
            "连续 agent 文字消息不应插空行，cell_tops={:?}",
            metrics.cell_tops
        );
    }

    #[test]
    fn consecutive_tool_cells_insert_readable_separator() {
        let cache = TranscriptRenderCache::default();
        let single = cell_list(vec![exec_line("read_file")]);
        let single_metrics = transcript_viewport_metrics(&single, 32, 80, None, None, None, &cache);
        let single_height = cache.desired_height(&single[0], false, 80).max(1) as usize;
        let _ = single_metrics;

        let pair = cell_list(vec![exec_line("read_file"), exec_line("edit")]);
        let metrics = transcript_viewport_metrics(&pair, 32, 80, None, None, None, &cache);
        // 工具 cell 连续出现时保留 1 行空白，避免多个工具回执粘连。
        assert_eq!(
            metrics.cell_tops[1],
            metrics.cell_tops[0] + single_height + 1,
            "工具 cell 之间应插入 1 行空白，cell_tops={:?}, expected={}",
            metrics.cell_tops,
            single_height + 1
        );
    }

    #[test]
    fn viewport_metrics_capture_row_level_top_anchor() {
        let lines = cell_list(vec![agent_cell("anchor-lines", "第一行\n第二行")]);
        let cache = TranscriptRenderCache::default();

        let metrics = transcript_viewport_metrics(&lines, 2, 80, None, None, None, &cache);

        assert_eq!(
            metrics.top_anchor,
            Some(ScrollAnchor {
                cell_id: lines[0].render_id(),
                row_offset_in_cell: 0,
            })
        );
        assert_eq!(metrics.top_anchor_row, Some(0));
        assert_eq!(metrics.cells[0].height, 2);
    }

    #[test]
    fn viewport_metrics_resolves_anchor_against_same_projection() {
        let lines = cell_list(vec![
            agent_cell("first-line", "第一行"),
            RenderCellRecord::user("第二行\n第三行"),
        ]);
        let cache = TranscriptRenderCache::default();
        let metrics = transcript_viewport_metrics(&lines, 3, 80, None, None, None, &cache);
        let second_row = metrics.cells[1].top + 1;

        let anchor = metrics
            .capture_anchor(second_row)
            .expect("应捕获第二个 cell 的行锚点");

        assert_eq!(
            anchor,
            ScrollAnchor {
                cell_id: lines[1].render_id(),
                row_offset_in_cell: 1,
            }
        );
        assert_eq!(metrics.anchor_row(anchor), Some(second_row));
    }

    #[test]
    fn selected_cell_does_not_invalidate_transcript_content_cache() {
        let lines = cell_list(vec![agent_cell("cache-stable", "稳定内容")]);
        let cache = TranscriptRenderCache::default();

        let _ = transcript_viewport_metrics(&lines, 4, 80, None, None, None, &cache);
        let _ = transcript_viewport_metrics(&lines, 4, 80, Some(0), None, None, &cache);
        let metrics = cache.take_metrics_delta();

        assert_eq!(metrics.misses, 1);
        assert_eq!(metrics.hits, 1);
    }

    #[test]
    fn selected_diff_cell_renders_expanded_content() {
        let mut changes = BTreeMap::new();
        changes.insert(
            PathBuf::from("src/example.rs"),
            FileChange::Update {
                unified_diff: "--- src/example.rs\n+++ src/example.rs\n@@ -1 +1 @@\n-old content\n+new content".to_owned(),
                move_path: None,
            },
        );
        let lines = cell_list(vec![RenderCellRecord::file_change_with_data(
            AgentRenderDiffCell {
                id: "diff:test:expanded".to_owned(),
                title: "文件变更".to_owned(),
                changes,
                diagnostics: BTreeMap::new(),
                text_writes_applied: 1,
                renames_applied: 0,
                truncated: false,
                truncation_boundary: None,
            },
        )]);
        let cache = TranscriptRenderCache::default();
        let height_cache = TranscriptViewportHeightCache::default();
        let area = Rect::new(0, 0, 80, 6);
        let mut buffer = Buffer::empty(area);

        let height_index = build_transcript_viewport_height_index(
            &lines,
            Some(0),
            None,
            None,
            area.width,
            &cache,
            &height_cache,
        );
        render_transcript_viewport_with_height_index(
            height_index,
            0,
            false,
            &cache,
            area,
            &mut buffer,
            None,
        );

        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(
            rendered.contains("+new content"),
            "选中 diff 后应绘制详情内容，实际输出：{rendered}"
        );
        assert!(
            rendered.contains("1   │ -old content"),
            "选中 diff 后应绘制结构化行号，实际输出：{rendered}"
        );
    }

    fn cell_list(records: Vec<RenderCellRecord>) -> RenderCellList {
        RenderCellList::from_records(records)
    }

    fn agent_cell(id: impl Into<String>, text: impl Into<String>) -> RenderCellRecord {
        RenderCellRecord::agent_message(format!("assistant:test:{}", id.into()), text)
    }
}
