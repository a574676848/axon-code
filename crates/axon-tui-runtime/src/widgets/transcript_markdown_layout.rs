//! Markdown renderer 的逻辑行布局语义。

use super::{
    terminal_hyperlinks::HyperlinkLine, transcript_cells::display_width,
    transcript_markdown_helpers::line_is_blank,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkdownLineKind {
    Reflow,
    Preformatted,
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MarkdownLineLayout {
    Reflow { continuation_indent: usize },
    Preformatted { continuation_indent: usize },
    Fixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RenderedMarkdownLine {
    pub(super) content: HyperlinkLine,
    pub(super) layout: MarkdownLineLayout,
}

impl RenderedMarkdownLine {
    pub(super) fn new(
        content: HyperlinkLine,
        kind: MarkdownLineKind,
        block_prefix_width: usize,
    ) -> Self {
        let layout = match kind {
            MarkdownLineKind::Reflow => MarkdownLineLayout::Reflow {
                continuation_indent: block_prefix_width,
            },
            MarkdownLineKind::Preformatted => MarkdownLineLayout::Preformatted {
                continuation_indent: block_prefix_width
                    .saturating_add(content_leading_width(&content)),
            },
            MarkdownLineKind::Fixed => MarkdownLineLayout::Fixed,
        };
        Self { content, layout }
    }

    pub(super) fn blank() -> Self {
        Self::new(HyperlinkLine::default(), MarkdownLineKind::Reflow, 0)
    }

    pub(super) fn is_blank(&self) -> bool {
        line_is_blank(&self.content)
    }
}

fn content_leading_width(content: &HyperlinkLine) -> usize {
    let leading = content
        .line
        .spans
        .iter()
        .flat_map(|span| span.content.chars())
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    display_width(&leading)
}
