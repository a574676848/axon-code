//! 换窗注入的「最近动作」摘要：从本轮工具回执生成。
//!
//! 便签由模型自己写，常常只剩一句结论；最近几步做了什么必须由运行时兜底，
//! 否则新窗口会忘记刚执行的命令与改过的文件。
//!
//! 两条判据决定谁占这几行：
//!
//! - **动作才占位**：便签类回执的正文已经落在 canonical checkpoint 的便签事实区，
//!   换窗请求与预算查询本身不是「做了什么」，它们占位只会把真正的工作挤出窗口；
//! - **同一动作合并**：反复读同一个文件、反复检索同一个词只留一行并标次数，读者
//!   一眼看出「这件事做了几遍」，而不是被重复行骗成清单很长。

use axon_agent_runtime::bare_tool_name;

use super::RunToolReceipt;

/// 最多保留的动作条数；去重之后仍然按条数封顶。
const MAX_LINES: usize = 4;
/// 单行摘要的字符上限。
const MAX_LINE_CHARS: usize = 160;
/// 摘要总量上限，计入新窗口 token 预算。
const MAX_TOTAL_BYTES: usize = 1_200;
/// 去重时参与比较的摘要前缀长度：够分清参数，又不至于把长输出算成不同动作。
const DEDUPE_PREFIX_CHARS: usize = 80;

pub(super) fn digest_from_receipts(receipts: &[RunToolReceipt]) -> Option<String> {
    // 从最近一条往前收：先收到的是最新的动作，收满再整体翻回时间顺序。
    let mut picked: Vec<&RunToolReceipt> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for receipt in receipts.iter().rev() {
        if is_meta_action(&receipt.tool_name) {
            continue;
        }
        let key = dedupe_key(receipt);
        if let Some(index) = picked.iter().position(|picked| dedupe_key(picked) == key) {
            counts[index] = counts[index].saturating_add(1);
            continue;
        }
        picked.push(receipt);
        counts.push(1);
        if picked.len() >= MAX_LINES {
            break;
        }
    }
    if picked.is_empty() {
        return None;
    }
    picked.reverse();
    counts.reverse();

    let mut text = String::new();
    for (receipt, count) in picked.iter().zip(counts) {
        let status = if count > 1 {
            format!("{}，×{count}", receipt.status)
        } else {
            receipt.status.clone()
        };
        let line = format!(
            "- {}（{status}）：{}",
            receipt.tool_name,
            compact_text(&receipt.summary, MAX_LINE_CHARS)
        );
        if text.len().saturating_add(line.len()).saturating_add(1) > MAX_TOTAL_BYTES {
            break;
        }
        text.push_str(&line);
        text.push('\n');
    }
    Some(text.trim_end().to_owned())
}

/// 不占动作位的回执：便签正文归 checkpoint，换窗与预算查询不是工作。
fn is_meta_action(tool_name: &str) -> bool {
    let bare = bare_tool_name(tool_name);
    bare.starts_with("notes_") || matches!(bare, "new_context" | "get_context_remaining")
}

/// 去重键：工具名、状态与摘要前缀。同一动作换了结果就不再合并，失败与成功不混算。
fn dedupe_key(receipt: &RunToolReceipt) -> String {
    let head = receipt
        .summary
        .chars()
        .take(DEDUPE_PREFIX_CHARS)
        .collect::<String>();
    format!("{}|{}|{head}", receipt.tool_name, receipt.status)
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
    fn keeps_only_the_last_four_actions() {
        let receipts = vec![
            receipt("builtin.read_file", "completed", "读 Cargo.toml"),
            receipt("builtin.search", "completed", "搜 checkpoint"),
            receipt("builtin.glob", "completed", "匹配 crates/*.rs"),
            receipt("builtin.edit", "completed", "改 crates/a.rs"),
            receipt("builtin.shell_command", "completed", "cargo test -p a"),
        ];
        let digest = digest_from_receipts(&receipts).expect("应有摘要");
        assert!(digest.contains("builtin.edit"), "{digest}");
        assert!(digest.contains("shell_command"), "{digest}");
        assert!(digest.contains("builtin.glob"), "{digest}");
        assert!(digest.contains("builtin.search"), "{digest}");
        assert!(
            !digest.contains("read_file"),
            "只保留最近四条动作：{digest}"
        );
    }

    /// 便签正文归 checkpoint，换窗与预算查询不是动作：三者都不许占位。
    #[test]
    fn notes_and_meta_actions_never_occupy_a_slot() {
        let receipts = vec![
            receipt(
                "builtin.shell_command",
                "completed",
                "git commit -m 修复工单",
            ),
            receipt("builtin.get_context_remaining", "completed", "ctx: 96%"),
            receipt(
                "builtin.notes_write_file",
                "completed",
                "便签已整文件写入：checkpoint.md（3793 字节）。\n\n# Goal Checkpoint\n\n## 目标\n修复工单",
            ),
            receipt(
                "builtin.notes_read_file",
                "completed",
                "# Goal Checkpoint\n## 目标\n修复工单",
            ),
            receipt("builtin.new_context", "completed", "请求换窗"),
        ];
        let digest = digest_from_receipts(&receipts).expect("应有摘要");
        assert!(digest.contains("git commit"), "动作要留下：{digest}");
        assert!(!digest.contains("Goal Checkpoint"), "{digest}");
        assert!(!digest.contains("checkpoint.md"), "{digest}");
        assert!(!digest.contains("ctx: 96%"), "{digest}");
        assert!(!digest.contains("new_context"), "{digest}");
        assert_eq!(digest.lines().count(), 1, "只剩那一条真动作：{digest}");
    }

    #[test]
    fn repeated_actions_collapse_to_a_count() {
        let receipts = vec![
            receipt("builtin.read_file", "completed", "crates/a.rs"),
            receipt("builtin.read_file", "completed", "crates/a.rs"),
            receipt("builtin.read_file", "completed", "crates/a.rs"),
            receipt("builtin.shell_command", "completed", "cargo test"),
        ];
        let digest = digest_from_receipts(&receipts).expect("应有摘要");
        assert_eq!(digest.lines().count(), 2, "三次读取只占一行：{digest}");
        assert!(digest.contains("×3"), "{digest}");
        assert!(digest.contains("cargo test"), "{digest}");
    }

    #[test]
    fn same_tool_with_a_different_result_is_not_merged() {
        let receipts = vec![
            receipt("builtin.shell_command", "failed", "cargo test 失败"),
            receipt("builtin.shell_command", "completed", "cargo test 通过"),
        ];
        let digest = digest_from_receipts(&receipts).expect("应有摘要");
        assert_eq!(digest.lines().count(), 2, "{digest}");
        assert!(!digest.contains('×'), "{digest}");
    }

    #[test]
    fn only_meta_actions_produce_no_digest() {
        let receipts = vec![
            receipt(
                "builtin.notes_write_file",
                "completed",
                "便签已整文件写入：checkpoint.md（12 字节）。\n\n正文",
            ),
            receipt(
                "builtin.notes_append_to_file",
                "completed",
                "便签已追加：checkpoint.md（新增 4 字节，当前 16 字节）。\n\n追加",
            ),
            receipt("builtin.new_context", "completed", "请求换窗"),
        ];
        assert_eq!(digest_from_receipts(&receipts), None);
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
