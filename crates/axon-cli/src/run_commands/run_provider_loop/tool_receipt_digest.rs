//! 换窗注入的「最近动作」摘要：从本轮工具回执生成。
//!
//! 便签由模型自己写，常常只剩一句结论；最近几步做了什么必须由运行时兜底，
//! 否则新窗口会忘记刚执行的命令与改过的文件。只取已收口的回执，最多两条。

use super::RunToolReceipt;

/// 最多保留的动作条数。
const MAX_LINES: usize = 2;
/// 单行摘要的字符上限。
const MAX_LINE_CHARS: usize = 160;
/// 摘要总量上限，计入新窗口 token 预算。
const MAX_TOTAL_BYTES: usize = 1_200;

pub(super) fn digest_from_receipts(receipts: &[RunToolReceipt]) -> Option<String> {
    let mut lines = Vec::new();
    for receipt in receipts.iter().rev() {
        lines.push(format!(
            "- {}（{}）：{}",
            receipt.tool_name,
            receipt.status,
            compact_text(&receipt.summary, MAX_LINE_CHARS)
        ));
        if lines.len() >= MAX_LINES {
            break;
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    let mut text = String::new();
    for line in lines {
        if text.len().saturating_add(line.len()).saturating_add(1) > MAX_TOTAL_BYTES {
            break;
        }
        text.push_str(&line);
        text.push('\n');
    }
    Some(text.trim_end().to_owned())
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let flattened = value.replace('\n', " ");
    let trimmed = flattened.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let clipped: String = trimmed.chars().take(max_chars).collect();
    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(name: &str, status: &str, summary: &str) -> RunToolReceipt {
        RunToolReceipt {
            tool_call_id: 1,
            part_id: "part-1".to_owned(),
            provider_call_id: "call-1".to_owned(),
            tool_name: name.to_owned(),
            status: status.to_owned(),
            summary: summary.to_owned(),
            metadata: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: None,
        }
    }

    #[test]
    fn keeps_only_the_last_two_receipts() {
        let receipts = vec![
            receipt("builtin.read_file", "completed", "读 Cargo.toml"),
            receipt("builtin.notes_write_file", "completed", "写 checkpoint.md"),
            receipt("builtin.new_context", "completed", "请求换窗"),
        ];
        let digest = digest_from_receipts(&receipts).expect("应有摘要");
        assert!(digest.contains("notes_write_file"));
        assert!(digest.contains("new_context"));
        assert!(!digest.contains("read_file"), "只保留最近两条：{digest}");
    }

    #[test]
    fn truncates_long_summaries() {
        let receipts = vec![receipt("builtin.search", "completed", &"命".repeat(500))];
        let digest = digest_from_receipts(&receipts).expect("应有摘要");
        assert!(digest.contains('…'));
        assert!(digest.chars().count() < 400);
    }

    #[test]
    fn empty_receipts_have_no_digest() {
        assert_eq!(digest_from_receipts(&[]), None);
    }
}
