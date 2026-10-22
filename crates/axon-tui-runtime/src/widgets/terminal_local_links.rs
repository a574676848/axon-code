//! 终端本地文件链接目标解析。
//!
//! 该模块只负责把 Markdown 中的本地路径目标解析成可见路径和 file:// 目标，
//! 不参与 OSC8 注入或可见文本扫描。

use std::path::Path;

use url::Url;

const FILE_SCHEME: &str = "file://";
const CURRENT_DIR_LABEL: &str = ".";
const HASH_LINE_PREFIX: &str = "L";
const LOCATION_SEPARATOR: char = '-';

pub(crate) fn display_path_for_link(path: &str) -> Option<(String, String)> {
    if !is_local_path_like_link(path) {
        return None;
    }
    let target = LocalLinkTarget::parse(path)?;
    let destination = file_destination(Path::new(&target.path))?;
    let display = format!(
        "{}{}",
        relative_display_path(Path::new(&target.path)),
        target.location
    );
    Some((display, format!("{destination}{}", target.location)))
}

fn is_local_path_like_link(path: &str) -> bool {
    path.starts_with(FILE_SCHEME)
        || path.starts_with('/')
        || path.starts_with("~/")
        || path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with("\\\\")
        || matches!(
            path.as_bytes(),
            [drive, b':', separator, ..]
                if drive.is_ascii_alphabetic() && matches!(separator, b'/' | b'\\')
        )
        || (!has_non_file_uri_scheme(path) && path_contains_path_separator(path))
}

fn has_non_file_uri_scheme(path: &str) -> bool {
    let Some((scheme, _)) = path.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

fn path_contains_path_separator(path: &str) -> bool {
    path.contains('/') || path.contains('\\')
}

fn file_destination(path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let normalized = absolute.to_string_lossy().replace('\\', "/");
    Some(format!("{FILE_SCHEME}{normalized}"))
}

fn relative_display_path(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let current = std::env::current_dir().ok();
    current
        .as_deref()
        .and_then(|cwd| absolute.strip_prefix(cwd).ok())
        .map(|relative| {
            let text = relative.to_string_lossy().replace('\\', "/");
            if text.is_empty() {
                CURRENT_DIR_LABEL.to_owned()
            } else {
                text
            }
        })
        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"))
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct LocalLinkTarget {
    path: String,
    location: String,
}

impl LocalLinkTarget {
    fn parse(raw: &str) -> Option<Self> {
        if raw.starts_with(FILE_SCHEME) {
            return parse_file_url_target(raw);
        }
        let (path, location) = split_local_path_location(raw);
        Some(Self {
            path: percent_decode(path),
            location,
        })
    }
}

fn parse_file_url_target(raw: &str) -> Option<LocalLinkTarget> {
    let url = Url::parse(raw).ok()?;
    let mut path = url
        .to_file_path()
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    path = percent_decode(&path);
    let location = url
        .fragment()
        .and_then(hash_fragment_location)
        .unwrap_or_default();
    Some(LocalLinkTarget { path, location })
}

fn split_local_path_location(raw: &str) -> (&str, String) {
    if let Some((path, fragment)) = raw.rsplit_once('#')
        && let Some(location) = hash_fragment_location(fragment)
    {
        return (path, location);
    }
    if let Some((path, location)) = colon_location(raw) {
        return (path, location);
    }
    (raw, String::new())
}

fn hash_fragment_location(fragment: &str) -> Option<String> {
    let value = fragment.strip_prefix(HASH_LINE_PREFIX)?;
    parse_hash_position_range(value).map(|range| range.to_suffix())
}

fn colon_location(raw: &str) -> Option<(&str, String)> {
    let (path, suffix) = trailing_colon_suffix(raw)?;
    let range = parse_colon_position_range(suffix)?;
    Some((path, range.to_suffix()))
}

fn trailing_colon_suffix(raw: &str) -> Option<(&str, &str)> {
    let start = raw
        .char_indices()
        .rev()
        .find_map(|(index, ch)| {
            (!(ch.is_ascii_digit() || ch == ':' || ch == LOCATION_SEPARATOR))
                .then_some(index + ch.len_utf8())
        })
        .unwrap_or(0);
    if start == raw.len() {
        return None;
    }
    let suffix = &raw[start..];
    suffix.starts_with(':').then_some((&raw[..start], suffix))
}

fn parse_hash_position_range(value: &str) -> Option<LocationRange> {
    let mut positions = Vec::new();
    for segment in value.split(LOCATION_SEPARATOR) {
        let segment = segment.strip_prefix(HASH_LINE_PREFIX).unwrap_or(segment);
        positions.push(parse_position(segment, 'C')?);
    }
    LocationRange::from_positions(positions)
}

fn parse_colon_position_range(value: &str) -> Option<LocationRange> {
    let mut positions = Vec::new();
    for segment in value.trim_start_matches(':').split(LOCATION_SEPARATOR) {
        positions.push(parse_position(segment, ':')?);
    }
    LocationRange::from_positions(positions)
}

fn parse_position(value: &str, separator: char) -> Option<LocationPosition> {
    let (line, column) = value
        .split_once(separator)
        .map_or((value, None), |(line, column)| (line, Some(column)));
    if line.is_empty() || !line.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let column = match column {
        Some(column) if !column.is_empty() && column.chars().all(|ch| ch.is_ascii_digit()) => {
            Some(column.to_owned())
        }
        Some(_) => return None,
        None => None,
    };
    Some(LocationPosition {
        line: line.to_owned(),
        column,
    })
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct LocationRange {
    start: LocationPosition,
    end: Option<LocationPosition>,
}

impl LocationRange {
    fn from_positions(positions: Vec<LocationPosition>) -> Option<Self> {
        match positions.as_slice() {
            [start] => Some(Self {
                start: start.clone(),
                end: None,
            }),
            [start, end] => Some(Self {
                start: start.clone(),
                end: Some(end.clone()),
            }),
            _ => None,
        }
    }

    fn to_suffix(&self) -> String {
        match &self.end {
            Some(end) => format!(":{}-{}", self.start, end),
            None => format!(":{}", self.start),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct LocationPosition {
    line: String,
    column: Option<String>,
}

impl std::fmt::Display for LocationPosition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.column {
            Some(column) => write!(formatter, "{}:{column}", self.line),
            None => formatter.write_str(&self.line),
        }
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(decoded) = percent_byte(bytes.get(index + 1), bytes.get(index + 2))
        {
            output.push(decoded);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).unwrap_or_else(|_| value.to_owned())
}

fn percent_byte(high: Option<&u8>, low: Option<&u8>) -> Option<u8> {
    Some(hex_value(*high?)? * 16 + hex_value(*low?)?)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_relative_path_with_colon_location() {
        let (display, destination) =
            display_path_for_link("crates/axon-tui-runtime/src/lib.rs:12:3")
                .expect("local path link should parse");

        assert_eq!(display, "crates/axon-tui-runtime/src/lib.rs:12:3");
        assert!(destination.starts_with(FILE_SCHEME));
        assert!(destination.ends_with("crates/axon-tui-runtime/src/lib.rs:12:3"));
    }

    #[test]
    fn displays_relative_path_with_colon_range() {
        let (display, destination) =
            display_path_for_link("crates/axon-tui-runtime/src/lib.rs:12:3-18:9")
                .expect("local path range link should parse");

        assert_eq!(display, "crates/axon-tui-runtime/src/lib.rs:12:3-18:9");
        assert!(destination.starts_with(FILE_SCHEME));
        assert!(destination.ends_with("crates/axon-tui-runtime/src/lib.rs:12:3-18:9"));
    }

    #[test]
    fn displays_file_url_with_decoded_hash_location() {
        let cwd = std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .replace('\\', "/");
        let target =
            format!("file://{cwd}/crates/axon-tui-runtime/src/Example%20File.rs#L74C3-L80C4");
        let (display, destination) = display_path_for_link(&target).expect("file URL should parse");

        assert_eq!(
            display,
            "crates/axon-tui-runtime/src/Example File.rs:74:3-80:4"
        );
        assert!(destination.ends_with("crates/axon-tui-runtime/src/Example File.rs:74:3-80:4"));
    }

    #[test]
    fn ignores_non_location_hash_fragment() {
        let (display, destination) =
            display_path_for_link("crates/axon-tui-runtime/src/lib.rs#architecture-notes")
                .expect("local hash path should parse");

        assert_eq!(
            display,
            "crates/axon-tui-runtime/src/lib.rs#architecture-notes"
        );
        assert!(destination.ends_with("crates/axon-tui-runtime/src/lib.rs#architecture-notes"));
    }

    #[test]
    fn ignores_non_path_like_destinations() {
        assert_eq!(display_path_for_link("mailto:test@example.com"), None);
        assert_eq!(display_path_for_link("urn:example:resource"), None);
        assert_eq!(display_path_for_link("https://example.com/docs"), None);
    }
}
