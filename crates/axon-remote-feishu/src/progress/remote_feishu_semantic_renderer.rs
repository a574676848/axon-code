//! JARVIS 语义输出的 Feishu 渲染器。

use axon_domain::{AxonResult, SemanticOutput};

use crate::remote_feishu_outbound::{
    CONTENT_TYPE_MARKDOWN, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
    RemoteFeishuOutboundMessagePlan, RemoteFeishuOutboundMessageRuntime,
};
use crate::{RemoteNotificationAdapterKind, RemoteNotificationRenderArtifact, RemoteTransportKind};

/// Feishu 语义输出 renderer。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuSemanticRenderer;

impl RemoteFeishuSemanticRenderer {
    /// 将语义输出渲染为 Feishu OpenAPI 消息计划。
    pub fn render(output: &SemanticOutput) -> AxonResult<RemoteFeishuOutboundMessagePlan> {
        match output {
            SemanticOutput::Answer { text } => markdown(text),
            SemanticOutput::Clarification { question } => {
                markdown(&format!("**需要补充信息**\n\n{question}"))
            }
            SemanticOutput::Completion { text } => markdown(&format!("**已完成**\n\n{text}")),
            SemanticOutput::Failure {
                code,
                message,
                retryable,
            } => {
                let retry_hint = if *retryable {
                    "\n\n你可以明确请求重试。"
                } else {
                    ""
                };
                markdown(&format!(
                    "**执行失败**\n\n{message}\n\n错误码：`{code}`{retry_hint}"
                ))
            }
            SemanticOutput::Error { code, message } => {
                markdown(&format!("**运行时错误**\n\n{message}\n\n错误码：`{code}`"))
            }
            SemanticOutput::Progress { text } => Ok(interactive_card("执行进度", text)),
        }
    }
}

fn markdown(body: &str) -> AxonResult<RemoteFeishuOutboundMessagePlan> {
    RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&RemoteNotificationRenderArtifact {
        adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
        transport: RemoteTransportKind::Feishu,
        content_type: CONTENT_TYPE_MARKDOWN.to_owned(),
        payload: body.to_owned(),
        network_send_allowed: true,
    })
}

fn interactive_card(title: &str, body: &str) -> RemoteFeishuOutboundMessagePlan {
    let elements = vec![serde_json::json!({
        "tag": "markdown",
        "content": format!("**{title}**\\n\\n{body}"),
    })];
    let card = serde_json::json!({"schema":"2.0","body":{"elements":elements}});
    RemoteFeishuOutboundMessagePlan {
        msg_type: REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE.to_owned(),
        content_json: card.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use axon_domain::SemanticOutput;

    use super::RemoteFeishuSemanticRenderer;
    use crate::REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE;

    #[test]
    fn semantic_messages_use_markdown_without_internal_ids() {
        let output = SemanticOutput::completion("任务已完成").expect("completion");
        let plan = RemoteFeishuSemanticRenderer::render(&output).expect("render");

        assert_eq!(plan.msg_type, "post");
        assert!(plan.content_json.contains("任务已完成"));
        assert!(!plan.content_json.contains("session_id"));
        assert!(!plan.content_json.contains("thread_id"));
        assert!(!plan.content_json.contains("run_id"));
    }

    #[test]
    fn progress_renders_interactive_card_without_action_buttons() {
        let output = SemanticOutput::progress("正在执行").expect("progress");
        let plan = RemoteFeishuSemanticRenderer::render(&output).expect("render");

        assert_eq!(plan.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE);
        assert!(plan.content_json.contains("执行进度"));
        assert!(!plan.content_json.contains("axon_action"));
        assert!(!plan.content_json.contains("button"));
    }
}
