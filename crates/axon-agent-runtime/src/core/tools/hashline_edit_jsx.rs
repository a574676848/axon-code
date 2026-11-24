//! Hashline replacement 边界修复使用的轻量 JSX 标签配对。

pub(super) fn payload_has_opener_for_echo(payload: &[String], echo: &[String]) -> bool {
    let mut open_tags = Vec::<String>::new();
    for tag in read_tags(&payload.join("\n")) {
        if tag.closing {
            if open_tags.last() == Some(&tag.name) {
                open_tags.pop();
            }
        } else if !tag.self_closing {
            open_tags.push(tag.name);
        }
    }
    echo.iter().any(|line| {
        closer_name(line).is_some_and(|name| open_tags.iter().any(|open| open == &name))
    })
}

pub(super) fn is_jsx_closer_line(line: &str) -> bool {
    let trimmed = line.trim().trim_end_matches([';', ',']).trim();
    trimmed == "/>" || closer_name(trimmed).is_some()
}

#[derive(Debug)]
struct JsxTag {
    name: String,
    closing: bool,
    self_closing: bool,
}

fn read_tags(text: &str) -> Vec<JsxTag> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'<' || !is_tag_start(bytes.get(index + 1).copied()) {
            index += 1;
            continue;
        }
        let Some(end) = find_tag_end(bytes, index) else {
            break;
        };
        if let Some(tag) = parse_tag(&text[index..=end]) {
            tags.push(tag);
        }
        index = end + 1;
    }
    tags
}

fn is_tag_start(next: Option<u8>) -> bool {
    matches!(next, Some(b'>' | b'/')) || next.is_some_and(|byte| byte.is_ascii_alphabetic())
}

fn find_tag_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut quote = None;
    let mut braces = 0_usize;
    let mut index = start + 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if byte == b'\\' {
                index = index.saturating_add(2);
                continue;
            }
            if byte == active {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"' | b'`') {
            quote = Some(byte);
        } else if byte == b'{' {
            braces = braces.saturating_add(1);
        } else if byte == b'}' && braces > 0 {
            braces -= 1;
        } else if byte == b'>' && braces == 0 {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn parse_tag(raw: &str) -> Option<JsxTag> {
    if matches!(raw, "<>" | "</>") {
        return Some(JsxTag {
            name: String::new(),
            closing: raw == "</>",
            self_closing: false,
        });
    }
    let closing = raw.starts_with("</");
    let start = if closing { 2 } else { 1 };
    let end = raw[start..]
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-')))
        .map(|offset| start + offset)
        .unwrap_or(raw.len());
    if end == start {
        return None;
    }
    Some(JsxTag {
        name: raw[start..end].to_owned(),
        closing,
        self_closing: !closing && raw.trim_end().ends_with("/>"),
    })
}

fn closer_name(line: &str) -> Option<String> {
    let trimmed = line.trim().trim_end_matches([';', ',']).trim();
    if trimmed == "</>" {
        return Some(String::new());
    }
    trimmed
        .strip_prefix("</")
        .and_then(|value| value.strip_suffix('>'))
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-'))
        })
        .map(str::to_owned)
}
