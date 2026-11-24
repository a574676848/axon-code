//! Markdown 表格列宽分配与降级判定。

use super::{transcript_cells::display_width, transcript_markdown_table::TableCell};
const MIN_COLUMN_WIDTH: usize = 3;
const PREFERRED_EXPANSIVE_COLUMN_WIDTH: usize = 16;
const LONG_TOKEN_WIDTH: usize = 20;
const NARRATIVE_AVG_WORDS_PER_CELL: f64 = 4.0;
const NARRATIVE_AVG_CELL_WIDTH: f64 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TableColumnKind {
    Narrative,
    TokenHeavy,
    Compact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TableColumnMetrics {
    pub(super) max_width: usize,
    pub(super) header_token_width: usize,
    pub(super) body_token_width: usize,
    pub(super) kind: TableColumnKind,
}

pub(super) fn collect_column_metrics(
    header: &[TableCell],
    rows: &[Vec<TableCell>],
    column_count: usize,
) -> Vec<TableColumnMetrics> {
    let mut metrics = Vec::with_capacity(column_count);
    for column in 0..column_count {
        let header_cell = &header[column];
        let header_plain = header_cell.plain_text();
        let header_token_width = longest_token_width(&header_plain);
        let mut max_width = header_cell.width();
        let mut body_token_width = 0usize;
        let mut body_token_count = 0usize;
        let mut long_body_token_count = 0usize;
        let mut total_words = 0usize;
        let mut total_cells = 0usize;
        let mut total_cell_width = 0usize;

        for row in rows {
            let cell = &row[column];
            max_width = max_width.max(cell.width());
            let plain = cell.plain_text();
            body_token_width = body_token_width.max(longest_token_width(&plain));
            let word_count = plain.split_whitespace().count();
            if word_count > 0 {
                body_token_count += word_count;
                long_body_token_count += plain
                    .split_whitespace()
                    .filter(|token| display_width(token) >= LONG_TOKEN_WIDTH)
                    .count();
                total_words += word_count;
                total_cells += 1;
                total_cell_width += display_width(&plain);
            }
        }

        let avg_words_per_cell = if total_cells == 0 {
            header_plain.split_whitespace().count() as f64
        } else {
            total_words as f64 / total_cells as f64
        };
        let avg_cell_width = if total_cells == 0 {
            display_width(&header_plain) as f64
        } else {
            total_cell_width as f64 / total_cells as f64
        };
        let kind = if long_body_token_count > 0
            && long_body_token_count >= body_token_count.saturating_sub(long_body_token_count)
        {
            TableColumnKind::TokenHeavy
        } else if avg_words_per_cell >= NARRATIVE_AVG_WORDS_PER_CELL
            || avg_cell_width >= NARRATIVE_AVG_CELL_WIDTH
        {
            TableColumnKind::Narrative
        } else {
            TableColumnKind::Compact
        };

        metrics.push(TableColumnMetrics {
            max_width,
            header_token_width,
            body_token_width,
            kind,
        });
    }
    metrics
}

pub(super) fn allocate_column_widths(
    metrics: &[TableColumnMetrics],
    available_width: Option<usize>,
) -> Option<Vec<usize>> {
    let mut widths = metrics
        .iter()
        .map(|metric| metric.max_width.max(MIN_COLUMN_WIDTH))
        .collect::<Vec<_>>();
    let Some(max_width) = available_width else {
        return Some(widths);
    };
    if max_width < metrics.len().saturating_mul(MIN_COLUMN_WIDTH) {
        return Some(vec![MIN_COLUMN_WIDTH; metrics.len()]);
    }

    let mut floors = metrics
        .iter()
        .map(preferred_column_floor)
        .collect::<Vec<_>>();
    relax_floors_to_budget(&mut floors, metrics, max_width);

    let mut total_width = widths.iter().sum::<usize>();
    while total_width > max_width {
        let Some(index) = next_column_to_shrink(&widths, &floors, metrics) else {
            break;
        };
        widths[index] -= 1;
        total_width -= 1;
    }

    (total_width <= max_width).then_some(widths)
}

fn relax_floors_to_budget(floors: &mut [usize], metrics: &[TableColumnMetrics], max_width: usize) {
    let mut floor_total = floors.iter().sum::<usize>();
    while floor_total > max_width {
        let Some((index, _)) = floors
            .iter()
            .enumerate()
            .filter(|(_, floor)| **floor > MIN_COLUMN_WIDTH)
            .min_by_key(|(index, floor)| {
                (
                    column_shrink_priority(metrics[*index].kind),
                    usize::MAX.saturating_sub(**floor),
                )
            })
        else {
            break;
        };
        floors[index] -= 1;
        floor_total -= 1;
    }
}

fn preferred_column_floor(metrics: &TableColumnMetrics) -> usize {
    let target = match metrics.kind {
        TableColumnKind::Narrative | TableColumnKind::TokenHeavy => {
            PREFERRED_EXPANSIVE_COLUMN_WIDTH
        }
        TableColumnKind::Compact => metrics.header_token_width.max(
            metrics
                .body_token_width
                .min(PREFERRED_EXPANSIVE_COLUMN_WIDTH),
        ),
    };
    target.max(MIN_COLUMN_WIDTH).min(metrics.max_width)
}

fn next_column_to_shrink(
    widths: &[usize],
    floors: &[usize],
    metrics: &[TableColumnMetrics],
) -> Option<usize> {
    widths
        .iter()
        .enumerate()
        .filter(|(index, width)| **width > floors[*index])
        .min_by_key(|(index, width)| {
            let slack = width.saturating_sub(floors[*index]);
            (
                column_shrink_priority(metrics[*index].kind),
                usize::MAX.saturating_sub(slack),
            )
        })
        .map(|(index, _)| index)
}

fn column_shrink_priority(kind: TableColumnKind) -> usize {
    match kind {
        TableColumnKind::TokenHeavy => 0,
        TableColumnKind::Narrative => 1,
        TableColumnKind::Compact => 2,
    }
}

fn longest_token_width(text: &str) -> usize {
    text.split_whitespace()
        .map(display_width)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_heavy_column_shrinks_before_compact_status_column() {
        let header = vec![
            TableCell::from_plain_text("文件"),
            TableCell::from_plain_text("状态"),
        ];
        let rows = vec![vec![
            TableCell::from_plain_text("crates/axon-tui-runtime/src/widgets/transcript.rs"),
            TableCell::from_plain_text("已完成"),
        ]];
        let metrics = collect_column_metrics(&header, &rows, header.len());
        let widths = allocate_column_widths(&metrics, Some(24)).expect("宽度预算应可分配");

        assert!(widths[0] > widths[1], "路径列应吸收主要宽度：{widths:?}");
        assert!(
            widths[1] >= display_width("已完成"),
            "紧凑状态列不应被过度压缩：{widths:?}"
        );
    }

    #[test]
    fn allocation_keeps_minimum_grid_when_width_is_too_narrow() {
        let metrics = vec![
            TableColumnMetrics {
                max_width: 10,
                header_token_width: 2,
                body_token_width: 10,
                kind: TableColumnKind::TokenHeavy,
            };
            4
        ];

        assert_eq!(
            allocate_column_widths(&metrics, Some(11)),
            Some(vec![MIN_COLUMN_WIDTH; 4])
        );
    }
}
