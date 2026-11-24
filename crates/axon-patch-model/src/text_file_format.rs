//! 文本文件格式继承。

const UTF8_BOM: char = '\u{feff}';
const CRLF: &str = "\r\n";
const LF: &str = "\n";

/// 将候选文本规范化为现有文件的 UTF-8 BOM、换行符和尾换行格式。
///
/// 现有单行文件没有可观察的换行风格，此时保留候选文本自身的换行风格。
#[must_use]
pub fn inherit_text_file_format(existing: &str, proposed: &str) -> String {
    let (existing_has_bom, existing_body) = strip_bom(existing);
    let (_, proposed_body) = strip_bom(proposed);
    let line_ending = detected_line_ending(existing_body)
        .or_else(|| detected_line_ending(proposed_body))
        .unwrap_or(LF);
    let trailing_newline = existing_body.ends_with('\n');
    let mut normalized = normalize_line_endings(proposed_body, line_ending);
    set_trailing_newline(&mut normalized, line_ending, trailing_newline);
    if existing_has_bom {
        normalized.insert(0, UTF8_BOM);
    }
    normalized
}

/// 返回文本的逻辑行数；空文本没有可见行。
#[must_use]
pub fn text_file_line_count(content: &str) -> usize {
    logical_lines(content).len()
}

/// 返回完整文本替换 diff 在新文件侧展示的行号（含指定上下文）。
#[must_use]
pub fn text_change_visible_lines(
    existing: &str,
    proposed: &str,
    context_radius: usize,
) -> Vec<usize> {
    let old_lines = logical_lines(existing);
    let new_lines = logical_lines(proposed);
    if old_lines == new_lines || new_lines.is_empty() {
        return Vec::new();
    }
    let prefix = common_prefix_len(&old_lines, &new_lines);
    let suffix = common_suffix_len(&old_lines, &new_lines, prefix);
    let changed_end = new_lines.len().saturating_sub(suffix);
    let visible_start = prefix.saturating_sub(context_radius);
    let visible_end = changed_end
        .saturating_add(context_radius)
        .min(new_lines.len());
    (visible_start + 1..=visible_end).collect()
}

fn logical_lines(content: &str) -> Vec<&str> {
    let (_, body) = strip_bom(content);
    if body.is_empty() {
        return Vec::new();
    }
    body.strip_suffix('\n')
        .unwrap_or(body)
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

fn common_prefix_len(old_lines: &[&str], new_lines: &[&str]) -> usize {
    old_lines
        .iter()
        .zip(new_lines)
        .take_while(|(old, new)| old == new)
        .count()
}

fn common_suffix_len(old_lines: &[&str], new_lines: &[&str], prefix: usize) -> usize {
    old_lines
        .iter()
        .rev()
        .zip(new_lines.iter().rev())
        .take_while(|(old, new)| old == new)
        .take(old_lines.len().saturating_sub(prefix))
        .take(new_lines.len().saturating_sub(prefix))
        .count()
}

fn strip_bom(content: &str) -> (bool, &str) {
    content
        .strip_prefix(UTF8_BOM)
        .map_or((false, content), |body| (true, body))
}

fn detected_line_ending(content: &str) -> Option<&'static str> {
    content.find('\n').map(|index| {
        if index > 0 && content.as_bytes()[index - 1] == b'\r' {
            CRLF
        } else {
            LF
        }
    })
}

fn normalize_line_endings(content: &str, line_ending: &str) -> String {
    let lf = content.replace(CRLF, LF).replace('\r', LF);
    if line_ending == CRLF {
        lf.replace(LF, CRLF)
    } else {
        lf
    }
}

fn set_trailing_newline(content: &mut String, line_ending: &str, expected: bool) {
    if expected && !content.ends_with('\n') {
        content.push_str(line_ending);
    } else if !expected {
        while content.ends_with('\n') {
            content.truncate(content.len() - 1);
            if content.ends_with('\r') {
                content.truncate(content.len() - 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherits_bom_crlf_and_trailing_newline() {
        let existing = "\u{feff}one\r\ntwo\r\n";
        let proposed = "one\nchanged";

        assert_eq!(
            inherit_text_file_format(existing, proposed),
            "\u{feff}one\r\nchanged\r\n"
        );
    }

    #[test]
    fn single_line_without_trailing_newline_keeps_proposed_line_style() {
        let existing = "one";
        let proposed = "one\r\ntwo\r\n";

        assert_eq!(inherit_text_file_format(existing, proposed), "one\r\ntwo");
    }

    #[test]
    fn visible_lines_cover_changed_region_and_bounded_context() {
        let existing = "one\ntwo\nold\nfour\nfive\nsix\n";
        let proposed = "one\ntwo\nnew\nfour\nfive\nsix\n";

        assert_eq!(
            text_change_visible_lines(existing, proposed, 1),
            vec![2, 3, 4]
        );
        assert_eq!(text_file_line_count(proposed), 6);
    }
}
