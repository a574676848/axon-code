//! Unicode 显示宽度、截断与硬换行基础工具。

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) const TAB_DISPLAY_WIDTH: usize = 4;
const ELLIPSIS: &str = "…";

pub(crate) struct DisplayWidthText;

impl DisplayWidthText {
    pub(crate) fn width(value: &str) -> usize {
        Self::width_with_tab(value, TAB_DISPLAY_WIDTH)
    }

    pub(crate) fn width_with_tab(value: &str, tab_width: usize) -> usize {
        value
            .graphemes(true)
            .map(|grapheme| Self::grapheme_width(grapheme, tab_width))
            .sum()
    }

    pub(crate) fn ellipsize(value: &str, max_width: usize) -> String {
        let trimmed = value.trim();
        if Self::width(trimmed) <= max_width {
            return trimmed.to_owned();
        }
        let ellipsis_width = Self::width(ELLIPSIS);
        if max_width <= ellipsis_width {
            return ELLIPSIS.to_owned();
        }
        let content_width = max_width.saturating_sub(ellipsis_width);
        Self::ellipsize_tail(trimmed, content_width)
    }

    pub(crate) fn hard_wrap(value: &str, max_width: usize) -> Vec<String> {
        Self::hard_wrap_with_tab(value, max_width, TAB_DISPLAY_WIDTH)
    }

    pub(crate) fn hard_wrap_with_tab(
        value: &str,
        max_width: usize,
        tab_width: usize,
    ) -> Vec<String> {
        let width = max_width.max(1);
        let mut chunks = Vec::new();
        let mut current = String::new();
        let mut current_width = 0usize;

        for grapheme in value.graphemes(true) {
            let grapheme_width = Self::grapheme_width(grapheme, tab_width);
            if current_width > 0 && current_width.saturating_add(grapheme_width) > width {
                chunks.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push_str(grapheme);
            current_width = current_width.saturating_add(grapheme_width);
            if current_width >= width {
                chunks.push(std::mem::take(&mut current));
                current_width = 0;
            }
        }

        if !current.is_empty() || chunks.is_empty() {
            chunks.push(current);
        }
        chunks
    }

    fn ellipsize_tail(value: &str, content_width: usize) -> String {
        let mut output = String::new();
        let mut used = 0usize;
        for grapheme in value.graphemes(true) {
            let width = Self::grapheme_width(grapheme, TAB_DISPLAY_WIDTH);
            if used.saturating_add(width) > content_width {
                break;
            }
            output.push_str(grapheme);
            used = used.saturating_add(width);
        }
        output.push_str(ELLIPSIS);
        output
    }

    fn grapheme_width(grapheme: &str, tab_width: usize) -> usize {
        if grapheme == "\t" {
            tab_width
        } else {
            UnicodeWidthStr::width(grapheme)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapPolicy {
    pub(crate) trim: bool,
    pub(crate) break_words: bool,
    pub(crate) preserve_preformatted: bool,
    pub(crate) preserve_url: bool,
}

impl WrapPolicy {
    pub(crate) const TRANSCRIPT: Self = Self {
        trim: false,
        break_words: false,
        preserve_preformatted: true,
        preserve_url: true,
    };
}
