//! 用户问答消息正文与结构化回执编码器（UQ-010）。
//!
//! 规定唯一编码格式：
//! ```text
//! <逐题答案可见摘要>
//!
//! [问答回执]
//! 回答待决问题：<callId>
//! 回答内容：<JSON>
//! ```
//! 并提供统一的剥离与解析函数，供全端（CLI/TUI/桌面/Web）保持逐字一致。

use axon_domain::{UserAnswerItem, UserQuestionItem};

/// 问答结构化回执块标题标识。
pub const QUESTION_RECEIPT_BLOCK_HEADER: &str = "[问答回执]";
/// 回答问题 ID 引导前缀。
pub const QUESTION_RECEIPT_CALL_PREFIX: &str = "回答待决问题：";
/// 回答 JSON 内容引导前缀。
pub const QUESTION_RECEIPT_CONTENT_PREFIX: &str = "回答内容：";

/// 将问题与用户答案编码为可持久化的用户消息正文。
pub fn encode_question_answer_message(
    call_id: &str,
    questions: &[UserQuestionItem],
    answers: &[UserAnswerItem],
) -> String {
    let summary = format_answer_summary(questions, answers);
    let answers_json = serde_json::to_string(answers).unwrap_or_else(|_| "[]".to_string());

    if summary.is_empty() {
        format!(
            "{QUESTION_RECEIPT_BLOCK_HEADER}\n{QUESTION_RECEIPT_CALL_PREFIX}{call_id}\n{QUESTION_RECEIPT_CONTENT_PREFIX}{answers_json}"
        )
    } else {
        format!(
            "{summary}\n\n{QUESTION_RECEIPT_BLOCK_HEADER}\n{QUESTION_RECEIPT_CALL_PREFIX}{call_id}\n{QUESTION_RECEIPT_CONTENT_PREFIX}{answers_json}"
        )
    }
}

/// 格式化逐题答案的可见摘要。
pub fn format_answer_summary(questions: &[UserQuestionItem], answers: &[UserAnswerItem]) -> String {
    let mut lines = Vec::new();
    for (idx, q) in questions.iter().enumerate() {
        if let Some(ans) = answers.iter().find(|a| a.id == q.id) {
            let num = idx + 1;
            let choice_str = if let Some(custom) = &ans.custom {
                if !custom.trim().is_empty() {
                    custom.clone()
                } else if !ans.selected.is_empty() {
                    ans.selected.join("、")
                } else {
                    "（跳过）".to_string()
                }
            } else if !ans.selected.is_empty() {
                ans.selected.join("、")
            } else {
                "（跳过）".to_string()
            };
            lines.push(format!("{num}. {} 回答：{}", q.question, choice_str));
        }
    }
    lines.join("\n")
}

/// 剥离正文中的 `[问答回执]` 结构化块，仅保留可见正文。
pub fn strip_question_receipt_block(content: &str) -> String {
    if let Some(idx) = content.find(QUESTION_RECEIPT_BLOCK_HEADER) {
        content[..idx].trim_end().to_string()
    } else {
        content.to_string()
    }
}

/// 从消息正文中解析结构化问答回执 `(call_id, answers_json)`。
pub fn extract_question_receipt(content: &str) -> Option<(String, String)> {
    let header_idx = content.find(QUESTION_RECEIPT_BLOCK_HEADER)?;
    let block = &content[header_idx + QUESTION_RECEIPT_BLOCK_HEADER.len()..];

    let mut call_id = None;
    let mut answer_json = None;

    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(QUESTION_RECEIPT_CALL_PREFIX) {
            call_id = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix(QUESTION_RECEIPT_CONTENT_PREFIX) {
            answer_json = Some(rest.trim().to_string());
        }
    }

    match (call_id, answer_json) {
        (Some(cid), Some(ajson)) if !cid.is_empty() && !ajson.is_empty() => Some((cid, ajson)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_and_extract_and_strip() {
        let questions = vec![UserQuestionItem {
            id: "q1".to_string(),
            question: "是否继续部署？".to_string(),
            header: None,
            detail: None,
            options: vec![],
            multi_select: false,
        }];
        let answers = vec![UserAnswerItem {
            id: "q1".to_string(),
            selected: vec!["确认部署".to_string()],
            custom: None,
        }];

        let msg = encode_question_answer_message("call-deploy-99", &questions, &answers);

        // 验证结构
        assert!(msg.contains("1. 是否继续部署？ 回答：确认部署"));
        assert!(msg.contains("[问答回执]"));
        assert!(msg.contains("回答待决问题：call-deploy-99"));
        assert!(msg.contains("回答内容："));

        // 验证剥离可见正文
        let stripped = strip_question_receipt_block(&msg);
        assert_eq!(stripped, "1. 是否继续部署？ 回答：确认部署");
        assert!(!stripped.contains("[问答回执]"));

        // 验证提取
        let extracted = extract_question_receipt(&msg).expect("必须可提取");
        assert_eq!(extracted.0, "call-deploy-99");
        let parsed_answers: Vec<UserAnswerItem> =
            serde_json::from_str(&extracted.1).expect("反序列化");
        assert_eq!(parsed_answers, answers);
    }
}
