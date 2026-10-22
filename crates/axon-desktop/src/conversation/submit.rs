//! 提交用户消息：走网关 HTTP 交互事件面。
//!
//! 交互标识由服务端派生（`external-interaction-<hash>`），客户端无法预知，
//! 因此提交成功后的第一件事是把回执里的标识交给订阅器。

use crate::conversation::wire::{EVENTS_PATH, EventSubmission, SubmissionReceipt};
use crate::gateway;
use crate::lane_source::DESKTOP_LANE_CHANNEL;

/// 提交正文的字符上限。
///
/// 网关请求体上限是 64 KiB，超限会被直接拒绝；这里在客户端先给出可读结论，
/// 避免用户只看到一个 413。
const MAX_CONTENT_CHARS: usize = 16_000;
/// 事件有效期：网关要求失效时间必须晚于当前时间，且与 daemon 时钟偏差不超过 5 分钟。
const EVENT_TTL_MS: u64 = 10 * 60 * 1_000;

/// 提交一条用户消息。
///
/// `workspace_id` 是用户选定的工作区归属：桌面端显式给出它，会话才会落在
/// 正确的项目下，而不是 daemon 自己的默认工作区。
/// `attachments` 是已经落地到磁盘的附件绝对路径，后端把它们投影为 Provider turn
/// 的输入素材；空数组表示纯文本消息。
pub fn submit_message(
    workspace_id: &str,
    destination: &str,
    content: &str,
    attachments: &[String],
) -> Result<SubmissionReceipt, String> {
    let content = content.trim();
    if content.is_empty() && attachments.is_empty() {
        return Err("消息内容不能为空".to_string());
    }
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err(format!(
            "消息超过 {MAX_CONTENT_CHARS} 个字符，请拆分后再发送"
        ));
    }
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return Err("必须先选定工作区才能发送消息".to_string());
    }
    let destination = destination.trim();
    if destination.is_empty() {
        return Err("会话目标缺失，请先新建会话".to_string());
    }

    let now_ms = crate::workspace::now_millis();
    let envelope = EventSubmission {
        source_channel: DESKTOP_LANE_CHANNEL.to_string(),
        event_id: format!(
            "{}-{}",
            DESKTOP_LANE_CHANNEL,
            crate::workspace::generate_uuid()
        ),
        workspace_id: workspace_id.to_string(),
        observed_at_ms: now_ms,
        expires_at_ms: now_ms.saturating_add(EVENT_TTL_MS),
        content: content.to_string(),
        reply_channel: DESKTOP_LANE_CHANNEL.to_string(),
        reply_destination: destination.to_string(),
        local_files: attachments.to_vec(),
    };
    let payload =
        serde_json::to_value(&envelope).map_err(|error| format!("构造提交载荷失败：{error}"))?;
    let response = gateway::post_json_at(EVENTS_PATH, &payload)?;
    serde_json::from_value::<SubmissionReceipt>(response)
        .map_err(|error| format!("解析提交回执失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_content_is_rejected_before_any_request() {
        assert!(submit_message("workspace-a", "workbench-1", "   ", &[]).is_err());
    }

    #[test]
    fn missing_workspace_or_destination_is_rejected_before_any_request() {
        assert!(submit_message("", "workbench-1", "你好", &[]).is_err());
        assert!(submit_message("workspace-a", "  ", "你好", &[]).is_err());
    }

    #[test]
    fn oversized_content_is_rejected_with_a_readable_reason() {
        let content = "好".repeat(MAX_CONTENT_CHARS + 1);
        let error =
            submit_message("workspace-a", "workbench-1", &content, &[]).expect_err("必须拒绝");
        assert!(error.contains("字符"), "错误必须指明是长度问题：{error}");
    }

    #[test]
    fn attachments_make_an_empty_caption_acceptable() {
        // 只发一张图不带文字是合法输入；空文本且无附件才应被拒绝。
        assert!(submit_message("", "workbench-1", "", &["a.png".to_string()]).is_err());
    }
}
