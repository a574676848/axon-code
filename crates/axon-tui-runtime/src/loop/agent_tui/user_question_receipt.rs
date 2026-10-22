//! 用户问答回执块剥离（UQ-205）。
//!
//! 唯一编码格式由 `axon-cli` 的 `user_question::content` 编码器规定：一段可见摘要
//! 之后接标题行、`回答待决问题：` 与 `回答内容：` 两行；摘要为空时以标题行开头。
//!
//! 因此这里按**整块校验**再剥离：标题必须独占一行，且后面紧跟引导前缀与取值。
//! 正文里恰好出现同名标题（例如用户复述这段格式）不会被误剥。

/// 问答结构化回执块标题标识。
const QUESTION_RECEIPT_BLOCK_HEADER: &str = "[问答回执]";
/// 回答问题 ID 引导前缀。
const QUESTION_RECEIPT_CALL_PREFIX: &str = "回答待决问题：";
/// 回答 JSON 内容引导前缀。
const QUESTION_RECEIPT_CONTENT_PREFIX: &str = "回答内容：";

/// 剥离正文里的结构化问答回执块，只保留可见正文。
///
/// 找不到完整回执块时原样返回，绝不因为出现同名标题就截断正文。
#[must_use]
pub(crate) fn strip_question_receipt_block(content: &str) -> String {
    let lines = content.split('\n').collect::<Vec<_>>();
    for start in (0..lines.len()).rev() {
        if lines[start].trim() != QUESTION_RECEIPT_BLOCK_HEADER {
            continue;
        }
        if !is_receipt_block_body(&lines[start + 1..]) {
            continue;
        }
        return lines[..start].join("\n").trim_end().to_owned();
    }
    content.to_owned()
}

/// 标题之后的内容是否构成一个完整回执块。
fn is_receipt_block_body(lines: &[&str]) -> bool {
    let Some(call_at) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return false;
    };
    let call = lines[call_at].trim();
    let Some(call_id) = call.strip_prefix(QUESTION_RECEIPT_CALL_PREFIX) else {
        return false;
    };
    if call_id.trim().is_empty() {
        return false;
    }
    // 作答内容是压缩过的单行 JSON，但也允许它自带换行：取引导行之后的全部文本。
    let rest = lines[call_at + 1..].join("\n");
    let rest = rest.trim();
    let Some(answers) = rest.strip_prefix(QUESTION_RECEIPT_CONTENT_PREFIX) else {
        return false;
    };
    !answers.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_full_receipt_block_and_keeps_visible_summary() {
        let content = "1. 是否继续部署？ 回答：确认部署\n\n[问答回执]\n回答待决问题：call-deploy-99\n回答内容：[{\"id\":\"q1\",\"selected\":[\"确认部署\"]}]";

        assert_eq!(
            strip_question_receipt_block(content),
            "1. 是否继续部署？ 回答：确认部署"
        );
    }

    #[test]
    fn strip_drops_header_only_when_summary_is_empty() {
        let content = "[问答回执]\n回答待决问题：call-1\n回答内容：[]";

        assert_eq!(strip_question_receipt_block(content), "");
    }

    #[test]
    fn strip_keeps_body_that_merely_mentions_the_header() {
        // 用户复述格式：标题出现，但后面不构成完整回执块，必须原样保留。
        let content = "我在文档里写了 [问答回执] 这个标题。\n它后面还有正常正文。";
        assert_eq!(strip_question_receipt_block(content), content);

        let partial = "[问答回执]\n这里是我对格式的说明，没有引导前缀。";
        assert_eq!(strip_question_receipt_block(partial), partial);

        let missing_content = "[问答回执]\n回答待决问题：call-1\n下面是别的内容";
        assert_eq!(
            strip_question_receipt_block(missing_content),
            missing_content
        );
    }

    #[test]
    fn strip_uses_the_last_matching_block() {
        let content = "[问答回执]\n用户复述的标题，不是回执\n\n正文续写\n[问答回执]\n回答待决问题：call-2\n回答内容：[]";

        assert_eq!(
            strip_question_receipt_block(content),
            "[问答回执]\n用户复述的标题，不是回执\n\n正文续写"
        );
    }
}
