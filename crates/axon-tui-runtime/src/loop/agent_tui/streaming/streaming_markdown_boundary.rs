//! Markdown 流式边界判定。
//!
//! pulldown-cmark 会在 EOF 隐式闭合最后一个块。流式渲染不能直接消费这段
//! 不稳定尾块，否则代码块、表格、列表等结构还在生成时会反复重排。

use std::ops::Range;

use axon_display::{
    is_table_delimiter_line, is_table_header_line, parse_fence_marker, strip_blockquote_prefix,
    table_candidate_text,
};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use super::streaming_markdown_inline_boundary::{
    ambiguous_inline_start, incomplete_atx_heading_start,
};
use crate::widgets::markdown_parser_options;

#[derive(Debug, Default, Clone)]
pub(crate) struct StreamingMarkdownBoundary;

/// 完整 source 的增量稳定边界。
///
/// 已确认的前缀不会再改变 Markdown 语义，因此后续只解析尚未确认的尾段。
/// 普通文本可直接追加；Markdown 语法、列表、表格与内联标记在尾段内
/// 保持无状态精确解析。
#[derive(Debug, Default, Clone)]
pub(crate) struct IncrementalStreamingMarkdownBoundary {
    stable_source_len: usize,
    #[cfg(test)]
    parsed_source_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StableSourceUpdate {
    pub(crate) stable_source_len: usize,
    pub(crate) can_append_stable_layout: bool,
}

impl IncrementalStreamingMarkdownBoundary {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn append_source(
        &mut self,
        source: &str,
        appended_source: &str,
    ) -> StableSourceUpdate {
        let source_start = source.len().saturating_sub(appended_source.len());
        debug_assert!(source.ends_with(appended_source));
        let previous_stable_source_len = self.stable_source_len;
        let plain_text_increment = previous_stable_source_len == source_start
            && is_plain_text_source_chunk(appended_source);
        self.stable_source_len = if plain_text_increment {
            source.len()
        } else {
            let unresolved_source = &source[previous_stable_source_len..];
            #[cfg(test)]
            self.record_parse(unresolved_source);
            #[cfg(not(test))]
            Self::record_parse(unresolved_source);
            previous_stable_source_len.saturating_add(StreamingMarkdownBoundary::stable_source_len(
                unresolved_source,
            ))
        };
        StableSourceUpdate {
            stable_source_len: self.stable_source_len,
            can_append_stable_layout: plain_text_increment,
        }
    }

    /// 用当前尚未完成的尾行替换边界状态。
    ///
    /// 这段 source 仍可能被后续 delta 改写 Markdown 语义，必须保留完整上下文。
    pub(crate) fn replace_uncommitted_source(&mut self, source: &str) -> StableSourceUpdate {
        #[cfg(test)]
        self.record_parse(source);
        #[cfg(not(test))]
        Self::record_parse(source);
        self.stable_source_len = StreamingMarkdownBoundary::stable_source_len(source);
        StableSourceUpdate {
            stable_source_len: self.stable_source_len,
            can_append_stable_layout: false,
        }
    }

    pub(crate) const fn stable_source_len(&self) -> usize {
        self.stable_source_len
    }

    #[cfg(test)]
    pub(crate) const fn parsed_source_bytes(&self) -> usize {
        self.parsed_source_bytes
    }

    #[cfg(test)]
    fn record_parse(&mut self, source: &str) {
        self.parsed_source_bytes = self.parsed_source_bytes.saturating_add(source.len());
    }

    #[cfg(not(test))]
    fn record_parse(_source: &str) {}
}

impl StreamingMarkdownBoundary {
    pub(crate) fn stable_source_len(source: &str) -> usize {
        let Some(block) = LastTopLevelBlock::parse(source) else {
            return source.len();
        };
        if !block.ends_at_source_end(source) {
            return source.len();
        }
        if let Some(unstable_start) = streaming_unstable_start(source, &block) {
            return unstable_start;
        }
        source.len()
    }
}

/// 只接受不会开启或修改 Markdown 结构的文本 chunk。
fn is_plain_text_source_chunk(source: &str) -> bool {
    source.split_inclusive('\n').all(is_plain_text_line)
}

fn is_plain_text_line(source_line: &str) -> bool {
    let line = source_line.trim_end_matches(['\n', '\r']);
    if line.trim().is_empty() {
        return false;
    }
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let first = chars.next();
    if matches!(
        first,
        Some('#' | '>' | '-' | '+' | '*' | '`' | '[' | '<' | '|')
    ) {
        return false;
    }
    if first.is_some_and(|character| character.is_ascii_digit())
        && trimmed
            .find(|character: char| !character.is_ascii_digit())
            .is_some_and(|index| trimmed[index..].starts_with('.'))
    {
        return false;
    }
    !line.contains(['#', '*', '_', '`', '[', ']', '~', '\\', '<', '>', '|', '='])
}

#[derive(Debug, Clone)]
struct LastTopLevelBlock {
    kind: BlockKind,
    range: Range<usize>,
    last_item_range: Option<Range<usize>>,
}

impl LastTopLevelBlock {
    fn parse(source: &str) -> Option<Self> {
        let mut stack = Vec::<BlockKind>::new();
        let mut current_start: Option<usize> = None;
        let mut current_last_item_range: Option<Range<usize>> = None;
        let mut current_top_list_item_depth: Option<usize> = None;
        let mut last: Option<Self> = None;

        for (event, range) in Parser::new_ext(source, markdown_parser_options()).into_offset_iter()
        {
            match event {
                Event::Start(tag) => {
                    let Some(kind) = BlockKind::from_start_tag(&tag) else {
                        continue;
                    };
                    if stack.is_empty() {
                        current_start = Some(range.start);
                        current_last_item_range = None;
                        current_top_list_item_depth = None;
                    }
                    stack.push(kind);
                    if kind == BlockKind::List && stack.len() == 1 {
                        current_top_list_item_depth = Some(2);
                    }
                }
                Event::End(tag_end) => {
                    if !is_block_end(tag_end) {
                        continue;
                    }
                    let Some(kind) = stack.pop() else {
                        continue;
                    };
                    if kind == BlockKind::Item
                        && current_top_list_item_depth.is_some_and(|depth| stack.len() + 1 == depth)
                    {
                        current_last_item_range = Some(range.clone());
                    }
                    if stack.is_empty() {
                        let start = current_start.take().unwrap_or(range.start);
                        last = Some(Self {
                            kind,
                            range: start..range.end,
                            last_item_range: current_last_item_range.take(),
                        });
                    }
                }
                _ => {}
            }
        }

        last
    }

    fn ends_at_source_end(&self, source: &str) -> bool {
        source[self.range.end..].trim().is_empty()
    }

    fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.clone()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading,
    BlockQuote,
    FencedCode,
    IndentedCode,
    Html,
    List,
    Item,
    Table,
    TableHead,
    TableRow,
    TableCell,
    FootnoteDefinition,
    DefinitionList,
    DefinitionListTitle,
    DefinitionListDefinition,
    Metadata,
}

impl BlockKind {
    fn from_start_tag(tag: &Tag<'_>) -> Option<Self> {
        match tag {
            Tag::Paragraph => Some(Self::Paragraph),
            Tag::Heading { .. } => Some(Self::Heading),
            Tag::BlockQuote(_) => Some(Self::BlockQuote),
            Tag::CodeBlock(CodeBlockKind::Fenced(_)) => Some(Self::FencedCode),
            Tag::CodeBlock(CodeBlockKind::Indented) => Some(Self::IndentedCode),
            Tag::HtmlBlock => Some(Self::Html),
            Tag::List(_) => Some(Self::List),
            Tag::Item => Some(Self::Item),
            Tag::FootnoteDefinition(_) => Some(Self::FootnoteDefinition),
            Tag::DefinitionList => Some(Self::DefinitionList),
            Tag::DefinitionListTitle => Some(Self::DefinitionListTitle),
            Tag::DefinitionListDefinition => Some(Self::DefinitionListDefinition),
            Tag::Table(_) => Some(Self::Table),
            Tag::TableHead => Some(Self::TableHead),
            Tag::TableRow => Some(Self::TableRow),
            Tag::TableCell => Some(Self::TableCell),
            Tag::MetadataBlock(_) => Some(Self::Metadata),
            Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. } => None,
        }
    }
}

fn is_block_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::MetadataBlock(_)
    )
}

fn streaming_unstable_start(source: &str, block: &LastTopLevelBlock) -> Option<usize> {
    let text = block.text(source);
    match block.kind {
        BlockKind::FencedCode => (!has_closing_fence(text)).then_some(block.range.start),
        BlockKind::IndentedCode => {
            (!ends_with_blank_line(source)).then_some(line_start_at(source, block.range.start))
        }
        BlockKind::Html => (!html_block_has_streaming_boundary(text)
            && !ends_with_blank_line(source))
        .then_some(block.range.start),
        BlockKind::List => list_streaming_unstable_start(source, block),
        BlockKind::Item
        | BlockKind::BlockQuote
        | BlockKind::FootnoteDefinition
        | BlockKind::DefinitionList
        | BlockKind::DefinitionListTitle
        | BlockKind::DefinitionListDefinition
        | BlockKind::Metadata => (!ends_with_blank_line(source)).then_some(block.range.start),
        BlockKind::Table | BlockKind::TableHead | BlockKind::TableRow | BlockKind::TableCell => {
            (!ends_with_blank_line(source)).then_some(block.range.start)
        }
        BlockKind::Paragraph => paragraph_streaming_unstable_start(source, text)
            .map(|offset| block.range.start + offset),
        BlockKind::Heading => None,
    }
}

fn list_streaming_unstable_start(source: &str, block: &LastTopLevelBlock) -> Option<usize> {
    if ends_with_blank_line(source) {
        return None;
    }
    let item = block.last_item_range.as_ref()?;
    Some(line_start_at(source, item.start))
}

fn paragraph_streaming_unstable_start(source: &str, text: &str) -> Option<usize> {
    if ends_with_blank_line(source) {
        return None;
    }
    let mut last_non_blank: Option<(usize, &str)> = None;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        if !line.trim().is_empty() {
            last_non_blank = Some((offset, line));
        }
        offset = offset.saturating_add(line.len());
    }
    let (line_start, line) = last_non_blank?;
    let table_start = table_candidate_text(line).and_then(|candidate| {
        (is_table_header_line(candidate) && !is_table_delimiter_line(candidate))
            .then_some(line_start)
    });
    table_start
        .or_else(|| incomplete_atx_heading_start(text))
        .or_else(|| ambiguous_inline_start(text))
}

fn ends_with_blank_line(source: &str) -> bool {
    source.ends_with('\n')
        && source
            .lines()
            .last()
            .is_some_and(|line| line.trim().is_empty())
}

fn line_start_at(source: &str, offset: usize) -> usize {
    source[..offset]
        .rfind('\n')
        .map_or(0, |line_end| line_end + 1)
}

fn html_block_has_streaming_boundary(block: &str) -> bool {
    let trimmed = block.trim();
    if trimmed.starts_with("<!--") {
        return trimmed.ends_with("-->");
    }
    if trimmed.starts_with("<?") {
        return trimmed.ends_with("?>");
    }
    if trimmed.starts_with("<![CDATA[") {
        return trimmed.ends_with("]]>");
    }
    raw_html_block_tag_name(trimmed).is_some_and(|tag| has_html_closing_tag(trimmed, tag))
}

fn raw_html_block_tag_name(block: &str) -> Option<&str> {
    let tag = html_block_tag_name(block)?;
    is_raw_html_block_tag(tag).then_some(tag)
}

fn is_raw_html_block_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "script" | "pre" | "style" | "textarea"
    )
}

fn html_block_tag_name(block: &str) -> Option<&str> {
    let rest = block.strip_prefix('<')?;
    if rest.starts_with('/') || rest.starts_with('!') || rest.starts_with('?') {
        return None;
    }
    let end = rest
        .find(|ch: char| ch == '>' || ch == '/' || ch.is_ascii_whitespace())
        .unwrap_or(rest.len());
    (end > 0).then_some(&rest[..end])
}

fn has_html_closing_tag(block: &str, tag: &str) -> bool {
    let needle = format!("</{}>", tag.to_ascii_lowercase());
    block.to_ascii_lowercase().contains(&needle)
}

fn has_closing_fence(block: &str) -> bool {
    let mut lines = block.lines();
    let Some(open_line) = lines.next() else {
        return false;
    };
    let Some(open) = fence_open(open_line) else {
        return false;
    };
    lines.any(|line| is_fence_close(line, open))
}

fn fence_open(line: &str) -> Option<FenceBlock> {
    let scan = fence_scan_text(line)?;
    let (marker, len) = parse_fence_marker(scan)?;
    Some(FenceBlock { marker, len })
}

fn is_fence_close(line: &str, open: FenceBlock) -> bool {
    let Some(scan) = fence_scan_text(line) else {
        return false;
    };
    let Some((marker, len)) = parse_fence_marker(scan) else {
        return false;
    };
    marker == open.marker && len >= open.len && fence_tail(scan, len).is_empty()
}

fn fence_scan_text(line: &str) -> Option<&str> {
    let stripped = strip_optional_fence_indent(line)?;
    let stripped = strip_blockquote_prefix(stripped);
    strip_optional_fence_indent(stripped)
}

fn strip_optional_fence_indent(line: &str) -> Option<&str> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let mut byte_index = 0usize;
    let mut columns = 0usize;
    for byte in trimmed.as_bytes() {
        match byte {
            b' ' => {
                byte_index += 1;
                columns += 1;
            }
            b'\t' => {
                byte_index += 1;
                columns += 4;
            }
            _ => break,
        }
        if columns >= 4 {
            return None;
        }
    }
    Some(&trimmed[byte_index..])
}

fn fence_tail(line: &str, marker_len: usize) -> &str {
    let marker_end = line
        .char_indices()
        .nth(marker_len)
        .map_or(line.len(), |(index, _)| index);
    line[marker_end..].trim()
}

#[derive(Debug, Clone, Copy)]
struct FenceBlock {
    marker: char,
    len: usize,
}

#[cfg(test)]
#[path = "streaming_markdown_boundary_tests.rs"]
mod tests;
