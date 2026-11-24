//! Transcript 消息 Markdown 渲染器。

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::ops::Range;

use super::{
    transcript_markdown_blocks::{BlockPrefixStack, ListContext},
    transcript_markdown_code::CodeBlockState,
    transcript_markdown_layout::{MarkdownLineKind, MarkdownLineLayout, RenderedMarkdownLine},
    transcript_markdown_table::TableState,
    transcript_markdown_wrap::{wrap_preformatted_line, wrap_reflow_line},
};
use crate::app_shell::syntax_theme::SyntaxTheme;
use crate::widgets::{
    terminal_hyperlinks::{HyperlinkLine, annotate_web_urls_in_line},
    transcript_markdown_helpers::HyperlinkLineExt,
    transcript_markdown_links::{LinkContext, LinkFinish},
    transcript_markdown_styles::{MarkdownStyles, heading_prefix},
    transcript_markdown_text_merge::DecodedTextMerge,
};
use axon_display::{normalize_markdown_source, table_candidate_text};

const HORIZONTAL_RULE_TEXT: &str = "———";

struct MarkdownWriter<'a> {
    lines: Vec<RenderedMarkdownLine>,
    current: HyperlinkLine,
    input: &'a str,
    styles: MarkdownStyles,
    inline_styles: Vec<Style>,
    links: Vec<LinkContext>,
    lists: Vec<ListContext>,
    block_prefixes: BlockPrefixStack,
    code_block: Option<CodeBlockState>,
    paragraph_active: bool,
    heading_level: Option<HeadingLevel>,
    table: Option<TableState>,
    output_body_width: usize,
    syntax_theme: Option<&'a SyntaxTheme>,
    line_ends_with_local_link_target: bool,
    pending_local_link_soft_break: bool,
    current_item_paragraphs: usize,
    list_item_paragraph_stack: Vec<usize>,
    current_item_has_wrapped_line: bool,
    list_item_wrapped_line_stack: Vec<bool>,
    current_item_needs_following_separator: bool,
    list_item_separator_stack: Vec<bool>,
}

impl<'a> MarkdownWriter<'a> {
    fn new(body_style: Style, syntax_theme: Option<&'a SyntaxTheme>) -> Self {
        Self {
            lines: Vec::new(),
            current: HyperlinkLine::default(),
            input: "",
            styles: MarkdownStyles::new(body_style),
            inline_styles: Vec::new(),
            links: Vec::new(),
            lists: Vec::new(),
            block_prefixes: BlockPrefixStack::default(),
            code_block: None,
            paragraph_active: false,
            heading_level: None,
            table: None,
            output_body_width: 1,
            syntax_theme,
            line_ends_with_local_link_target: false,
            pending_local_link_soft_break: false,
            current_item_paragraphs: 0,
            list_item_paragraph_stack: Vec::new(),
            current_item_has_wrapped_line: false,
            list_item_wrapped_line_stack: Vec::new(),
            current_item_needs_following_separator: false,
            list_item_separator_stack: Vec::new(),
        }
    }

    fn run(mut self, input: &'a str, width: u16) -> Vec<HyperlinkLine> {
        self.output_body_width = usize::from(width.max(1));
        self.parse(input);
        self.into_hyperlink_lines()
    }

    fn parse(&mut self, input: &'a str) {
        self.input = input;
        let parser = DecodedTextMerge::new(
            Parser::new_ext(input, markdown_parser_options()).into_offset_iter(),
        );
        for (event, range) in parser {
            self.handle_event(event, range);
        }
        self.flush_current_line();
        self.trim_blank_edges();
    }

    fn handle_event(&mut self, event: Event<'_>, range: Range<usize>) {
        self.prepare_for_event(&event);
        match event {
            Event::Start(tag) => self.start_tag(tag, range),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_span(code.into_string(), self.styles.code),
            Event::SoftBreak | Event::HardBreak => self.push_line_break(),
            Event::Html(html) => self.push_html(&html, false),
            Event::InlineHtml(html) => self.push_html(&html, true),
            Event::InlineMath(math) | Event::DisplayMath(math) => self.push_text(&math),
            Event::Rule => self.push_rule(),
            Event::TaskListMarker(checked) => self.push_task_marker(checked),
            Event::FootnoteReference(_) => {}
        }
    }

    fn prepare_for_event(&mut self, event: &Event<'_>) {
        if !self.pending_local_link_soft_break {
            return;
        }
        if matches!(event, Event::Text(text) if text.trim_start().starts_with(':')) {
            self.pending_local_link_soft_break = false;
            return;
        }
        self.pending_local_link_soft_break = false;
        self.flush_current_line();
    }

    fn start_tag(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Paragraph => self.start_paragraph(),
            Tag::Heading { level, .. } => self.start_heading(level),
            Tag::List(start) => self.lists.push(ListContext::new(start)),
            Tag::Item => self.start_item(),
            Tag::Emphasis => self.inline_styles.push(self.styles.emphasis),
            Tag::Strong => self.inline_styles.push(self.styles.strong),
            Tag::Strikethrough => self
                .inline_styles
                .push(self.inline_style().add_modifier(Modifier::CROSSED_OUT)),
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::BlockQuote(_) => self.start_blockquote(),
            Tag::Table(alignments) => self.start_table(alignments),
            Tag::TableHead | Tag::TableRow => self.start_table_row(range),
            Tag::TableCell => self.start_table_cell(),
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.links
                    .push(LinkContext::from_destination(dest_url.as_ref()));
            }
            Tag::Superscript | Tag::Subscript => self.inline_styles.push(self.inline_style()),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_current_line();
                self.paragraph_active = false;
            }
            TagEnd::Heading(_) => {
                self.flush_current_line();
                self.paragraph_active = false;
                self.heading_level = None;
                self.inline_styles.pop();
            }
            TagEnd::Item => {
                self.flush_current_line();
                if self.current_item_paragraphs > 1
                    || self.current_item_has_wrapped_line
                    || self.current_item_needs_following_separator
                {
                    self.push_blank_line();
                }
                self.block_prefixes.end_block();
                self.current_item_paragraphs = self.list_item_paragraph_stack.pop().unwrap_or(0);
                self.current_item_has_wrapped_line =
                    self.list_item_wrapped_line_stack.pop().unwrap_or(false);
                self.current_item_needs_following_separator =
                    self.list_item_separator_stack.pop().unwrap_or(false);
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.push_blank_line();
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Superscript | TagEnd::Subscript => {
                self.inline_styles.pop();
            }
            TagEnd::CodeBlock => {
                self.flush_current_line();
                self.code_block = None;
                self.current_item_needs_following_separator = true;
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::TableHead => self.finish_table_head(),
            TagEnd::TableRow => self.finish_table_row(),
            TagEnd::TableCell => self.finish_table_cell(),
            TagEnd::BlockQuote(_) => {
                self.flush_current_line();
                self.block_prefixes.end_block();
                self.inline_styles.pop();
            }
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => {}
            TagEnd::Link | TagEnd::Image => {
                self.finish_link();
            }
        }
    }

    fn start_paragraph(&mut self) {
        if self.paragraph_active {
            return;
        }
        if self.current_item_paragraphs == 0
            && !self.block_prefixes.has_active_blockquote()
            && self.lists.is_empty()
            && !self.lines.is_empty()
            && !self
                .lines
                .last()
                .is_some_and(RenderedMarkdownLine::is_blank)
        {
            self.push_blank_line();
        }
        if self.current_item_paragraphs > 0 && !self.block_prefixes.has_active_blockquote() {
            self.push_blank_line();
        }
        self.current_item_paragraphs = self.current_item_paragraphs.saturating_add(1);
        self.paragraph_active = true;
    }

    fn start_heading(&mut self, level: HeadingLevel) {
        self.flush_current_line();
        if self.lists.is_empty()
            && !self.lines.is_empty()
            && !self
                .lines
                .last()
                .is_some_and(RenderedMarkdownLine::is_blank)
        {
            self.push_blank_line();
        }
        self.inline_styles.push(self.styles.heading(level));
        self.heading_level = Some(level);
    }

    fn start_item(&mut self) {
        self.flush_current_line();
        self.block_prefixes.start_list_item(&mut self.lists);
        self.list_item_paragraph_stack
            .push(std::mem::take(&mut self.current_item_paragraphs));
        self.list_item_wrapped_line_stack
            .push(std::mem::take(&mut self.current_item_has_wrapped_line));
        self.list_item_separator_stack.push(std::mem::take(
            &mut self.current_item_needs_following_separator,
        ));
    }

    fn start_blockquote(&mut self) {
        self.flush_current_line();
        if self.block_prefixes.has_active_blockquote() {
            self.push_blank_line();
        }
        self.block_prefixes.start_blockquote();
        self.inline_styles.push(self.styles.blockquote);
    }

    fn start_code_block(&mut self, kind: pulldown_cmark::CodeBlockKind<'_>) {
        self.flush_current_line();
        if !self.lines.is_empty()
            && !self
                .lines
                .last()
                .is_some_and(RenderedMarkdownLine::is_blank)
        {
            self.push_blank_line();
        }
        self.code_block = Some(CodeBlockState::from_kind(kind));
    }

    fn push_text(&mut self, text: &str) {
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        if self.code_block.is_some() {
            for (index, line) in text.lines().enumerate() {
                let line_started = self
                    .code_block
                    .as_ref()
                    .is_some_and(CodeBlockState::line_started);
                if index > 0 || line_started {
                    self.flush_current_line();
                }
                self.push_code_line(line);
                if let Some(code_block) = self.code_block.as_mut() {
                    code_block.mark_line_started();
                }
            }
            return;
        }

        for (index, line) in text.lines().enumerate() {
            if index > 0 {
                self.push_line_break();
            }
            if !line.is_empty() {
                let text = self.display_text_for_active_link(line);
                self.push_span(text, self.inline_text_style());
            }
        }
    }

    fn push_html(&mut self, html: &str, inline: bool) {
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        if self.code_block.is_some() {
            self.push_text(html);
            return;
        }
        for (index, line) in html.lines().enumerate() {
            if index > 0 {
                self.push_line_break();
            }
            if !line.is_empty() {
                self.push_span(line.to_owned(), self.inline_text_style());
            }
        }
        if !inline {
            self.push_line_break();
        }
    }

    fn push_span(&mut self, text: String, style: Style) {
        if self.suppressing_local_link_label() {
            return;
        }
        self.line_ends_with_local_link_target = false;
        let destination = self.active_link_destination();
        self.push_span_with_destination(text, style, destination);
    }

    fn push_span_with_destination(
        &mut self,
        text: String,
        style: Style,
        destination: Option<String>,
    ) {
        if text.is_empty() {
            return;
        }
        if let Some(table) = self.table.as_mut()
            && table.in_cell()
        {
            table.push_span(Span::styled(text, style), destination);
            return;
        }
        self.current
            .push_span(Span::styled(text, style), destination);
    }

    fn push_line_break(&mut self) {
        if let Some(table) = self.table.as_mut()
            && table.in_cell()
        {
            table.push_line_break();
            return;
        }
        if self.suppressing_local_link_label() {
            return;
        }
        if self.line_ends_with_local_link_target {
            self.pending_local_link_soft_break = true;
            self.line_ends_with_local_link_target = false;
            return;
        }
        self.line_ends_with_local_link_target = false;
        self.flush_current_line();
    }

    fn push_code_line(&mut self, line: &str) {
        let Some(code_block) = self.code_block.as_ref() else {
            return;
        };
        let code_line = format!("{}{}", code_block.line_prefix(), line);
        if code_line.trim().is_empty() {
            self.flush_current_line();
            self.lines.push(RenderedMarkdownLine::blank());
            return;
        }
        let highlighted = self
            .syntax_theme
            .zip(code_block.language())
            .and_then(|(theme, language)| {
                theme
                    .syntax_for_language(language)
                    .map(|syntax| theme.highlight_line(syntax, &code_line))
            })
            .filter(|spans| !spans.is_empty());
        if let Some(spans) = highlighted {
            for span in spans {
                self.current.push_span(span, None);
            }
            return;
        }
        self.push_span(code_line, self.styles.code);
    }

    fn push_task_marker(&mut self, checked: bool) {
        let marker = if checked { "☑ " } else { "☐ " };
        self.push_span(marker.to_owned(), self.styles.unordered_list_marker);
    }

    fn push_rule(&mut self) {
        self.flush_current_line();
        if !self.lines.is_empty() {
            self.push_blank_line();
        }
        self.push_span(HORIZONTAL_RULE_TEXT.to_owned(), self.styles.marker);
        self.flush_current_line();
        self.push_blank_line();
    }

    fn start_table(&mut self, alignments: Vec<Alignment>) {
        self.flush_current_line();
        self.table = Some(TableState::new(alignments));
    }

    fn start_table_row(&mut self, source_range: Range<usize>) {
        let has_table_syntax = self
            .input
            .get(source_range)
            .and_then(table_candidate_text)
            .is_some();
        if let Some(table) = self.table.as_mut() {
            table.start_row(has_table_syntax);
        }
    }

    fn start_table_cell(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.start_cell();
        }
    }

    fn finish_table_cell(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.end_cell();
        }
    }

    fn finish_table_head(&mut self) {
        self.finish_table_row();
    }

    fn finish_table_row(&mut self) {
        if let Some(table) = self.table.as_mut() {
            table.end_row();
        }
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        for line in table.render_lines(self.styles.marker, self.output_body_width) {
            self.push_prefixed_line(line, MarkdownLineKind::Fixed);
        }
    }

    fn inline_style(&self) -> Style {
        self.inline_styles
            .last()
            .copied()
            .unwrap_or(self.styles.body)
    }

    fn inline_text_style(&self) -> Style {
        if self.active_link_destination().is_some() {
            self.inline_style().patch(self.styles.link)
        } else {
            self.inline_style()
        }
    }

    fn flush_current_line(&mut self) {
        if self.current.line.spans.is_empty() {
            return;
        }
        let current = std::mem::take(&mut self.current);
        let kind = if self.code_block.is_some() {
            MarkdownLineKind::Preformatted
        } else {
            MarkdownLineKind::Reflow
        };
        self.push_prefixed_line(current, kind);
        self.line_ends_with_local_link_target = false;
    }

    fn push_prefixed_line(&mut self, mut content: HyperlinkLine, kind: MarkdownLineKind) {
        let mut spans = self.current_block_prefix_spans();
        if let Some(prefix) = self.heading_level.and_then(heading_prefix) {
            spans.push(Span::styled(
                prefix.to_owned(),
                self.styles
                    .heading(self.heading_level.unwrap_or(HeadingLevel::H3)),
            ));
        }
        let mut line = HyperlinkLine::new(Line::from(spans));
        let prefix_width = line.width();
        line.line.spans.append(&mut content.line.spans);
        if !self.list_item_separator_stack.is_empty() {
            self.current_item_has_wrapped_line |= line.width() > self.output_body_width.max(1);
        }
        line.hyperlinks = content
            .hyperlinks
            .drain(..)
            .map(|mut hyperlink| {
                hyperlink.columns =
                    hyperlink.columns.start + prefix_width..hyperlink.columns.end + prefix_width;
                hyperlink
            })
            .collect();
        self.lines
            .push(RenderedMarkdownLine::new(line, kind, prefix_width));
    }

    fn push_blank_line(&mut self) {
        if self
            .lines
            .last()
            .is_some_and(RenderedMarkdownLine::is_blank)
        {
            return;
        }
        if self.block_prefixes.has_active_blockquote() {
            self.push_prefixed_line(HyperlinkLine::default(), MarkdownLineKind::Reflow);
            return;
        }
        self.lines.push(RenderedMarkdownLine::blank());
    }

    fn trim_blank_edges(&mut self) {
        while self
            .lines
            .first()
            .is_some_and(RenderedMarkdownLine::is_blank)
        {
            self.lines.remove(0);
        }
        while self
            .lines
            .last()
            .is_some_and(RenderedMarkdownLine::is_blank)
        {
            self.lines.pop();
        }
    }

    fn into_hyperlink_lines(self) -> Vec<HyperlinkLine> {
        let body_width = self.output_body_width;
        self.lines
            .into_iter()
            .flat_map(|rendered| {
                let line = annotate_web_urls_in_line(rendered.content.line)
                    .merge_hyperlinks(rendered.content.hyperlinks);
                match rendered.layout {
                    MarkdownLineLayout::Fixed => vec![line],
                    MarkdownLineLayout::Preformatted {
                        continuation_indent,
                    } => wrap_preformatted_line(line, body_width, continuation_indent),
                    MarkdownLineLayout::Reflow {
                        continuation_indent,
                    } => wrap_reflow_line(line, body_width, continuation_indent),
                }
            })
            .collect()
    }

    fn active_link_destination(&self) -> Option<String> {
        self.links.last().map(LinkContext::destination)
    }

    fn display_text_for_active_link(&self, text: &str) -> String {
        self.links
            .last()
            .and_then(|link| link.display_text_for_label(text))
            .unwrap_or_else(|| text.to_owned())
    }

    fn finish_link(&mut self) {
        let Some(link) = self.links.pop() else {
            return;
        };
        match link.finish() {
            LinkFinish::LocalTarget {
                display,
                destination,
            } => {
                self.push_span_with_destination(display, self.styles.code, Some(destination));
                self.line_ends_with_local_link_target = true;
            }
            LinkFinish::ShowDestination { destination } => {
                self.push_span_with_destination(" (".to_owned(), self.inline_style(), None);
                self.push_span_with_destination(
                    destination.clone(),
                    self.styles.link,
                    Some(destination),
                );
                self.push_span(")".to_owned(), self.inline_style());
            }
        }
    }

    fn suppressing_local_link_label(&self) -> bool {
        self.links.last().is_some_and(LinkContext::suppress_label)
    }

    fn current_block_prefix_spans(&mut self) -> Vec<Span<'static>> {
        self.block_prefixes.current_spans(
            self.styles.ordered_list_marker,
            self.styles.unordered_list_marker,
            self.styles.blockquote,
        )
    }
}

pub(crate) fn markdown_parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

pub(super) fn render_markdown_hyperlink_lines(
    source: &str,
    body_style: Style,
    width: u16,
    syntax_theme: Option<&SyntaxTheme>,
) -> Vec<HyperlinkLine> {
    if source.trim().is_empty() {
        return Vec::new();
    }
    let normalized = normalize_markdown_source(source);
    MarkdownWriter::new(body_style, syntax_theme).run(&normalized, width)
}
