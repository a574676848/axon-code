//! Transcript 终端 hyperlink 元数据与 OSC 8 注入。
//!
//! hyperlink 由 cell 渲染阶段显式携带，通过独立 screen surface 记录物理坐标；
//! OSC 8 仅在 backend 输出边界编码，不进入可见 Buffer，也不从渲染文本反向扫描 URL。

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use crate::surface::display_width::DisplayWidthText;
use ratatui::{
    buffer::{Buffer, Cell},
    layout::{Position, Rect},
    text::{Line, Span},
};

#[cfg(test)]
use ratatui::style::Style;

const OSC8_OPEN_PREFIX: &str = "\x1b]8;;";
const OSC8_CLOSE: &str = "\x1b]8;;\x07";
const OSC8_SEPARATOR: char = '\x07';
const HTTP_SCHEME: &str = "http://";
const HTTPS_SCHEME: &str = "https://";
const FILE_SCHEME: &str = "file://";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalHyperlink {
    pub(crate) columns: Range<usize>,
    pub(crate) destination: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct HyperlinkLine {
    pub(crate) line: Line<'static>,
    pub(crate) hyperlinks: Vec<TerminalHyperlink>,
}

/// 当前帧屏幕坐标到 hyperlink 目标的独立投影。
///
/// 可见字符始终留在 ratatui `Buffer`，OSC 8 只在 backend 输出边界编码，避免控制序列
/// 参与 cell width、diff、复制和文本选择计算。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TerminalHyperlinkSurface {
    destinations: BTreeMap<(u16, u16), String>,
}

impl TerminalHyperlinkSurface {
    pub(crate) fn clear(&mut self) {
        self.destinations.clear();
    }

    pub(crate) fn insert(&mut self, position: Position, destination: &str) {
        let Some(destination) = safe_destination(destination) else {
            return;
        };
        self.destinations
            .insert((position.y, position.x), destination);
    }

    fn destination(&self, x: u16, y: u16) -> Option<&str> {
        self.destinations.get(&(y, x)).map(String::as_str)
    }

    fn changed_positions(&self, other: &Self) -> BTreeSet<(u16, u16)> {
        self.destinations
            .keys()
            .chain(other.destinations.keys())
            .filter(|position| self.destinations.get(position) != other.destinations.get(position))
            .copied()
            .collect()
    }
}

impl HyperlinkLine {
    pub(crate) fn new(line: Line<'static>) -> Self {
        Self {
            line,
            hyperlinks: Vec::new(),
        }
    }

    pub(crate) fn push_span(&mut self, span: Span<'static>, destination: Option<String>) {
        let start = self.width();
        let end = start.saturating_add(display_width(span.content.as_ref()));
        self.line.push_span(span);
        if end > start
            && let Some(destination) = destination.as_deref().and_then(safe_destination)
        {
            self.hyperlinks.push(TerminalHyperlink {
                columns: start..end,
                destination,
            });
        }
    }

    pub(crate) fn width(&self) -> usize {
        self.line
            .spans
            .iter()
            .map(|span| display_width(span.content.as_ref()))
            .sum()
    }
}

impl From<Line<'static>> for HyperlinkLine {
    fn from(line: Line<'static>) -> Self {
        Self::new(line)
    }
}

impl From<String> for HyperlinkLine {
    fn from(text: String) -> Self {
        Self::new(Line::from(text))
    }
}

impl From<&'static str> for HyperlinkLine {
    fn from(text: &'static str) -> Self {
        Self::new(Line::from(text))
    }
}

pub(crate) fn visible_lines(lines: &[HyperlinkLine]) -> Vec<Line<'static>> {
    lines.iter().map(|line| line.line.clone()).collect()
}

pub(crate) fn plain_hyperlink_lines(lines: Vec<Line<'static>>) -> Vec<HyperlinkLine> {
    lines.into_iter().map(HyperlinkLine::new).collect()
}

pub(crate) fn prefix_hyperlink_lines(
    lines: Vec<HyperlinkLine>,
    initial_prefix: Span<'static>,
    continuation_prefix: Span<'static>,
) -> Vec<HyperlinkLine> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            if line
                .line
                .spans
                .iter()
                .all(|span| span.content.chars().all(char::is_whitespace))
            {
                line.line = Line::default().style(line.line.style);
                line.hyperlinks.clear();
                return line;
            }
            let prefix = if index == 0 {
                initial_prefix.clone()
            } else {
                continuation_prefix.clone()
            };
            let prefix_width = display_width(prefix.content.as_ref());
            line.line.spans.insert(0, prefix);
            line.hyperlinks.iter_mut().for_each(|link| {
                link.columns = link.columns.start + prefix_width..link.columns.end + prefix_width;
            });
            line
        })
        .collect()
}

pub(crate) fn annotate_web_urls_in_line(line: Line<'static>) -> HyperlinkLine {
    let text = line_text(&line);
    let mut out = HyperlinkLine::new(line);
    out.hyperlinks = web_links_in_text(&text);
    out
}

pub(crate) fn web_links_in_text(text: &str) -> Vec<TerminalHyperlink> {
    let mut links = Vec::new();
    let mut search_from = 0usize;
    for raw_token in text.split_ascii_whitespace() {
        let Some(relative_start) = text[search_from..].find(raw_token) else {
            continue;
        };
        let raw_start = search_from + relative_start;
        search_from = raw_start + raw_token.len();
        let trimmed_start = raw_token
            .find(|ch: char| !is_leading_punctuation(ch))
            .unwrap_or(raw_token.len());
        let trimmed_end = trailing_url_end(&raw_token[trimmed_start..]) + trimmed_start;
        if trimmed_start >= trimmed_end {
            continue;
        }
        let candidate = &raw_token[trimmed_start..trimmed_end];
        let Some(destination) = web_destination(candidate) else {
            continue;
        };
        let start = display_width(&text[..raw_start + trimmed_start]);
        let end = start.saturating_add(display_width(candidate));
        links.push(TerminalHyperlink {
            columns: start..end,
            destination,
        });
    }
    links
}

pub(crate) fn web_destination(destination: &str) -> Option<String> {
    let safe = sanitize_destination(destination);
    let is_web = safe.starts_with(HTTP_SCHEME) || safe.starts_with(HTTPS_SCHEME);
    let has_host = safe
        .split_once("://")
        .map(|(_, rest)| !rest.split('/').next().unwrap_or_default().is_empty())
        .unwrap_or(false);
    (is_web && has_host).then_some(safe)
}

pub(crate) fn record_buffer_hyperlinks(
    surface: &mut TerminalHyperlinkSurface,
    area: Rect,
    lines: &[HyperlinkLine],
    scroll_rows: usize,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    for (visible_row, line) in lines
        .iter()
        .skip(scroll_rows)
        .take(usize::from(area.height))
        .enumerate()
    {
        let y = area
            .y
            .saturating_add(u16::try_from(visible_row).unwrap_or(u16::MAX));
        for hyperlink in &line.hyperlinks {
            let start = hyperlink.columns.start.min(usize::from(area.width));
            let end = hyperlink.columns.end.min(usize::from(area.width));
            for column in start..end {
                let x = area
                    .x
                    .saturating_add(u16::try_from(column).unwrap_or(u16::MAX));
                surface.insert(Position::new(x, y), &hyperlink.destination);
            }
        }
    }
}

pub(crate) fn terminal_frame_diff(
    previous: &Buffer,
    current: &Buffer,
    previous_hyperlinks: &TerminalHyperlinkSurface,
    current_hyperlinks: &TerminalHyperlinkSurface,
    supports_osc8: bool,
) -> Vec<(u16, u16, Cell)> {
    let mut updates = previous
        .diff_iter(current)
        .map(|(x, y, cell)| ((y, x), cell.clone()))
        .collect::<BTreeMap<_, _>>();
    insert_removed_wide_glyph_trailing_updates(previous, current, &mut updates);

    if supports_osc8 {
        for (y, x) in previous_hyperlinks.changed_positions(current_hyperlinks) {
            if let Some(cell) = current.cell(Position::new(x, y)) {
                updates.insert((y, x), cell.clone());
            }
        }
    }

    updates
        .into_iter()
        .map(|((y, x), mut cell)| {
            if supports_osc8 && let Some(destination) = current_hyperlinks.destination(x, y) {
                let symbol = osc8_hyperlink(destination, cell.symbol());
                cell.set_symbol(&symbol);
            }
            (x, y, cell)
        })
        .collect()
}

fn insert_removed_wide_glyph_trailing_updates(
    previous: &Buffer,
    current: &Buffer,
    updates: &mut BTreeMap<(u16, u16), Cell>,
) {
    let row_width = usize::from(previous.area.width);
    if row_width == 0 {
        return;
    }
    let cell_count = previous.content().len().min(current.content().len());
    for (index, previous_cell) in previous.content()[..cell_count].iter().enumerate() {
        let previous_width = display_width(previous_cell.symbol());
        let current_width = display_width(current.content()[index].symbol()).max(1);
        if previous_width <= current_width {
            continue;
        }
        let row_end = (index / row_width + 1)
            .saturating_mul(row_width)
            .min(cell_count);
        let trailing_end = index.saturating_add(previous_width).min(row_end);
        for trailing_index in index.saturating_add(current_width)..trailing_end {
            let column =
                u16::try_from(trailing_index % row_width).expect("buffer 列索引必须适配 u16");
            let row = u16::try_from(trailing_index / row_width).expect("buffer 行索引必须适配 u16");
            let x = previous.area.x.saturating_add(column);
            let y = previous.area.y.saturating_add(row);
            updates.insert((y, x), current.content()[trailing_index].clone());
        }
    }
}

fn osc8_hyperlink(destination: &str, text: &str) -> String {
    let Some(destination) = safe_destination(destination) else {
        return text.to_owned();
    };
    format!("{OSC8_OPEN_PREFIX}{destination}{OSC8_SEPARATOR}{text}{OSC8_CLOSE}")
}

fn safe_destination(destination: &str) -> Option<String> {
    if destination.starts_with(FILE_SCHEME) {
        return Some(sanitize_destination(destination));
    }
    web_destination(destination)
}

fn sanitize_destination(destination: &str) -> String {
    destination
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn display_width(text: &str) -> usize {
    DisplayWidthText::width(text)
}

fn is_leading_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | '.' | ';' | '!' | '\'' | '"'
    )
}

fn trailing_url_end(candidate: &str) -> usize {
    let mut end = candidate.len();
    while end > 0 {
        let remaining = &candidate[..end];
        let Some(ch) = remaining.chars().next_back() else {
            break;
        };
        let should_trim = matches!(ch, ',' | '.' | ';' | '!' | '\'' | '"')
            || matches!(ch, ')' | ']' | '}' | '>')
                && has_unmatched_closing_delimiter(remaining, ch);
        if !should_trim {
            break;
        }
        end -= ch.len_utf8();
    }
    end
}

fn has_unmatched_closing_delimiter(candidate: &str, closing: char) -> bool {
    let opening = match closing {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        '>' => '<',
        _ => return false,
    };
    candidate.chars().filter(|ch| *ch == closing).count()
        > candidate.chars().filter(|ch| *ch == opening).count()
}

#[cfg(test)]
pub(crate) fn strip_osc8(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut index = 0usize;
    let bytes = text.as_bytes();
    while index < bytes.len() {
        if bytes[index..].starts_with(OSC8_OPEN_PREFIX.as_bytes()) {
            index += OSC8_OPEN_PREFIX.len();
            while index < bytes.len() {
                if bytes[index] == b'\x07' {
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        stripped.push(ch);
        index += ch.len_utf8();
    }
    stripped
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    #[test]
    fn annotates_punctuated_web_url_columns_without_buffer_scan() {
        let line = annotate_web_urls_in_line(Line::from("See (https://example.com/a)."));

        assert_eq!(
            line.hyperlinks,
            vec![TerminalHyperlink {
                columns: 5..26,
                destination: "https://example.com/a".to_owned(),
            }]
        );
    }

    #[test]
    fn hyperlink_surface_keeps_control_sequences_out_of_visible_buffer() {
        let destination = "https://example.com/a";
        let line = HyperlinkLine {
            line: Line::from(format!("go {destination}")),
            hyperlinks: vec![TerminalHyperlink {
                columns: 3..3 + display_width(destination),
                destination: destination.to_owned(),
            }],
        };
        let area = Rect::new(0, 0, 32, 1);
        let mut buf = Buffer::empty(area);
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(line.line.clone()),
            area,
            &mut buf,
        );
        let mut surface = TerminalHyperlinkSurface::default();

        record_buffer_hyperlinks(&mut surface, area, &[line], 0);

        let visible = area
            .positions()
            .map(|position| buf[position].symbol())
            .collect::<String>();
        assert!(visible.contains(destination));
        assert!(!visible.contains(OSC8_OPEN_PREFIX));
        assert_eq!(
            surface.destination(3, 0),
            Some(destination),
            "链接目标应独立记录在 hyperlink surface"
        );
    }

    #[test]
    fn terminal_diff_clears_all_cells_covered_by_removed_default_cjk() {
        let area = Rect::new(0, 0, 8, 1);
        let mut previous = Buffer::empty(area);
        previous.set_string(2, 0, "修复", Style::default());
        let current = Buffer::empty(area);

        let updates = terminal_frame_diff(
            &previous,
            &current,
            &TerminalHyperlinkSurface::default(),
            &TerminalHyperlinkSurface::default(),
            false,
        );

        for x in 2..6 {
            assert!(
                updates
                    .iter()
                    .any(|(update_x, y, cell)| *update_x == x && *y == 0 && cell.symbol() == " "),
                "宽字符覆盖过的列 {x} 必须显式清空：{updates:?}"
            );
        }
    }

    #[test]
    fn terminal_diff_does_not_clear_current_wide_glyph_trailing_cell() {
        let area = Rect::new(0, 0, 4, 1);
        let mut previous = Buffer::empty(area);
        previous.set_string(0, 0, "你好", Style::default());
        let mut current = Buffer::empty(area);
        current.set_string(0, 0, "a好", Style::default());

        let updates = terminal_frame_diff(
            &previous,
            &current,
            &TerminalHyperlinkSurface::default(),
            &TerminalHyperlinkSurface::default(),
            false,
        );

        assert!(
            !updates.iter().any(|(x, y, _)| *x == 2 && *y == 0),
            "当前宽字符的 trailing cell 不得被空格覆盖：{updates:?}"
        );
    }

    #[test]
    fn terminal_diff_emits_complete_url_without_skipping_following_cells() {
        let destination = "https://github.com/volcengine/OpenViking";
        let text = format!("{destination} tail");
        let line = HyperlinkLine {
            line: Line::from(text.clone()),
            hyperlinks: vec![TerminalHyperlink {
                columns: 0..display_width(destination),
                destination: destination.to_owned(),
            }],
        };
        let area = Rect::new(0, 0, 80, 1);
        let previous = Buffer::empty(area);
        let mut current = Buffer::empty(area);
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(line.line.clone()),
            area,
            &mut current,
        );
        let mut hyperlinks = TerminalHyperlinkSurface::default();
        record_buffer_hyperlinks(&mut hyperlinks, area, &[line], 0);

        let updates = terminal_frame_diff(
            &previous,
            &current,
            &TerminalHyperlinkSurface::default(),
            &hyperlinks,
            true,
        );

        let linked = updates
            .iter()
            .filter(|(_, _, cell)| cell.symbol().contains(OSC8_OPEN_PREFIX))
            .map(|(_, _, cell)| strip_osc8(cell.symbol()))
            .collect::<String>();
        let following_text_x = u16::try_from(display_width(destination))
            .expect("测试 URL 宽度应在 u16 范围内")
            .saturating_add(1);
        assert_eq!(linked, destination);
        assert!(
            updates
                .iter()
                .any(|(x, _, cell)| *x == following_text_x && cell.symbol() == "t"),
            "URL 后的普通文本必须按真实坐标继续输出"
        );
        assert!(
            !current
                .content()
                .iter()
                .any(|cell| cell.symbol().contains(OSC8_OPEN_PREFIX))
        );
    }

    #[test]
    fn terminal_diff_redraws_when_only_hyperlink_destination_changes() {
        let text = "link";
        let previous = Buffer::with_lines([text]);
        let current = previous.clone();
        let mut previous_hyperlinks = TerminalHyperlinkSurface::default();
        let mut current_hyperlinks = TerminalHyperlinkSurface::default();
        let text_width = u16::try_from(display_width(text)).expect("测试文本宽度应在 u16 范围内");
        for x in 0..text_width {
            previous_hyperlinks.insert(Position::new(x, 0), "https://example.com/old");
            current_hyperlinks.insert(Position::new(x, 0), "https://example.com/new");
        }

        let updates = terminal_frame_diff(
            &previous,
            &current,
            &previous_hyperlinks,
            &current_hyperlinks,
            true,
        );

        assert_eq!(updates.len(), display_width(text));
        assert!(
            updates
                .iter()
                .all(|(_, _, cell)| { cell.symbol().contains("https://example.com/new") })
        );

        let removed = terminal_frame_diff(
            &previous,
            &current,
            &current_hyperlinks,
            &TerminalHyperlinkSurface::default(),
            true,
        );
        assert_eq!(removed.len(), display_width(text));
        assert!(
            removed
                .iter()
                .all(|(_, _, cell)| !cell.symbol().contains(OSC8_OPEN_PREFIX))
        );
    }

    #[test]
    fn hyperlink_surface_does_not_reflow_renderer_physical_lines() {
        let destination = "https://github.com/volcengine/OpenViking";
        let prefix_width = display_width("clone ");
        let line = HyperlinkLine {
            line: Line::from(format!("clone {destination} tail")),
            hyperlinks: vec![TerminalHyperlink {
                columns: prefix_width..prefix_width + display_width(destination),
                destination: destination.to_owned(),
            }],
        };
        let area = Rect::new(0, 0, 16, 4);
        let mut visible = Buffer::empty(area);
        ratatui::widgets::Widget::render(
            ratatui::widgets::Paragraph::new(line.line.clone()),
            area,
            &mut visible,
        );
        let mut surface = TerminalHyperlinkSurface::default();

        record_buffer_hyperlinks(&mut surface, area, &[line], 0);

        assert_eq!(surface.destination(6, 0), Some(destination));
        assert_eq!(
            surface.destination(0, 1),
            None,
            "viewport 不得把 renderer 的单个物理行再次换行"
        );
    }
}
