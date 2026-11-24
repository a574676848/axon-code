//! 引用消息在 provider 边界的瞬时预算投影。

use axon_domain::QuotedMessageContext;

use crate::{
    AgentModelMessage, AgentRuntimeNoteKind, ModelToolDefinition, PromptCacheProviderWireFormat,
    ProviderStreamProtocol, ProviderStreamingTransportInput,
    estimated_history_tokens_for_provider_with_capabilities,
};

const QUOTED_MESSAGE_TRUNCATION_MARKER: &str = "\n...[引用正文已截断]...\n";

/// 运行中用户输入及其可选引用 sidecar。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnSteerInput {
    /// canonical 用户消息或内部 runtime note。
    pub message: AgentModelMessage,
    /// 被回复消息的可见文本上下文。
    pub quoted_message: Option<QuotedMessageContext>,
}

/// 引用正文的 provider 投影模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotedMessageProjectionMode {
    /// 完整正文。
    Full,
    /// Unicode 安全的首尾正文。
    HeadTail,
    /// 仅保留消息 ID 指针。
    IdOnly,
}

/// 引用正文的单次 provider 投影结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotedMessageProjection {
    /// 注入 runtime note 的内部正文。
    pub content: String,
    /// 原始正文字符数。
    pub original_chars: usize,
    /// 实际注入的正文字符数。
    pub included_chars: usize,
    /// 正文是否被截断或省略。
    pub truncated: bool,
    /// 本次投影模式。
    pub mode: QuotedMessageProjectionMode,
}

impl QuotedMessageProjection {
    /// 转换为不参与 cache 和历史压缩的瞬时消息。
    #[must_use]
    pub fn into_runtime_note(self) -> AgentModelMessage {
        AgentModelMessage::runtime_note(AgentRuntimeNoteKind::QuotedMessageContext, self.content)
    }
}

/// 按当前 provider 的真实上下文预算投影引用正文。
#[must_use]
pub fn project_quoted_message(
    quoted: &QuotedMessageContext,
    messages: &[AgentModelMessage],
    protocol: ProviderStreamProtocol,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
    context_window: u64,
) -> QuotedMessageProjection {
    let original_chars = quoted.visible_text.chars().count();
    let full = projection(quoted, original_chars, QuotedMessageProjectionMode::Full);
    if projection_fits(
        &full,
        messages,
        protocol,
        model_tools,
        supports_vision,
        context_window,
    ) {
        return full;
    }
    let included_chars = largest_fitting_char_count(
        quoted,
        messages,
        protocol,
        model_tools,
        supports_vision,
        context_window,
    );
    if included_chars == 0 {
        return projection(quoted, 0, QuotedMessageProjectionMode::IdOnly);
    }
    projection(
        quoted,
        included_chars,
        QuotedMessageProjectionMode::HeadTail,
    )
}

fn largest_fitting_char_count(
    quoted: &QuotedMessageContext,
    messages: &[AgentModelMessage],
    protocol: ProviderStreamProtocol,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
    context_window: u64,
) -> usize {
    let mut lower = 0usize;
    let mut upper = quoted.visible_text.chars().count();
    while lower < upper {
        let candidate_chars = lower + (upper - lower).div_ceil(2);
        let candidate = projection(
            quoted,
            candidate_chars,
            QuotedMessageProjectionMode::HeadTail,
        );
        if projection_fits(
            &candidate,
            messages,
            protocol,
            model_tools,
            supports_vision,
            context_window,
        ) {
            lower = candidate_chars;
        } else {
            upper = candidate_chars - 1;
        }
    }
    lower
}

fn projection_fits(
    projection: &QuotedMessageProjection,
    messages: &[AgentModelMessage],
    protocol: ProviderStreamProtocol,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
    context_window: u64,
) -> bool {
    let mut projected_messages = messages.to_vec();
    insert_before_latest_user(
        &mut projected_messages,
        projection.clone().into_runtime_note(),
    );
    let tokens = estimated_history_tokens_for_provider_with_capabilities(
        &projected_messages,
        provider_format(protocol),
        model_tools,
        supports_vision,
    );
    tokens <= context_window
}

/// 将引用 note 放在最近一条真实用户消息之前，明确其归属。
pub fn insert_before_latest_user(messages: &mut Vec<AgentModelMessage>, note: AgentModelMessage) {
    let index = messages
        .iter()
        .rposition(AgentModelMessage::is_real_user_turn)
        .unwrap_or(messages.len());
    messages.insert(index, note);
}

/// 将一批 typed steer input 投影为当前 provider 可见的瞬时消息。
#[must_use]
pub fn project_steer_inputs(
    inputs: Vec<AgentTurnSteerInput>,
    messages: &[AgentModelMessage],
    provider: &ProviderStreamingTransportInput,
) -> Vec<AgentModelMessage> {
    let mut budget_messages = messages.to_vec();
    let mut projected = Vec::with_capacity(inputs.len().saturating_mul(2));
    for input in inputs {
        if let Some(quoted) = input.quoted_message.as_ref() {
            let mut candidate_messages = budget_messages.clone();
            candidate_messages.push(input.message.clone());
            let note = project_quoted_message(
                quoted,
                &candidate_messages,
                provider.protocol,
                &provider.model_tools,
                provider.supports_vision,
                provider.context_window,
            )
            .into_runtime_note();
            budget_messages.push(note.clone());
            projected.push(note);
        }
        budget_messages.push(input.message.clone());
        projected.push(input.message);
    }
    projected
}

fn projection(
    quoted: &QuotedMessageContext,
    included_chars: usize,
    mode: QuotedMessageProjectionMode,
) -> QuotedMessageProjection {
    let original_chars = quoted.visible_text.chars().count();
    let message_id = escape_attribute(&quoted.message_id);
    let content = match mode {
        QuotedMessageProjectionMode::Full => format!(
            "<quoted-message message-id=\"{message_id}\">\n{}\n</quoted-message>",
            quoted.visible_text
        ),
        QuotedMessageProjectionMode::HeadTail => head_tail_content(
            &message_id,
            &quoted.visible_text,
            original_chars,
            included_chars,
        ),
        QuotedMessageProjectionMode::IdOnly => format!(
            "<quoted-message message-id=\"{message_id}\" original-chars=\"{original_chars}\" truncated=\"true\">正文因上下文预算未注入</quoted-message>"
        ),
    };
    QuotedMessageProjection {
        content,
        original_chars,
        included_chars,
        truncated: mode != QuotedMessageProjectionMode::Full,
        mode,
    }
}

fn head_tail_content(
    message_id: &str,
    text: &str,
    original_chars: usize,
    included_chars: usize,
) -> String {
    let head_chars = included_chars.div_ceil(2);
    let tail_chars = included_chars.saturating_sub(head_chars);
    let head: String = text.chars().take(head_chars).collect();
    let tail = text
        .char_indices()
        .nth(original_chars.saturating_sub(tail_chars))
        .map_or("", |(index, _)| &text[index..]);
    format!(
        "<quoted-message message-id=\"{message_id}\" original-chars=\"{original_chars}\" truncated=\"true\">\n{head}{QUOTED_MESSAGE_TRUNCATION_MARKER}{tail}\n</quoted-message>"
    )
}

const fn provider_format(protocol: ProviderStreamProtocol) -> PromptCacheProviderWireFormat {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => PromptCacheProviderWireFormat::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible => {
            PromptCacheProviderWireFormat::OpenAiCompatibleChat
        }
        ProviderStreamProtocol::AnthropicMessages => {
            PromptCacheProviderWireFormat::AnthropicMessages
        }
    }
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quoted(text: &str) -> QuotedMessageContext {
        QuotedMessageContext::new("om_parent", text).expect("引用上下文有效")
    }

    fn messages() -> Vec<AgentModelMessage> {
        vec![AgentModelMessage::text("user", "请基于引用内容处理")]
    }

    #[test]
    fn sufficient_budget_keeps_full_text_and_message_pointer() {
        let projection = project_quoted_message(
            &quoted("完整父消息"),
            &messages(),
            ProviderStreamProtocol::OpenAiResponses,
            &[],
            false,
            1_024,
        );

        assert_eq!(projection.mode, QuotedMessageProjectionMode::Full);
        assert!(projection.content.contains("om_parent"));
        assert!(projection.content.contains("完整父消息"));
    }

    #[test]
    fn constrained_budget_truncates_unicode_at_character_boundaries() {
        let source = "甲乙丙丁戊己庚辛😀壬癸".repeat(40);
        let projection = project_quoted_message(
            &quoted(&source),
            &messages(),
            ProviderStreamProtocol::OpenAiCompatible,
            &[],
            false,
            180,
        );

        assert_eq!(projection.mode, QuotedMessageProjectionMode::HeadTail);
        assert!(projection.truncated);
        assert!(
            projection
                .content
                .contains(QUOTED_MESSAGE_TRUNCATION_MARKER)
        );
        assert!(projection.included_chars < projection.original_chars);
    }

    #[test]
    fn zero_body_budget_still_keeps_message_pointer() {
        let projection = project_quoted_message(
            &quoted("父消息正文"),
            &messages(),
            ProviderStreamProtocol::AnthropicMessages,
            &[],
            false,
            64,
        );

        assert_eq!(projection.mode, QuotedMessageProjectionMode::IdOnly);
        assert_eq!(projection.included_chars, 0);
        assert!(projection.content.contains("om_parent"));
    }
}
