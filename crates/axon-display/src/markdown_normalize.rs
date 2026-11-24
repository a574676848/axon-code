//! 渠道无关的 Markdown 输入语义规范化。
//!
//! 模型有时会把有序步骤压成一行，例如 `1、分析 2、实施 3、验证`。
//! 这里先把明显连续的结构规范成 CommonMark，再交给各渠道 renderer。

use super::{
    markdown_fences::unwrap_markdown_fences,
    markdown_table::{
        FenceTracker, is_table_delimiter_line, is_table_header_line, table_candidate_text,
    },
};

const MIN_INLINE_ORDERED_LIST_MARKERS: usize = 2;
const MIN_INLINE_DASH_MARKERS: usize = 2;
const MAX_INLINE_METADATA_LABEL_CHARS: usize = 16;
const MIN_INLINE_CHINESE_SECTION_MARKERS: usize = 2;
const MAX_ATX_HEADING_MARKERS: usize = 6;
const MAX_COMMONMARK_INDENT: usize = 3;
const UTF8_BOM: char = '\u{feff}';
const CHINESE_SECTION_MARKERS: &[&str] = &[
    "问题一",
    "问题二",
    "问题三",
    "问题四",
    "问题五",
    "问题六",
    "问题七",
    "问题八",
    "问题九",
    "问题十",
];

/// 将模型输出规范为各展示渠道共享的 Markdown source。
///
/// 该入口统一处理 Markdown fence 中的表格、表格边界以及密集列表；终端宽度、
/// 颜色和消息前缀等渠道特有规则由各 renderer 在此之后处理。
#[must_use]
pub fn normalize_markdown_source(source: &str) -> String {
    let source = source.strip_prefix(UTF8_BOM).unwrap_or(source);
    let source = unwrap_markdown_fences(source);
    normalize_markdown_lines(&normalize_markdown_tables(&source))
}

fn normalize_markdown_lines(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut fence = FenceTracker::new();
    for line in source.split_inclusive('\n') {
        let was_inside = fence.is_inside();
        fence.advance(line);
        if was_inside || fence.is_inside() {
            output.push_str(line);
        } else {
            output.push_str(&normalize_markdown_line(line));
        }
    }
    output
}

fn normalize_markdown_line(line: &str) -> String {
    let line = normalize_atx_heading_line(line);
    normalize_inline_chinese_sections(&normalize_inline_dash_items(
        &normalize_inline_ordered_list_line(&line),
    ))
}

fn normalize_atx_heading_line(line: &str) -> String {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > MAX_COMMONMARK_INDENT {
        return line.to_owned();
    }
    let marker_len = line[indent..]
        .bytes()
        .take_while(|byte| *byte == b'#')
        .count();
    if marker_len == 0 || marker_len > MAX_ATX_HEADING_MARKERS {
        return line.to_owned();
    }
    let marker_end = indent + marker_len;
    let after_marker = &line[marker_end..];
    let Some(first) = after_marker.chars().next() else {
        return line.to_owned();
    };
    if matches!(first, ' ' | '\t' | '\r' | '\n') {
        return line.to_owned();
    }
    if is_heading_separator(first) {
        let content_start = marker_end
            + after_marker
                .char_indices()
                .find(|(_, ch)| !is_heading_separator(*ch))
                .map_or(after_marker.len(), |(index, _)| index);
        return replace_heading_separator(line, marker_end, content_start);
    }
    if is_cjk_heading_start(first) {
        return replace_heading_separator(line, marker_end, marker_end);
    }
    line.to_owned()
}

fn replace_heading_separator(line: &str, marker_end: usize, content_start: usize) -> String {
    let mut output = String::with_capacity(line.len() + 1);
    output.push_str(&line[..marker_end]);
    output.push(' ');
    output.push_str(&line[content_start..]);
    output
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

fn normalize_inline_ordered_list_line(line: &str) -> String {
    let markers = inline_ordered_markers(line);
    if markers.len() < MIN_INLINE_ORDERED_LIST_MARKERS {
        return line.to_owned();
    }
    let mut output = String::with_capacity(line.len() + markers.len() * 2);
    let mut previous = 0usize;
    for marker in markers {
        output.push_str(&line[previous..marker.start]);
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&line[marker.start..marker.number_end]);
        output.push_str(". ");
        previous = marker.delimiter_end;
    }
    output.push_str(&line[previous..]);
    output
}

fn normalize_inline_dash_items(line: &str) -> String {
    line.split_inclusive('\n')
        .map(normalize_inline_dash_segment)
        .collect()
}

fn normalize_inline_dash_segment(line: &str) -> String {
    let markers = inline_dash_markers(line);
    if markers.is_empty()
        || (markers.len() < MIN_INLINE_DASH_MARKERS
            && !single_labeled_metadata_marker(line, markers[0]))
    {
        return line.to_owned();
    }
    let mut output = String::with_capacity(line.len() + markers.len() * 2);
    output.push_str(&line[..markers[0].start]);
    for (index, marker) in markers.iter().copied().enumerate() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("- ");
        let item_end = markers.get(index + 1).map_or(line.len(), |next| next.start);
        output.push_str(line[marker.content_start..item_end].trim_start());
    }
    output
}

#[derive(Debug, Clone, Copy)]
struct InlineDashMarker {
    start: usize,
    content_start: usize,
}

fn inline_dash_markers(line: &str) -> Vec<InlineDashMarker> {
    line.char_indices()
        .filter_map(|(index, ch)| {
            (ch == '-')
                .then(|| inline_dash_marker_at(line, index))
                .flatten()
        })
        .collect()
}

fn inline_dash_marker_at(line: &str, dash_index: usize) -> Option<InlineDashMarker> {
    if !line[..dash_index]
        .chars()
        .next_back()
        .is_some_and(char::is_whitespace)
    {
        return None;
    }
    let left = &line[..dash_index];
    let start = left
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(0, |(index, ch)| index + ch.len_utf8());
    if start == dash_index {
        return None;
    }
    let after_dash = dash_index + '-'.len_utf8();
    let content_start = after_dash + leading_whitespace_len(&line[after_dash..]);
    if content_start == after_dash || !dash_marker_is_item_boundary(line, start, content_start) {
        return None;
    }
    Some(InlineDashMarker {
        start,
        content_start,
    })
}

fn dash_marker_is_item_boundary(line: &str, start: usize, content_start: usize) -> bool {
    if line[..start].trim().is_empty() {
        return false;
    }
    line[content_start..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '*' | '`' | '[' | '【'))
}

fn single_labeled_metadata_marker(line: &str, marker: InlineDashMarker) -> bool {
    let rest = &line[marker.content_start..];
    let Some(label_end) = rest.find([':', '：']) else {
        return false;
    };
    let label = rest[..label_end].trim().trim_matches('*').trim();
    let label_chars = label.chars().count();
    label_chars > 0
        && label_chars <= MAX_INLINE_METADATA_LABEL_CHARS
        && label.chars().any(char::is_alphanumeric)
}

fn normalize_inline_chinese_sections(line: &str) -> String {
    line.split_inclusive('\n')
        .map(normalize_inline_chinese_section_segment)
        .collect()
}

fn normalize_inline_chinese_section_segment(line: &str) -> String {
    let markers = inline_chinese_section_markers(line);
    if markers.len() < MIN_INLINE_CHINESE_SECTION_MARKERS {
        return line.to_owned();
    }
    let mut output = String::with_capacity(line.len() + markers.len());
    let mut previous = 0usize;
    for marker in markers {
        output.push_str(&line[previous..marker]);
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        previous = marker;
    }
    output.push_str(&line[previous..]);
    output
}

fn inline_chinese_section_markers(line: &str) -> Vec<usize> {
    line.char_indices()
        .filter_map(|(index, _)| chinese_section_marker_at(line, index).then_some(index))
        .collect()
}

fn chinese_section_marker_at(line: &str, index: usize) -> bool {
    let rest = &line[index..];
    let Some(marker) = CHINESE_SECTION_MARKERS
        .iter()
        .find(|marker| rest.starts_with(**marker))
    else {
        return false;
    };
    let after_marker = &rest[marker.len()..];
    if !after_marker.starts_with(['、', ':', '：']) {
        return false;
    }
    marker_has_valid_left_boundary(line, index)
}

#[derive(Debug, Clone, Copy)]
struct InlineOrderedMarker {
    start: usize,
    number_end: usize,
    delimiter_end: usize,
}

fn inline_ordered_markers(line: &str) -> Vec<InlineOrderedMarker> {
    let mut markers = Vec::new();
    let mut cursor = 0usize;
    while cursor < line.len() {
        let Some((relative_start, marker, _number)) = find_next_ordered_marker(&line[cursor..])
        else {
            break;
        };
        let marker = marker.shifted(cursor);
        markers.push(marker);
        cursor = marker.delimiter_end.max(cursor + relative_start + 1);
    }
    markers
}

fn find_next_ordered_marker(source: &str) -> Option<(usize, InlineOrderedMarker, u32)> {
    let mut iter = source.char_indices().peekable();
    while let Some((index, ch)) = iter.next() {
        if !ch.is_ascii_digit() || !marker_has_valid_left_boundary(source, index) {
            continue;
        }
        let number_start = index;
        let mut number_end = index + ch.len_utf8();
        while let Some((next_index, next_ch)) = iter.peek().copied() {
            if !next_ch.is_ascii_digit() {
                break;
            }
            number_end = next_index + next_ch.len_utf8();
            iter.next();
        }
        let Some(delimiter_end) = ordered_marker_delimiter_end(source, number_end) else {
            continue;
        };
        let Ok(number) = source[number_start..number_end].parse::<u32>() else {
            continue;
        };
        return Some((
            number_start,
            InlineOrderedMarker {
                start: number_start,
                number_end,
                delimiter_end,
            },
            number,
        ));
    }
    None
}

fn ordered_marker_delimiter_end(source: &str, number_end: usize) -> Option<usize> {
    let delimiter = source[number_end..].chars().next()?;
    if !matches!(delimiter, '.' | '、' | ',' | '，' | ')' | '）') {
        return None;
    }
    let delimiter_end = number_end + delimiter.len_utf8();
    let after_delimiter = &source[delimiter_end..];
    if delimiter_requires_space(delimiter)
        && after_delimiter.starts_with(|ch: char| ch.is_ascii_digit())
    {
        return None;
    }
    let content_start = delimiter_end + leading_whitespace_len(after_delimiter);
    if !ordered_marker_has_content_boundary(source, content_start) {
        return None;
    }
    Some(content_start)
}

fn delimiter_requires_space(delimiter: char) -> bool {
    matches!(delimiter, '.' | ',' | '，')
}

fn ordered_marker_has_content_boundary(source: &str, content_start: usize) -> bool {
    let Some(ch) = source[content_start..].chars().next() else {
        return false;
    };
    ch.is_alphanumeric() || matches!(ch, '*' | '`' | '[' | '【' | '《' | '“' | '"')
}

fn leading_whitespace_len(value: &str) -> usize {
    value
        .char_indices()
        .find(|(_, ch)| !ch.is_whitespace())
        .map_or(value.len(), |(index, _)| index)
}

fn marker_has_valid_left_boundary(source: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }
    source[..index].chars().next_back().is_some_and(|ch| {
        ch.is_whitespace()
            || is_cjk_text(ch)
            || matches!(
                ch,
                '。' | '；'
                    | ';'
                    | ':'
                    | '：'
                    | ','
                    | '，'
                    | '、'
                    | ')'
                    | '）'
                    | ']'
                    | '】'
                    | '»'
                    | '”'
            )
    })
}

fn is_cjk_text(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{3040}'..='\u{30ff}'
            | '\u{ac00}'..='\u{d7af}'
    )
}

fn normalize_markdown_tables(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut previous_table_line = false;
    let mut fence = FenceTracker::new();
    for line in source.split_inclusive('\n') {
        let candidate = (!fence.is_inside())
            .then(|| table_candidate_text(line))
            .flatten();
        let table_line = candidate
            .is_some_and(|text| is_table_header_line(text) || is_table_delimiter_line(text));
        if table_line && !previous_table_line && output_needs_table_gap(&output) {
            output.push('\n');
        }
        output.push_str(line);
        if table_line && candidate.is_some_and(is_table_delimiter_line) && !line.ends_with('\n') {
            output.push('\n');
        }
        previous_table_line = table_line;
        fence.advance(line);
    }
    output
}

fn output_needs_table_gap(output: &str) -> bool {
    !output.is_empty() && !output.ends_with("\n\n")
}

impl InlineOrderedMarker {
    const fn shifted(self, offset: usize) -> Self {
        Self {
            start: self.start + offset,
            number_end: self.number_end + offset,
            delimiter_end: self.delimiter_end + offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_markdown_source;

    #[test]
    fn normalizes_inline_chinese_ordered_steps() {
        assert_eq!(
            normalize_markdown_source("1、分析问题 2、实施修复 3、验证结果"),
            "1. 分析问题 \n2. 实施修复 \n3. 验证结果"
        );
    }

    #[test]
    fn normalizes_inline_commonmark_ordered_steps() {
        assert_eq!(
            normalize_markdown_source("1. Analyse 2. Fix 3. Verify"),
            "1. Analyse \n2. Fix \n3. Verify"
        );
    }

    #[test]
    fn splits_intro_before_dense_ordered_steps() {
        assert_eq!(
            normalize_markdown_source("我能帮你做什么 1. 读代码 2. 改代码 3. 写测试"),
            "我能帮你做什么 \n1. 读代码 \n2. 改代码 \n3. 写测试"
        );
    }

    #[test]
    fn splits_cjk_adjacent_ordered_steps() {
        assert_eq!(
            normalize_markdown_source(
                "建议的领域模型1. Provider（供应商）2. Action（动作定义）3. ConnectorAction（连接器动作绑定）"
            ),
            "建议的领域模型\n1. Provider（供应商）\n2. Action（动作定义）\n3. ConnectorAction（连接器动作绑定）"
        );
    }

    #[test]
    fn splits_inline_dash_items_from_status_text() {
        assert_eq!(
            normalize_markdown_source(
                "实施范围 - ova_mcp 已检索全部工具 - 本机 schema list 已完成 - 下一步继续修复"
            ),
            "实施范围\n- ova_mcp 已检索全部工具\n- 本机 schema list 已完成\n- 下一步继续修复"
        );
    }

    #[test]
    fn keeps_inline_hyphens_and_emphasis_unchanged() {
        let source = "路径 E:\\zbg\\axon-code - 不拆分 foo-bar *强调文本*";
        assert_eq!(normalize_markdown_source(source), source);
    }

    #[test]
    fn splits_dense_dash_items_after_ordered_intro() {
        assert_eq!(
            normalize_markdown_source("1. 读代码 - 讲架构 - 找调用 2. 改代码 3. 验证"),
            "1. 读代码\n- 讲架构\n- 找调用 \n2. 改代码 \n3. 验证"
        );
    }

    #[test]
    fn keeps_short_or_non_sequential_text_unchanged() {
        assert_eq!(
            normalize_markdown_source("版本 3.5 和 4.0"),
            "版本 3.5 和 4.0"
        );
        assert_eq!(normalize_markdown_source("1、只有一项"), "1、只有一项");
    }

    #[test]
    fn normalizes_dense_steps_with_mixed_delimiters_and_gaps() {
        assert_eq!(
            normalize_markdown_source("1.**读代码** - 看架构 - 找调用 2）改代码 4, 跑验证 5，复盘"),
            "1. **读代码**\n- 看架构\n- 找调用 \n2. 改代码 \n4. 跑验证 \n5. 复盘"
        );
    }

    #[test]
    fn normalizes_dense_steps_after_chinese_punctuation() {
        assert_eq!(
            normalize_markdown_source("步骤：1、读取报告，2、对照源码，3、实施验证"),
            "步骤：\n1. 读取报告，\n2. 对照源码，\n3. 实施验证"
        );
    }

    #[test]
    fn keeps_numeric_punctuation_unchanged() {
        assert_eq!(
            normalize_markdown_source("版本 3.5、金额 1,000、比例 2,500.25"),
            "版本 3.5、金额 1,000、比例 2,500.25"
        );
    }

    #[test]
    fn normalizes_dense_chinese_problem_sections() {
        assert_eq!(
            normalize_markdown_source("问题一、重复渲染 问题二：0ms 问题三、文字太密"),
            "问题一、重复渲染 \n问题二：0ms \n问题三、文字太密"
        );
    }

    #[test]
    fn separates_table_from_preceding_paragraph() {
        assert_eq!(
            normalize_markdown_source(
                "建议优先排这些任务：\n| 优先级 | 优化任务 |\n| --- | --- |\n| P0 | 配置示例 |"
            ),
            "建议优先排这些任务：\n\n| 优先级 | 优化任务 |\n| --- | --- |\n| P0 | 配置示例 |"
        );
    }

    #[test]
    fn appends_missing_newline_after_table_delimiter() {
        assert_eq!(
            normalize_markdown_source("| A | B |\n| --- | --- |"),
            "| A | B |\n| --- | --- |\n"
        );
    }

    #[test]
    fn keeps_pipe_lines_inside_non_markdown_fence_unchanged() {
        assert_eq!(
            normalize_markdown_source("```rust\n| A | B |\n| --- | --- |\n```"),
            "```rust\n| A | B |\n| --- | --- |\n```"
        );
    }

    #[test]
    fn normalizes_high_confidence_cjk_atx_headings() {
        assert_eq!(normalize_markdown_source("##标题"), "## 标题");
        assert_eq!(normalize_markdown_source("#\u{3000}标题\r\n"), "# 标题\r\n");
        assert_eq!(normalize_markdown_source("\u{feff}###标题"), "### 标题");
    }

    #[test]
    fn keeps_literal_hash_prefixes_outside_high_confidence_heading_cases() {
        for source in [
            "\\##标题",
            "#include <stdio.h>",
            "##Title",
            "#123",
            "#fff",
            "#!powershell",
            "    ##标题",
            "`##标题`",
        ] {
            assert_eq!(normalize_markdown_source(source), source);
        }
    }

    #[test]
    fn splits_heading_metadata_with_unicode_whitespace_immediately() {
        assert_eq!(
            normalize_markdown_source("### 任务标题\u{3000}-\u{00a0}状态：进行中"),
            "### 任务标题\n- 状态：进行中"
        );
    }

    #[test]
    fn splits_dense_task_metadata_without_vendor_prefix() {
        assert_eq!(
            normalize_markdown_source("任务：克隆仓库 - 状态：进行中 - 目标：E:\\zbg\\grok-build"),
            "任务：克隆仓库\n- 状态：进行中\n- 目标：E:\\zbg\\grok-build"
        );
    }

    #[test]
    fn keeps_non_metadata_single_heading_dash_unchanged() {
        assert_eq!(
            normalize_markdown_source("### API - Design Notes"),
            "### API - Design Notes"
        );
    }

    #[test]
    fn keeps_all_fenced_code_content_byte_exact() {
        let source = concat!(
            "正文 1、分析 2、实施 3、验证\n\n",
            "```mermaid\n",
            "erDiagram\n",
            "    PROVIDER ||--o{ ACTION : contains\n",
            "        1、字段 2、关系 3、索引\n",
            "```\n\n",
            "```json\n",
            "{\"task\": \"A - 状态：进行中\"}\n",
            "```\n\n",
            "```bash\n",
            "printf '1、读取 2、执行 3、验证\\n'\n",
            "```",
        );

        let normalized = normalize_markdown_source(source);

        assert!(normalized.starts_with("正文 \n1. 分析 \n2. 实施 \n3. 验证"));
        assert!(normalized.contains(
            "```mermaid\nerDiagram\n    PROVIDER ||--o{ ACTION : contains\n        1、字段 2、关系 3、索引\n```"
        ));
        assert!(normalized.contains("```json\n{\"task\": \"A - 状态：进行中\"}\n```"));
        assert!(normalized.contains("```bash\nprintf '1、读取 2、执行 3、验证\\n'\n```"));
    }

    #[test]
    fn normalization_is_idempotent() {
        let source = "```markdown\n| 文件 | 状态 |\n| --- | --- |\n| a.rs | ok |\n```\n\n### 任务标题 - 状态：进行中";
        let normalized = normalize_markdown_source(source);

        assert_eq!(normalize_markdown_source(&normalized), normalized);
    }
}
