//! Transcript Markdown 块级前缀状态。

use ratatui::{style::Style, text::Span};

const LIST_INDENT_WIDTH: usize = 4;
const BLOCKQUOTE_PREFIX: &str = "> ";
const UNORDERED_LIST_MARKER: &str = "- ";

#[derive(Debug, Clone)]
pub(super) struct ListContext {
    next_index: Option<u64>,
}

impl ListContext {
    pub(super) fn new(next_index: Option<u64>) -> Self {
        Self { next_index }
    }

    fn is_ordered(&self) -> bool {
        self.next_index.is_some()
    }

    fn marker(&mut self) -> String {
        match self.next_index {
            Some(index) => {
                self.next_index = Some(index.saturating_add(1));
                format!("{index}. ")
            }
            None => UNORDERED_LIST_MARKER.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct BlockPrefixStack {
    prefixes: Vec<BlockPrefixContext>,
}

impl BlockPrefixStack {
    pub(super) fn start_list_item(&mut self, lists: &mut [ListContext]) {
        let depth = lists.len().max(1);
        let is_ordered = lists.last().is_some_and(ListContext::is_ordered);
        let marker = lists
            .last_mut()
            .map(ListContext::marker)
            .unwrap_or_else(|| UNORDERED_LIST_MARKER.to_owned());
        let marker_column = depth.saturating_mul(LIST_INDENT_WIDTH).saturating_sub(3);
        let marker_prefix = if is_ordered {
            format!(
                "{:>width$}. ",
                marker.trim_end_matches(". "),
                width = marker_column
            )
        } else {
            format!("{}{}", " ".repeat(marker_column.saturating_sub(1)), marker)
        };
        let continuation_width = marker_column.saturating_add(if is_ordered { 2 } else { 1 });
        self.prefixes.push(BlockPrefixContext {
            kind: BlockPrefixKind::ListItem,
            ordered: is_ordered,
            marker: Some(marker_prefix),
            continuation: " ".repeat(continuation_width),
        });
    }

    pub(super) fn start_blockquote(&mut self) {
        self.prefixes.push(BlockPrefixContext {
            kind: BlockPrefixKind::BlockQuote,
            ordered: false,
            marker: Some(BLOCKQUOTE_PREFIX.to_owned()),
            continuation: BLOCKQUOTE_PREFIX.to_owned(),
        });
    }

    pub(super) fn end_block(&mut self) {
        self.prefixes.pop();
    }

    pub(super) fn current_spans(
        &mut self,
        ordered_list_marker_style: Style,
        unordered_list_marker_style: Style,
        blockquote_style: Style,
    ) -> Vec<Span<'static>> {
        let marker_index = self
            .prefixes
            .iter()
            .rposition(|context| context.marker.is_some());
        let marker_kind = marker_index.map(|index| self.prefixes[index].kind);
        let marker_is_list = marker_kind == Some(BlockPrefixKind::ListItem);
        let marker_is_blockquote = marker_kind == Some(BlockPrefixKind::BlockQuote);
        let last_list_index = self.prefixes.iter().rposition(BlockPrefixContext::is_list);

        self.prefixes
            .iter_mut()
            .enumerate()
            .filter_map(|(index, context)| {
                if Some(index) == marker_index {
                    let text = context.marker.take()?;
                    let style = context.style(
                        ordered_list_marker_style,
                        unordered_list_marker_style,
                        blockquote_style,
                    );
                    return Some(Span::styled(text, style));
                }
                if marker_is_list
                    && context.is_list()
                    && marker_index.is_some_and(|marker| marker > index)
                {
                    return None;
                }
                if marker_index.is_none() && context.is_list() && Some(index) != last_list_index {
                    return None;
                }
                let text = if marker_is_blockquote
                    && marker_index.is_some_and(|marker| marker > index)
                    && context.is_list()
                {
                    context
                        .marker
                        .take()
                        .unwrap_or_else(|| context.continuation.clone())
                } else {
                    context.continuation.clone()
                };
                let style = context.style(
                    ordered_list_marker_style,
                    unordered_list_marker_style,
                    blockquote_style,
                );
                Some(Span::styled(text, style))
            })
            .collect()
    }

    pub(super) fn has_active_blockquote(&self) -> bool {
        self.prefixes
            .iter()
            .any(|context| context.kind == BlockPrefixKind::BlockQuote)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockPrefixKind {
    ListItem,
    BlockQuote,
}

#[derive(Debug, Clone)]
struct BlockPrefixContext {
    kind: BlockPrefixKind,
    ordered: bool,
    marker: Option<String>,
    continuation: String,
}

impl BlockPrefixContext {
    fn is_list(&self) -> bool {
        self.kind == BlockPrefixKind::ListItem
    }

    fn style(
        &self,
        ordered_list_marker_style: Style,
        unordered_list_marker_style: Style,
        blockquote_style: Style,
    ) -> Style {
        match self.kind {
            BlockPrefixKind::ListItem if self.ordered => ordered_list_marker_style,
            BlockPrefixKind::ListItem => unordered_list_marker_style,
            BlockPrefixKind::BlockQuote => blockquote_style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    fn visible_text(spans: Vec<Span<'static>>) -> String {
        Line::from(spans)
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn list_item_prefix_uses_marker_once_then_continuation() {
        let mut lists = vec![ListContext::new(None)];
        let mut prefixes = BlockPrefixStack::default();

        prefixes.start_list_item(&mut lists);

        assert_eq!(
            visible_text(prefixes.current_spans(
                Style::default(),
                Style::default(),
                Style::default(),
            )),
            "- "
        );
        assert_eq!(
            visible_text(prefixes.current_spans(
                Style::default(),
                Style::default(),
                Style::default(),
            )),
            "  "
        );
    }

    #[test]
    fn nested_ordered_list_prefix_accounts_for_depth() {
        let mut lists = vec![ListContext::new(None), ListContext::new(Some(3))];
        let mut prefixes = BlockPrefixStack::default();

        prefixes.start_list_item(&mut lists);

        assert_eq!(
            visible_text(prefixes.current_spans(
                Style::default(),
                Style::default(),
                Style::default(),
            )),
            "    3. "
        );
    }
}
