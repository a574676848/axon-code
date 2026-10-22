//! Markdown 渲染器共享的行级辅助。

use super::terminal_hyperlinks::{HyperlinkLine, TerminalHyperlink};

pub(super) trait HyperlinkLineExt {
    fn merge_hyperlinks(self, hyperlinks: Vec<TerminalHyperlink>) -> Self;
}

impl HyperlinkLineExt for HyperlinkLine {
    fn merge_hyperlinks(mut self, hyperlinks: Vec<TerminalHyperlink>) -> Self {
        self.hyperlinks.extend(hyperlinks);
        self
    }
}

pub(super) fn line_is_blank(line: &HyperlinkLine) -> bool {
    line.line
        .spans
        .iter()
        .all(|span| span.content.trim().is_empty())
}

pub(super) fn code_block_language_token(info: &str) -> Option<&str> {
    info.split([',', ' ', '\t'])
        .next()
        .map(str::trim)
        .filter(|language| !language.is_empty())
}
