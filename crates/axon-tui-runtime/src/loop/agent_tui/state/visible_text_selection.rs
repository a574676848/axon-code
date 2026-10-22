use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;

use crate::surface::display_width::DisplayWidthText;

const MIN_TERMINAL_CELL_WIDTH: usize = 1;
const BLANK_CELL_SYMBOL: &str = " ";
const MOUSE_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(450);

#[derive(Debug, Clone, Default)]
pub(crate) struct VisibleTextSelectionState {
    snapshot: VisibleTextSnapshot,
    drag: Option<VisibleTextDrag>,
    completed: Option<VisibleTextSelectionRange>,
    last_click: Option<VisibleTextClick>,
}

impl VisibleTextSelectionState {
    pub(crate) fn update_snapshot(&mut self, snapshot: VisibleTextSnapshot) {
        if self.drag.is_none() {
            if self.completed.is_some() && self.snapshot != snapshot {
                self.completed = None;
            }
            self.snapshot = snapshot;
        }
    }

    pub(crate) fn begin(&mut self, row: u16, column: u16) {
        self.completed = None;
        let position = VisibleTextPosition::new(row, column);
        self.drag = Some(VisibleTextDrag {
            anchor: position,
            focus: position,
            moved: false,
        });
    }

    pub(crate) fn update(&mut self, row: u16, column: u16) -> bool {
        let Some(drag) = self.drag.as_mut() else {
            return false;
        };
        let focus = VisibleTextPosition::new(row, column);
        let changed = drag.focus != focus;
        drag.focus = focus;
        drag.moved |= changed;
        if changed {
            self.last_click = None;
        }
        changed
    }

    pub(crate) fn active_range(&self) -> Option<VisibleTextSelectionRange> {
        if let Some(drag) = self.drag {
            return drag
                .moved
                .then(|| VisibleTextSelectionRange::from_positions(drag.anchor, drag.focus));
        }
        self.completed
    }

    pub(crate) fn finish(&mut self) -> Option<String> {
        let drag = self.drag.take()?;
        drag.moved
            .then(|| self.snapshot.selected_text(drag.anchor, drag.focus))
            .filter(|text| !text.trim().is_empty())
    }

    pub(crate) fn take_click_position(&mut self) -> Option<(u16, u16)> {
        let drag = self.drag.as_ref()?;
        if drag.moved {
            return None;
        }
        let position = drag.anchor;
        self.drag = None;
        Some((position.row, position.column))
    }

    pub(crate) fn select_content_run_on_double_click(
        &mut self,
        row: u16,
        column: u16,
        now: Instant,
    ) -> bool {
        let position = VisibleTextPosition::new(row, column);
        self.completed = None;
        let double_click = self
            .last_click
            .is_some_and(|click| click.position == position && click.within_window(now));
        self.last_click = (!double_click).then_some(VisibleTextClick {
            position,
            clicked_at: now,
        });
        if !double_click {
            return false;
        }
        let Some((anchor, focus)) = self.snapshot.content_run_at(position) else {
            return false;
        };
        self.drag = Some(VisibleTextDrag {
            anchor,
            focus,
            moved: true,
        });
        true
    }

    pub(crate) fn retain_active_selection(&mut self) {
        self.completed = self.active_range();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VisibleTextSelectionRange {
    pub(crate) start_row: u16,
    pub(crate) start_column: u16,
    pub(crate) end_row: u16,
    pub(crate) end_column: u16,
}

impl VisibleTextSelectionRange {
    fn from_positions(anchor: VisibleTextPosition, focus: VisibleTextPosition) -> Self {
        let (start, end) = ordered_positions(anchor, focus);
        Self {
            start_row: start.row,
            start_column: start.column,
            end_row: end.row,
            end_column: end.column,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct VisibleTextSnapshot {
    rows: Vec<VisibleTextRow>,
}

impl VisibleTextSnapshot {
    pub(crate) fn from_buffer(buffer: &Buffer) -> Self {
        let rows = (buffer.area.y..buffer.area.y.saturating_add(buffer.area.height))
            .map(|y| VisibleTextRow::from_buffer(buffer, y))
            .collect();
        Self { rows }
    }

    fn content_run_at(
        &self,
        position: VisibleTextPosition,
    ) -> Option<(VisibleTextPosition, VisibleTextPosition)> {
        let row = self.rows.get(usize::from(position.row))?;
        let (start_column, end_column) = row.content_run_at(position.column)?;
        Some((
            VisibleTextPosition::new(position.row, start_column),
            VisibleTextPosition::new(position.row, end_column),
        ))
    }

    fn selected_text(&self, anchor: VisibleTextPosition, focus: VisibleTextPosition) -> String {
        let (start, end) = ordered_positions(anchor, focus);
        (start.row..=end.row)
            .filter_map(|row| {
                let line = self.rows.get(usize::from(row))?;
                let start_column = if row == start.row { start.column } else { 0 };
                let end_column = if row == end.row { end.column } else { u16::MAX };
                let selected = line.selected_text(start_column, end_column);
                (!selected.trim().is_empty()).then_some(selected)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct VisibleTextRow {
    spans: Vec<VisibleTextSpan>,
}

impl VisibleTextRow {
    fn from_buffer(buffer: &Buffer, y: u16) -> Self {
        let row_end = buffer.area.x.saturating_add(buffer.area.width);
        let mut spans = Vec::new();
        let mut x = buffer.area.x;
        while x < row_end {
            let symbol = buffer[(x, y)].symbol();
            let remaining_width = row_end.saturating_sub(x);
            let symbol_width =
                u16::try_from(DisplayWidthText::width(symbol).max(MIN_TERMINAL_CELL_WIDTH))
                    .unwrap_or(u16::MAX)
                    .min(remaining_width);
            spans.push(VisibleTextSpan {
                text: symbol.to_owned(),
                start_column: x.saturating_sub(buffer.area.x),
                end_column: x
                    .saturating_sub(buffer.area.x)
                    .saturating_add(symbol_width.saturating_sub(1)),
            });
            x = x.saturating_add(symbol_width);
        }
        while spans
            .last()
            .is_some_and(|span| span.text == BLANK_CELL_SYMBOL)
        {
            spans.pop();
        }
        Self { spans }
    }

    fn content_run_at(&self, column: u16) -> Option<(u16, u16)> {
        let hit_index = self
            .spans
            .iter()
            .position(|span| span.intersects(column, column))?;
        if self.spans[hit_index].is_blank() {
            return None;
        }
        let mut start_index = hit_index;
        while start_index > 0 && !self.spans[start_index - 1].is_blank() {
            start_index -= 1;
        }
        let mut end_index = hit_index;
        while end_index + 1 < self.spans.len() && !self.spans[end_index + 1].is_blank() {
            end_index += 1;
        }
        Some((
            self.spans[start_index].start_column,
            self.spans[end_index].end_column,
        ))
    }

    fn selected_text(&self, start_column: u16, end_column: u16) -> String {
        self.spans
            .iter()
            .filter(|span| span.intersects(start_column, end_column))
            .map(|span| span.text.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleTextSpan {
    text: String,
    start_column: u16,
    end_column: u16,
}

impl VisibleTextSpan {
    const fn intersects(&self, start_column: u16, end_column: u16) -> bool {
        self.end_column >= start_column && self.start_column <= end_column
    }

    fn is_blank(&self) -> bool {
        self.text.chars().all(char::is_whitespace)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VisibleTextPosition {
    row: u16,
    column: u16,
}

impl VisibleTextPosition {
    const fn new(row: u16, column: u16) -> Self {
        Self { row, column }
    }
}

#[derive(Debug, Clone, Copy)]
struct VisibleTextDrag {
    anchor: VisibleTextPosition,
    focus: VisibleTextPosition,
    moved: bool,
}

#[derive(Debug, Clone, Copy)]
struct VisibleTextClick {
    position: VisibleTextPosition,
    clicked_at: Instant,
}

impl VisibleTextClick {
    fn within_window(self, now: Instant) -> bool {
        now.saturating_duration_since(self.clicked_at) <= MOUSE_DOUBLE_CLICK_WINDOW
    }
}

fn ordered_positions(
    first: VisibleTextPosition,
    second: VisibleTextPosition,
) -> (VisibleTextPosition, VisibleTextPosition) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::{buffer::Buffer, layout::Rect, style::Style};

    use super::{VisibleTextSelectionState, VisibleTextSnapshot};
    use crate::surface::display_width::DisplayWidthText;

    #[test]
    fn drag_finish_selects_multiple_buffer_rows_by_terminal_column() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 6, 3));
        buffer.set_string(0, 0, "Header", Style::default());
        buffer.set_string(0, 1, "abcdef", Style::default());
        buffer.set_string(0, 2, "uvwxyz", Style::default());
        let mut selection = VisibleTextSelectionState::default();
        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&buffer));

        selection.begin(1, 1);
        assert!(selection.update(2, 2));

        assert_eq!(selection.finish(), Some("bcdef\nuvw".to_owned()));
    }

    #[test]
    fn click_without_drag_returns_none() {
        let mut selection = VisibleTextSelectionState::default();

        selection.begin(0, 0);

        assert_eq!(selection.finish(), None);
    }

    #[test]
    fn click_position_is_consumed_without_affecting_drag_copy() {
        let mut selection = VisibleTextSelectionState::default();
        selection.begin(3, 7);

        assert_eq!(selection.take_click_position(), Some((3, 7)));
        assert_eq!(selection.finish(), None);
    }

    #[test]
    fn double_click_selects_contiguous_non_blank_text() {
        const PREFIX: &str = "前缀 430 ";
        const CONTENT: &str = "个符号并行改动，不是本次单测修复新增的风险。";
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 1));
        buffer.set_string(0, 0, format!("{PREFIX}{CONTENT} 后缀"), Style::default());
        let mut selection = VisibleTextSelectionState::default();
        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&buffer));
        let clicked_column =
            u16::try_from(DisplayWidthText::width(PREFIX)).expect("测试列宽应适配终端");
        let first_click_at = Instant::now();

        assert!(!selection.select_content_run_on_double_click(0, clicked_column, first_click_at));
        assert!(selection.select_content_run_on_double_click(
            0,
            clicked_column,
            first_click_at + Duration::from_millis(450)
        ));

        assert_eq!(selection.finish(), Some(CONTENT.to_owned()));
    }

    #[test]
    fn double_click_selection_remains_highlighted_after_copy() {
        const CONTENT: &str = "复制后仍保持选中";
        let mut buffer = Buffer::empty(Rect::new(0, 0, 32, 1));
        buffer.set_string(0, 0, CONTENT, Style::default());
        let mut selection = VisibleTextSelectionState::default();
        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&buffer));
        let first_click_at = Instant::now();

        assert!(!selection.select_content_run_on_double_click(0, 2, first_click_at));
        assert!(selection.select_content_run_on_double_click(
            0,
            2,
            first_click_at + Duration::from_millis(100)
        ));
        let selected_range = selection.active_range();
        selection.retain_active_selection();

        assert_eq!(selection.finish(), Some(CONTENT.to_owned()));
        assert_eq!(selection.active_range(), selected_range);
    }

    #[test]
    fn changed_snapshot_clears_completed_selection() {
        let mut first_buffer = Buffer::empty(Rect::new(0, 0, 16, 1));
        first_buffer.set_string(0, 0, "first content", Style::default());
        let mut second_buffer = Buffer::empty(Rect::new(0, 0, 16, 1));
        second_buffer.set_string(0, 0, "new content", Style::default());
        let mut selection = VisibleTextSelectionState::default();
        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&first_buffer));
        let first_click_at = Instant::now();

        assert!(!selection.select_content_run_on_double_click(0, 1, first_click_at));
        assert!(selection.select_content_run_on_double_click(
            0,
            1,
            first_click_at + Duration::from_millis(100)
        ));
        selection.retain_active_selection();
        assert!(selection.finish().is_some());
        assert!(selection.active_range().is_some());

        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&second_buffer));

        assert!(selection.active_range().is_none());
    }

    #[test]
    fn clicks_outside_double_click_window_do_not_create_selection() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 16, 1));
        buffer.set_string(0, 0, "alpha beta", Style::default());
        let mut selection = VisibleTextSelectionState::default();
        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&buffer));
        let first_click_at = Instant::now();

        assert!(!selection.select_content_run_on_double_click(0, 1, first_click_at));
        assert!(!selection.select_content_run_on_double_click(
            0,
            1,
            first_click_at + Duration::from_millis(451)
        ));
        assert_eq!(selection.finish(), None);
    }

    #[test]
    fn double_click_on_blank_does_not_create_selection() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 16, 1));
        buffer.set_string(0, 0, "alpha beta", Style::default());
        let mut selection = VisibleTextSelectionState::default();
        selection.update_snapshot(VisibleTextSnapshot::from_buffer(&buffer));
        let first_click_at = Instant::now();

        assert!(!selection.select_content_run_on_double_click(0, 5, first_click_at));
        assert!(!selection.select_content_run_on_double_click(
            0,
            5,
            first_click_at + Duration::from_millis(100)
        ));
        assert_eq!(selection.finish(), None);
    }

    #[test]
    fn active_range_tracks_ordered_drag_coordinates() {
        let mut selection = VisibleTextSelectionState::default();

        selection.begin(3, 5);
        assert_eq!(selection.active_range(), None);
        assert!(selection.update(1, 2));

        assert_eq!(
            selection.active_range(),
            Some(super::VisibleTextSelectionRange {
                start_row: 1,
                start_column: 2,
                end_row: 3,
                end_column: 5,
            })
        );
    }
}
