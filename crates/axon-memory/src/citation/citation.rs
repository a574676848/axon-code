//! Memory citation 解析。

use axon_domain::MemoryId;

/// Memory citation 起始标签。
pub const MEMORY_CITATION_START_TAG: &str = "<oai-mem-citation>";
/// Memory citation 结束标签。
pub const MEMORY_CITATION_END_TAG: &str = "</oai-mem-citation>";

const ENTRIES_START_TAG: &str = "<citation_entries>";
const ENTRIES_END_TAG: &str = "</citation_entries>";
const MEMORY_ENTRY_PREFIX: &str = "memory:";

/// 单条 Memory citation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCitation {
    /// Memory ID。
    pub memory_id: MemoryId,
    /// 可选备注。
    pub note: Option<String>,
}

/// citation 解析问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCitationParseIssue {
    /// 原始条目。
    pub entry: String,
    /// 原因。
    pub reason: &'static str,
}

/// citation 解析报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryCitationParseReport {
    /// 有效 citation。
    pub citations: Vec<MemoryCitation>,
    /// 无效 citation。
    pub issues: Vec<MemoryCitationParseIssue>,
}

/// Memory citation parser。
pub struct MemoryCitationParser;

impl MemoryCitationParser {
    /// 从模型输出中解析 Memory citation。
    #[must_use]
    pub fn parse(text: &str) -> MemoryCitationParseReport {
        let Some(block) = between(text, MEMORY_CITATION_START_TAG, MEMORY_CITATION_END_TAG) else {
            return MemoryCitationParseReport::default();
        };
        let Some(entries) = between(block, ENTRIES_START_TAG, ENTRIES_END_TAG) else {
            return MemoryCitationParseReport {
                citations: Vec::new(),
                issues: vec![MemoryCitationParseIssue {
                    entry: block.trim().to_owned(),
                    reason: "missing_entries",
                }],
            };
        };
        parse_entries(entries)
    }
}

/// 生成面向用户的文本投影，隐藏模型输出中的内部 Memory citation 标记。
///
/// 输入可以是持续增长的流式累计文本。若 citation 块或起始标签尚未接收完整，
/// 对应尾部会暂缓展示，避免把半截内部协议泄漏给用户。
#[must_use]
pub fn project_memory_citation_text(text: &str) -> String {
    let mut visible = String::with_capacity(text.len());
    let mut remaining = text;
    loop {
        let Some(start_index) = remaining.find(MEMORY_CITATION_START_TAG) else {
            let holdback = trailing_start_tag_prefix_len(remaining);
            visible.push_str(&remaining[..remaining.len() - holdback]);
            return visible;
        };
        visible.push_str(&remaining[..start_index]);
        let citation = &remaining[start_index + MEMORY_CITATION_START_TAG.len()..];
        let Some(end_index) = citation.find(MEMORY_CITATION_END_TAG) else {
            return visible;
        };
        remaining = &citation[end_index + MEMORY_CITATION_END_TAG.len()..];
    }
}

fn trailing_start_tag_prefix_len(text: &str) -> usize {
    (1..MEMORY_CITATION_START_TAG.len())
        .rev()
        .find(|length| text.ends_with(&MEMORY_CITATION_START_TAG[..*length]))
        .unwrap_or_default()
}

fn parse_entries(entries: &str) -> MemoryCitationParseReport {
    let mut report = MemoryCitationParseReport::default();
    for entry in entries
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        match parse_entry(entry) {
            Ok(citation) => report.citations.push(citation),
            Err(reason) => report.issues.push(MemoryCitationParseIssue {
                entry: entry.to_owned(),
                reason,
            }),
        }
    }
    report
}

fn parse_entry(entry: &str) -> Result<MemoryCitation, &'static str> {
    let (id_part, note_part) = entry.split_once('|').unwrap_or((entry, ""));
    let Some(id_text) = id_part.trim().strip_prefix(MEMORY_ENTRY_PREFIX) else {
        return Err("missing_memory_prefix");
    };
    let memory_id = id_text
        .trim()
        .parse::<u64>()
        .map(MemoryId)
        .map_err(|_| "invalid_memory_id")?;
    if memory_id.0 == 0 {
        return Err("invalid_memory_id");
    }
    let note = note_part
        .trim()
        .strip_prefix("note=[")
        .and_then(|value| value.strip_suffix(']'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    Ok(MemoryCitation { memory_id, note })
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_index = text.find(start)? + start.len();
    let end_index = text[start_index..].find(end)? + start_index;
    Some(&text[start_index..end_index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_extracts_memory_citation_entries() {
        let report = MemoryCitationParser::parse(
            "<oai-mem-citation><citation_entries>\nmemory:7|note=[used]\n</citation_entries></oai-mem-citation>",
        );

        assert_eq!(report.citations.len(), 1);
        assert_eq!(report.citations[0].memory_id, MemoryId(7));
        assert_eq!(report.citations[0].note.as_deref(), Some("used"));
        assert!(report.issues.is_empty());
    }

    #[test]
    fn presentation_hides_complete_and_multiple_citation_blocks() {
        let text = "结论一<oai-mem-citation><citation_entries>memory:7</citation_entries></oai-mem-citation>\n结论二<oai-mem-citation><citation_entries>memory:8</citation_entries></oai-mem-citation>";

        assert_eq!(project_memory_citation_text(text), "结论一\n结论二");
    }

    #[test]
    fn presentation_holds_back_incomplete_citation_block() {
        let text = "可见结论<oai-mem-citation><citation_entries>memory:7";

        assert_eq!(project_memory_citation_text(text), "可见结论");
    }

    #[test]
    fn presentation_holds_back_partial_start_tag() {
        assert_eq!(
            project_memory_citation_text("可见结论<oai-mem-cit"),
            "可见结论"
        );
        assert_eq!(project_memory_citation_text("普通 < 标签"), "普通 < 标签");
    }
}
