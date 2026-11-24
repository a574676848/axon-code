//! Markdown pipe table 的增量 holdback 状态机。
//!
//! 表格列宽会随着后续数据行变化，因此未闭合表格必须保留在活动 cell 中，
//! 不能把中间布局提交到稳定队列。扫描器只处理已经换行的 source chunk，
//! 不参与 Markdown 渲染，也不会扫描历史全文。

use axon_display::{
    FenceKind, FenceTracker, is_table_delimiter_line, is_table_header_line, table_candidate_text,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableHoldbackState {
    None,
    PendingHeader { header_start: usize },
    Confirmed { table_start: usize },
}

#[derive(Debug, Clone, Copy)]
struct PreviousLine {
    source_start: usize,
    fence_kind: FenceKind,
    is_header: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct TableHoldbackScanner {
    source_offset: usize,
    fence_tracker: FenceTracker,
    previous_line: Option<PreviousLine>,
    pending_header_start: Option<usize>,
    confirmed_table_start: Option<usize>,
}

impl TableHoldbackScanner {
    pub(crate) const fn new() -> Self {
        Self {
            source_offset: 0,
            fence_tracker: FenceTracker::new(),
            previous_line: None,
            pending_header_start: None,
            confirmed_table_start: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    pub(crate) fn state(&self) -> TableHoldbackState {
        if let Some(table_start) = self.confirmed_table_start {
            TableHoldbackState::Confirmed { table_start }
        } else if let Some(header_start) = self.pending_header_start {
            TableHoldbackState::PendingHeader { header_start }
        } else {
            TableHoldbackState::None
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.state(), TableHoldbackState::None)
    }

    pub(crate) fn push_source_chunk(&mut self, source_chunk: &str) {
        for source_line in source_chunk.split_inclusive('\n') {
            self.push_line(source_line);
        }
    }

    fn push_line(&mut self, source_line: &str) {
        let source_start = self.source_offset;
        let line = source_line.trim_end_matches(['\n', '\r']);
        let fence_kind = self.fence_tracker.kind();

        if fence_kind != FenceKind::Other && line.trim().is_empty() {
            self.pending_header_start = None;
            self.confirmed_table_start = None;
            self.previous_line = None;
            self.fence_tracker.advance(line);
            self.source_offset = self.source_offset.saturating_add(source_line.len());
            return;
        }

        let candidate = (fence_kind != FenceKind::Other).then(|| table_candidate_text(line));
        let is_header = candidate
            .flatten()
            .is_some_and(|line| is_table_header_line(line) && !is_table_delimiter_line(line));
        let is_delimiter = candidate.flatten().is_some_and(is_table_delimiter_line);

        if self.confirmed_table_start.is_none()
            && let Some(previous_line) = self.previous_line
            && previous_line.fence_kind != FenceKind::Other
            && fence_kind != FenceKind::Other
            && previous_line.is_header
            && is_delimiter
        {
            self.confirmed_table_start = Some(previous_line.source_start);
            self.pending_header_start = None;
        }

        if self.confirmed_table_start.is_none() && !line.trim().is_empty() {
            self.pending_header_start = is_header.then_some(source_start);
        }

        self.previous_line = Some(PreviousLine {
            source_start,
            fence_kind,
            is_header,
        });
        self.fence_tracker.advance(line);
        self.source_offset = self.source_offset.saturating_add(source_line.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_header_and_delimiter_across_chunks() {
        let mut scanner = TableHoldbackScanner::new();
        scanner.push_source_chunk("| A | B |\n");
        assert_eq!(
            scanner.state(),
            TableHoldbackState::PendingHeader { header_start: 0 }
        );

        scanner.push_source_chunk("| --- | --- |\n");
        assert_eq!(
            scanner.state(),
            TableHoldbackState::Confirmed { table_start: 0 }
        );
    }

    #[test]
    fn ignores_table_like_lines_inside_non_markdown_fence() {
        let mut scanner = TableHoldbackScanner::new();
        scanner.push_source_chunk("```sh\n| A | B |\n| --- | --- |\n```\n");
        assert_eq!(scanner.state(), TableHoldbackState::None);
    }

    #[test]
    fn blank_line_releases_table_tail() {
        let mut scanner = TableHoldbackScanner::new();
        scanner.push_source_chunk("| A | B |\n| --- | --- |\n| 1 | 2 |\n");
        assert!(scanner.is_active());

        scanner.push_source_chunk("\n");
        assert_eq!(scanner.state(), TableHoldbackState::None);
    }
}
