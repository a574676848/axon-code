//! 反思候选的报告契约。
//!
//! 反思子代理不再自己写记忆，只交一份候选清单；准入、配额与三态动作由
//! runtime 在收口时统一执行。这样「写了几条、为什么写」不再是子代理的自觉，
//! 而是可复核的运行时行为。

use serde::{Deserialize, Serialize};

use crate::SemanticMemoryCandidateType;

/// 单次反思最多接受的候选条数；超出部分丢弃，防止一次吐一屏。
pub const REFLECTION_REPORT_MAX_CANDIDATES: usize = 8;
/// 单次反思最多接受的跳过说明条数。
pub const REFLECTION_REPORT_MAX_SKIPS: usize = 16;
/// 候选正文与证据的长度上限，单位字符。
pub const REFLECTION_REPORT_MAX_TEXT_CHARS: usize = 600;

/// 一条候选草稿，对应一次待写入的记忆。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionCandidateDraft {
    /// 记忆类型。
    pub kind: SemanticMemoryCandidateType,
    /// 记忆正文。
    pub summary: String,
    /// 证据说明。
    pub evidence: String,
    /// 支撑这次候选的证据引用；缺省时由评估结果兜底。
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    /// 淘汰或归档的目标记忆；仅当判定既有记忆失效时给出。
    #[serde(default)]
    pub target_memory_id: Option<i64>,
}

/// 一条被明确跳过的候选。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionSkipDraft {
    /// 被跳过的类型；模型没给就留空。
    #[serde(default)]
    pub kind: Option<SemanticMemoryCandidateType>,
    /// 跳过理由。
    pub reason: String,
}

/// 反思子代理的完整报告。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReflectionReportDraft {
    /// 候选清单。
    #[serde(default)]
    pub candidates: Vec<ReflectionCandidateDraft>,
    /// 跳过清单。
    #[serde(default)]
    pub skipped: Vec<ReflectionSkipDraft>,
}

impl ReflectionReportDraft {
    /// 清洗：去空白、丢弃空条目、按上限截断、按运行时证据索引过滤引用。
    ///
    /// 返回清洗后的报告与被丢弃的候选条数，调用方据此写回执。
    #[must_use]
    pub fn sanitized(mut self, allowed_evidence_refs: &[String]) -> (Self, usize) {
        let before = self.candidates.len();
        self.candidates.retain(|candidate| {
            !candidate.summary.trim().is_empty() && !candidate.evidence.trim().is_empty()
        });
        let dropped = before.saturating_sub(self.candidates.len());
        self.candidates.truncate(REFLECTION_REPORT_MAX_CANDIDATES);
        self.skipped.retain(|skip| !skip.reason.trim().is_empty());
        self.skipped.truncate(REFLECTION_REPORT_MAX_SKIPS);
        for candidate in &mut self.candidates {
            candidate.summary =
                truncate_chars(candidate.summary.trim(), REFLECTION_REPORT_MAX_TEXT_CHARS);
            candidate.evidence =
                truncate_chars(candidate.evidence.trim(), REFLECTION_REPORT_MAX_TEXT_CHARS);
            // 只保留落在运行时证据索引里的引用：模型可以编一个 receipt:99，
            // 不能让它带着假引用过准入。
            candidate.evidence_refs.retain(|reference| {
                !reference.trim().is_empty()
                    && allowed_evidence_refs
                        .iter()
                        .any(|allowed| allowed == reference)
            });
            // 一条都不剩时用索引兜底：索引本身来自运行时，兜底不会引入假引用。
            if candidate.evidence_refs.is_empty() {
                candidate.evidence_refs = allowed_evidence_refs.to_vec();
            }
        }
        (self, dropped)
    }
}

/// 从子代理正文里取出候选报告。
///
/// 取「最后一个能解析成报告的顶层对象」：模型常在 JSON 前写一段解释，取最后一个比
/// 「第一个左花括号」稳。必须带 `candidates` 或 `skipped` 字段才算数，否则一段无关
/// JSON 会被当成空报告，静默吞掉本轮的反思产出。
///
/// 之所以逐个 `{` 当起点试、而不是先配对出「最后一个完整对象」：正文里只要出现一个
/// 未闭合的 `{`（半截代码块、引用片段），按深度配对的扫描就会一直停在未闭合状态，
/// 把后面完好的报告一起判成不存在。逐个起点尝试能把这种标点噪声隔离掉。
#[must_use]
pub fn parse_reflection_report(content: &str) -> Option<ReflectionReportDraft> {
    for (index, character) in content.char_indices().rev() {
        if character != '{' {
            continue;
        }
        let Some(object) = balanced_object_at(content, index) else {
            continue;
        };
        if !object.contains("\"candidates\"") && !object.contains("\"skipped\"") {
            continue;
        }
        if let Ok(report) = serde_json::from_str::<ReflectionReportDraft>(object) {
            return Some(report);
        }
    }
    None
}

/// 从 `start`（必须是 `{`）起找配对的右花括号；字符串内的括号不参与配对。
fn balanced_object_at(text: &str, start: usize) -> Option<&str> {
    let rest = text.get(start..)?;
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, character) in rest.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        match character {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return text.get(start..start + offset + character.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    text.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_report_after_prose_and_fences() {
        let content = "先说明一下本轮做了什么。\n```json\n{\"candidates\":[{\"kind\":\"knowledge\",\"summary\":\"结论\",\"evidence\":\"证据\"}],\"skipped\":[]}\n```";
        let report = parse_reflection_report(content).expect("应能解析");

        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report.candidates[0].kind,
            SemanticMemoryCandidateType::Knowledge
        );
    }

    #[test]
    fn takes_the_last_complete_object_only() {
        let content = "样例 {\"candidates\":[{\"kind\":\"preference\",\"summary\":\"a\",\"evidence\":\"b\"}]} 然后真正的 {\"candidates\":[]}";
        let report = parse_reflection_report(content).expect("应能解析");

        assert!(report.candidates.is_empty(), "必须取最后一个对象");
    }

    #[test]
    fn unrelated_json_is_not_mistaken_for_a_report() {
        assert!(parse_reflection_report("{\"status\":\"ok\"}").is_none());
        assert!(parse_reflection_report("没有 JSON").is_none());
    }

    #[test]
    fn unclosed_brace_before_the_report_does_not_swallow_it() {
        let content =
            "下面引一段没写完的代码 { fn main( 然后才是报告：{\"candidates\":[],\"skipped\":[]}";
        let report = parse_reflection_report(content).expect("未闭合的 { 不能把报告吞掉");

        assert!(report.candidates.is_empty());
    }

    #[test]
    fn sanitize_drops_empty_and_caps_length() {
        let mut candidates = Vec::new();
        for index in 0..10 {
            candidates.push(ReflectionCandidateDraft {
                kind: SemanticMemoryCandidateType::Procedure,
                summary: format!("内容 {index}"),
                evidence: "证据".to_owned(),
                evidence_refs: Vec::new(),
                target_memory_id: None,
            });
        }
        candidates.push(ReflectionCandidateDraft {
            kind: SemanticMemoryCandidateType::Procedure,
            summary: "   ".to_owned(),
            evidence: "证据".to_owned(),
            evidence_refs: Vec::new(),
            target_memory_id: None,
        });
        let report = ReflectionReportDraft {
            candidates,
            skipped: Vec::new(),
        };

        let (sanitized, dropped) = report.sanitized(&["receipt:1".to_owned()]);

        assert_eq!(dropped, 1, "空正文必须被丢弃并计数");
        assert_eq!(sanitized.candidates.len(), REFLECTION_REPORT_MAX_CANDIDATES);
        assert_eq!(
            sanitized.candidates[0].evidence_refs,
            vec!["receipt:1".to_owned()],
            "缺证据引用时用评估结果兜底"
        );
    }
}
