//! Transcript Markdown 表格渲染。

use pulldown_cmark::Alignment;
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{
    transcript_cells::display_width,
    transcript_markdown_table_layout::{allocate_column_widths, collect_column_metrics},
    transcript_markdown_wrap::wrap_line_with_prefix,
};
use crate::widgets::terminal_hyperlinks::{HyperlinkLine, TerminalHyperlink};

const TABLE_HEADER_SEPARATOR_CHAR: char = '━';
const TABLE_BODY_SEPARATOR_CHAR: char = '─';
const TABLE_TOP_LEFT: char = '┌';
const TABLE_TOP_JOIN: char = '┬';
const TABLE_TOP_RIGHT: char = '┐';
const TABLE_MIDDLE_LEFT: char = '├';
const TABLE_MIDDLE_JOIN: char = '┼';
const TABLE_MIDDLE_RIGHT: char = '┤';
const TABLE_BOTTOM_LEFT: char = '└';
const TABLE_BOTTOM_JOIN: char = '┴';
const TABLE_BOTTOM_RIGHT: char = '┘';
const TABLE_VERTICAL: char = '│';
const TABLE_CELL_PADDING: usize = 1;
const TABLE_LEADING_PADDING: usize = 1;

#[derive(Clone, Debug, Default)]
pub(super) struct TableCell {
    lines: Vec<HyperlinkLine>,
}

#[derive(Clone, Debug, Default)]
struct TableRow {
    cells: Vec<TableCell>,
    has_table_syntax: bool,
}

#[derive(Debug, Default)]
pub(super) struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<TableRow>,
    current_row: Vec<TableCell>,
    current_row_has_table_syntax: bool,
    current_cell: Option<TableCell>,
}

impl TableCell {
    pub(super) fn plain_text(&self) -> String {
        self.lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned()
    }

    pub(super) fn width(&self) -> usize {
        self.lines
            .iter()
            .map(|line| display_width(&line_text(line)))
            .max()
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(super) fn from_plain_text(text: &str) -> Self {
        let mut cell = Self::default();
        cell.push_span(Span::raw(text.to_owned()), None);
        cell
    }

    fn is_blank(&self) -> bool {
        self.plain_text().trim().is_empty()
    }

    fn push_span(&mut self, span: Span<'static>, destination: Option<String>) {
        self.ensure_line();
        if let Some(line) = self.lines.last_mut() {
            line.push_span(span, destination);
        }
    }

    fn push_line_break(&mut self) {
        self.lines.push(HyperlinkLine::default());
    }

    fn ensure_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(HyperlinkLine::default());
        }
    }
}

impl TableState {
    pub(super) fn new(alignments: Vec<Alignment>) -> Self {
        Self {
            alignments,
            ..Self::default()
        }
    }

    pub(super) fn in_cell(&self) -> bool {
        self.current_cell.is_some()
    }

    pub(super) fn push_span(&mut self, span: Span<'static>, destination: Option<String>) {
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_span(span, destination);
        }
    }

    pub(super) fn push_line_break(&mut self) {
        if let Some(cell) = self.current_cell.as_mut() {
            cell.push_line_break();
        }
    }

    pub(super) fn start_row(&mut self, has_table_syntax: bool) {
        self.current_row.clear();
        self.current_row_has_table_syntax = has_table_syntax;
    }

    pub(super) fn start_cell(&mut self) {
        self.current_cell = Some(TableCell::default());
    }

    pub(super) fn end_cell(&mut self) {
        let Some(cell) = self.current_cell.take() else {
            return;
        };
        self.current_row.push(cell);
    }

    pub(super) fn end_row(&mut self) {
        if !self.current_row.is_empty() {
            self.rows.push(TableRow {
                cells: std::mem::take(&mut self.current_row),
                has_table_syntax: self.current_row_has_table_syntax,
            });
            self.current_row_has_table_syntax = false;
        }
    }

    pub(super) fn render_lines(&self, marker_style: Style, width: usize) -> Vec<HyperlinkLine> {
        render_table_lines(&self.rows, &self.alignments, marker_style, width)
    }
}

fn render_table_lines(
    rows: &[TableRow],
    alignments: &[Alignment],
    marker_style: Style,
    width: usize,
) -> Vec<HyperlinkLine> {
    if rows.is_empty() {
        return Vec::new();
    }
    let (mut table_rows, spillover_lines) = table_rows_without_spillover(rows, alignments.len());
    if table_rows.is_empty() {
        return spillover_lines;
    }
    let column_count = table_rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(alignments.len())
        .max(alignments.len());
    normalize_table_rows(&mut table_rows, column_count);
    let (header, body_rows) = table_rows.split_first().expect("表格行已判空");
    let metrics = collect_column_metrics(header, body_rows, column_count);
    let available_width = Some(available_grid_content_width(width, column_count));
    let widths =
        allocate_column_widths(&metrics, available_width).expect("表格列宽分配必须保留最小田字格");
    let mut lines = render_grid_table_lines(&table_rows, alignments, marker_style, &widths);
    lines.extend(spillover_lines);
    lines
}

fn normalize_table_rows(rows: &mut [Vec<TableCell>], column_count: usize) {
    for row in rows {
        row.truncate(column_count);
        row.resize(column_count, TableCell::default());
    }
}

fn table_rows_without_spillover(
    rows: &[TableRow],
    column_count: usize,
) -> (Vec<Vec<TableCell>>, Vec<HyperlinkLine>) {
    let Some(header) = rows.first() else {
        return (Vec::new(), Vec::new());
    };
    let mut table_rows = vec![header.cells.clone()];
    let mut spillover_lines = Vec::new();
    for row in &rows[1..] {
        if column_count > 1 && is_spillover_row(row) {
            spillover_lines.extend(
                first_non_empty_only_cell(&row.cells)
                    .into_iter()
                    .flat_map(|cell| cell.lines.clone()),
            );
        } else {
            table_rows.push(row.cells.clone());
        }
    }
    (table_rows, spillover_lines)
}

fn render_grid_table_lines(
    rows: &[Vec<TableCell>],
    alignments: &[Alignment],
    marker_style: Style,
    widths: &[usize],
) -> Vec<HyperlinkLine> {
    let mut lines = vec![render_table_border(
        widths,
        marker_style,
        TABLE_TOP_LEFT,
        TABLE_TOP_JOIN,
        TABLE_TOP_RIGHT,
        TABLE_HEADER_SEPARATOR_CHAR,
    )];
    for (index, row) in rows.iter().enumerate() {
        lines.extend(render_table_row(
            row,
            widths,
            alignments,
            marker_style,
            index == 0,
        ));
        if index == 0 {
            lines.push(render_table_border(
                widths,
                marker_style,
                TABLE_MIDDLE_LEFT,
                TABLE_MIDDLE_JOIN,
                TABLE_MIDDLE_RIGHT,
                TABLE_HEADER_SEPARATOR_CHAR,
            ));
        } else if index + 1 < rows.len() {
            lines.push(render_table_border(
                widths,
                marker_style,
                TABLE_MIDDLE_LEFT,
                TABLE_MIDDLE_JOIN,
                TABLE_MIDDLE_RIGHT,
                TABLE_BODY_SEPARATOR_CHAR,
            ));
        }
    }
    lines.push(render_table_border(
        widths,
        marker_style,
        TABLE_BOTTOM_LEFT,
        TABLE_BOTTOM_JOIN,
        TABLE_BOTTOM_RIGHT,
        TABLE_BODY_SEPARATOR_CHAR,
    ));
    lines
}

fn render_table_row(
    row: &[TableCell],
    widths: &[usize],
    alignments: &[Alignment],
    marker_style: Style,
    header: bool,
) -> Vec<HyperlinkLine> {
    let wrapped_cells = row
        .iter()
        .zip(widths)
        .map(|(cell, width)| wrap_cell_lines(cell, *width))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut rows = Vec::with_capacity(row_height);

    for row_line in 0..row_height {
        let mut line = HyperlinkLine::new(Line::from(Span::styled(
            format!("{}{}", " ".repeat(TABLE_LEADING_PADDING), TABLE_VERTICAL),
            marker_style,
        )));
        for (column, width) in widths.iter().enumerate() {
            append_wrapped_cell_line(
                &mut line,
                wrapped_cells[column].get(row_line),
                *width,
                alignment_at(alignments, column),
                header,
            );
            line.push_span(Span::styled(TABLE_VERTICAL.to_string(), marker_style), None);
        }
        rows.push(line);
    }

    rows
}

fn append_wrapped_cell_line(
    output: &mut HyperlinkLine,
    cell_line: Option<&HyperlinkLine>,
    width: usize,
    alignment: Alignment,
    header: bool,
) {
    output.push_span(Span::raw(" ".repeat(TABLE_CELL_PADDING)), None);
    let cell_width = cell_line.map(HyperlinkLine::width).unwrap_or(0);
    let padding = width.saturating_sub(cell_width);
    let (left_padding, right_padding) = aligned_padding(padding, alignment);
    if left_padding > 0 {
        output.push_span(Span::raw(" ".repeat(left_padding)), None);
    }
    if let Some(cell_line) = cell_line {
        let before = output.width();
        for span in &cell_line.line.spans {
            let mut span = span.clone();
            if header {
                span.style = span.style.add_modifier(Modifier::BOLD);
            }
            output.line.push_span(span);
        }
        output.hyperlinks.extend(
            cell_line
                .hyperlinks
                .iter()
                .map(|link| shifted_link(link, before)),
        );
    }
    if right_padding > 0 {
        output.push_span(Span::raw(" ".repeat(right_padding)), None);
    }
    output.push_span(Span::raw(" ".repeat(TABLE_CELL_PADDING)), None);
}

fn alignment_at(alignments: &[Alignment], index: usize) -> Alignment {
    alignments.get(index).copied().unwrap_or(Alignment::None)
}

fn aligned_padding(padding: usize, alignment: Alignment) -> (usize, usize) {
    match alignment {
        Alignment::Right => (padding, 0),
        Alignment::Center => (padding / 2, padding.saturating_sub(padding / 2)),
        Alignment::Left | Alignment::None => (0, padding),
    }
}

fn shifted_link(link: &TerminalHyperlink, offset: usize) -> TerminalHyperlink {
    TerminalHyperlink {
        columns: link.columns.start + offset..link.columns.end + offset,
        destination: link.destination.clone(),
    }
}

fn render_table_border(
    widths: &[usize],
    marker_style: Style,
    left: char,
    join: char,
    right: char,
    separator: char,
) -> HyperlinkLine {
    let mut text = String::new();
    text.push_str(&" ".repeat(TABLE_LEADING_PADDING));
    text.push(left);
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            text.push(join);
        }
        text.push_str(&separator.to_string().repeat(width + TABLE_CELL_PADDING * 2));
    }
    text.push(right);
    HyperlinkLine::from(Line::from(Span::styled(text, marker_style)))
}

fn available_grid_content_width(width: usize, column_count: usize) -> usize {
    if column_count == 0 {
        return width;
    }
    let reserved = TABLE_LEADING_PADDING
        .saturating_add(2)
        .saturating_add(column_count.saturating_sub(1))
        .saturating_add(
            TABLE_CELL_PADDING
                .saturating_mul(2)
                .saturating_mul(column_count),
        );
    width.saturating_sub(reserved)
}

fn wrap_cell_lines(cell: &TableCell, width: usize) -> Vec<HyperlinkLine> {
    if cell.lines.is_empty() {
        return vec![HyperlinkLine::default()];
    }
    let wrapped = cell
        .lines
        .iter()
        .flat_map(|line| wrap_line_with_prefix(line.clone(), "", "", Style::default(), width))
        .collect::<Vec<_>>();
    if wrapped.is_empty() {
        vec![HyperlinkLine::default()]
    } else {
        wrapped
    }
}

fn is_spillover_row(row: &TableRow) -> bool {
    !row.has_table_syntax && first_non_empty_only_cell(&row.cells).is_some()
}

fn first_non_empty_only_cell(cells: &[TableCell]) -> Option<&TableCell> {
    let mut non_empty = cells.iter().filter(|cell| !cell.is_blank());
    let first = non_empty.next()?;
    non_empty.next().is_none().then_some(first)
}

fn line_text(line: &HyperlinkLine) -> String {
    line.line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
