//! Markdown 行内结构的流式歧义判定。

const MAX_ATX_HEADING_MARKERS: usize = 6;
const MAX_COMMONMARK_INDENT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Delimiter {
    marker: char,
    len: usize,
    start: usize,
}

#[derive(Debug, Clone, Copy)]
struct LinkDestination {
    start: usize,
    depth: usize,
}

pub(super) fn incomplete_atx_heading_start(text: &str) -> Option<usize> {
    if text.ends_with('\n') {
        return None;
    }
    let line_start = text.rfind('\n').map_or(0, |index| index + 1);
    let line = &text[line_start..];
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > MAX_COMMONMARK_INDENT {
        return None;
    }
    let marker_len = line[indent..]
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();
    if marker_len == 0 || marker_len > MAX_ATX_HEADING_MARKERS {
        return None;
    }
    let after_marker = &line[indent + marker_len..];
    let ambiguous = after_marker.is_empty()
        || after_marker
            .chars()
            .next()
            .is_some_and(|ch| is_heading_separator(ch) || is_cjk_heading_start(ch));
    ambiguous.then_some(line_start + indent)
}

pub(super) fn ambiguous_inline_start(text: &str) -> Option<usize> {
    InlineAmbiguityScanner::new(text).scan()
}

struct InlineAmbiguityScanner<'a> {
    source: &'a str,
    cursor: usize,
    code: Option<Delimiter>,
    styles: Vec<Delimiter>,
    brackets: Vec<usize>,
    link_destination: Option<LinkDestination>,
}

impl<'a> InlineAmbiguityScanner<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            cursor: 0,
            code: None,
            styles: Vec::new(),
            brackets: Vec::new(),
            link_destination: None,
        }
    }

    fn scan(mut self) -> Option<usize> {
        while self.cursor < self.source.len() {
            let ch = self.source[self.cursor..].chars().next()?;
            if self.scan_code(ch) || self.scan_link_destination(ch) {
                continue;
            }
            if ch == '\\' {
                self.skip_escaped_char();
                continue;
            }
            match ch {
                '`' => self.open_code_span(),
                '[' => self.open_bracket(),
                ']' => self.close_bracket(),
                '*' | '_' | '~' => self.scan_style_delimiter(ch),
                _ => self.cursor += ch.len_utf8(),
            }
        }
        self.first_unclosed_start()
    }

    fn scan_code(&mut self, ch: char) -> bool {
        let Some(open) = self.code else {
            return false;
        };
        if ch == '`' {
            let run_end = delimiter_run_end(self.source, self.cursor, ch);
            if run_end - self.cursor == open.len {
                self.code = None;
            }
            self.cursor = run_end;
        } else {
            self.cursor += ch.len_utf8();
        }
        true
    }

    fn scan_link_destination(&mut self, ch: char) -> bool {
        let Some(mut destination) = self.link_destination.take() else {
            return false;
        };
        if ch == '\\' {
            self.skip_escaped_char();
        } else {
            self.cursor += ch.len_utf8();
            match ch {
                '(' => destination.depth = destination.depth.saturating_add(1),
                ')' => destination.depth = destination.depth.saturating_sub(1),
                _ => {}
            }
        }
        if destination.depth > 0 {
            self.link_destination = Some(destination);
        }
        true
    }

    fn skip_escaped_char(&mut self) {
        self.cursor += '\\'.len_utf8();
        if let Some(ch) = self.source[self.cursor..].chars().next() {
            self.cursor += ch.len_utf8();
        }
    }

    fn open_code_span(&mut self) {
        let start = self.cursor;
        let run_end = delimiter_run_end(self.source, start, '`');
        self.code = Some(Delimiter {
            marker: '`',
            len: run_end - start,
            start,
        });
        self.cursor = run_end;
    }

    fn open_bracket(&mut self) {
        let image_start = self.source[..self.cursor]
            .char_indices()
            .next_back()
            .filter(|(_, ch)| *ch == '!')
            .map_or(self.cursor, |(index, _)| index);
        self.brackets.push(image_start);
        self.cursor += '['.len_utf8();
    }

    fn close_bracket(&mut self) {
        let close_end = self.cursor + ']'.len_utf8();
        self.cursor = close_end;
        let Some(start) = self.brackets.pop() else {
            return;
        };
        if self.source[close_end..].starts_with('(') {
            self.cursor += '('.len_utf8();
            self.link_destination = Some(LinkDestination { start, depth: 1 });
        }
    }

    fn scan_style_delimiter(&mut self, marker: char) {
        let start = self.cursor;
        let run_end = delimiter_run_end(self.source, start, marker);
        self.cursor = run_end;
        let len = run_end - start;
        if marker == '~' && len < 2 {
            return;
        }
        let previous = self.source[..start].chars().next_back();
        let next = self.source[run_end..].chars().next();
        if marker == '_'
            && previous.is_some_and(char::is_alphanumeric)
            && next.is_some_and(char::is_alphanumeric)
        {
            return;
        }
        let can_close = previous.is_some_and(|ch| !ch.is_whitespace());
        if can_close
            && let Some(index) = self
                .styles
                .iter()
                .rposition(|open| open.marker == marker && open.len == len)
        {
            self.styles.remove(index);
            return;
        }
        if next.is_some_and(|ch| !ch.is_whitespace()) {
            self.styles.push(Delimiter { marker, len, start });
        }
    }

    fn first_unclosed_start(&self) -> Option<usize> {
        self.code
            .map(|delimiter| delimiter.start)
            .into_iter()
            .chain(self.styles.iter().map(|delimiter| delimiter.start))
            .chain(self.brackets.iter().copied())
            .chain(self.link_destination.map(|destination| destination.start))
            .min()
    }
}

fn delimiter_run_end(source: &str, start: usize, marker: char) -> usize {
    source[start..]
        .char_indices()
        .take_while(|(_, ch)| *ch == marker)
        .last()
        .map_or(start, |(index, ch)| start + index + ch.len_utf8())
}

fn is_heading_separator(ch: char) -> bool {
    (ch.is_whitespace() && !matches!(ch, '\r' | '\n')) || ch == '\u{200b}'
}

fn is_cjk_heading_start(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{3040}'..='\u{30ff}'
            | '\u{ac00}'..='\u{d7af}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_incomplete_cjk_atx_heading_without_matching_code_literals() {
        assert_eq!(incomplete_atx_heading_start("before\n##标题"), Some(7));
        assert_eq!(incomplete_atx_heading_start("##\u{3000}标题"), Some(0));
        assert_eq!(incomplete_atx_heading_start("##Title"), None);
        assert_eq!(incomplete_atx_heading_start("    ##标题"), None);
    }

    #[test]
    fn detects_unclosed_code_styles_and_links() {
        assert_eq!(ambiguous_inline_start("说明 `code"), Some("说明 ".len()));
        assert_eq!(ambiguous_inline_start("说明 *重点"), Some("说明 ".len()));
        assert_eq!(ambiguous_inline_start("说明 _重点"), Some("说明 ".len()));
        assert_eq!(ambiguous_inline_start("说明 **重点"), Some("说明 ".len()));
        assert_eq!(ambiguous_inline_start("说明 [链接"), Some("说明 ".len()));
        assert_eq!(
            ambiguous_inline_start("说明 [链接](https://a"),
            Some("说明 ".len())
        );
    }

    #[test]
    fn releases_closed_or_escaped_inline_markers() {
        assert_eq!(
            ambiguous_inline_start("`code` *强调* _强调_ **重点** [链接](https://a)"),
            None
        );
        assert_eq!(ambiguous_inline_start("\\`code \\*\\*重点 \\[链接"), None);
        assert_eq!(ambiguous_inline_start("snake_case 和 2 * 3"), None);
    }
}
