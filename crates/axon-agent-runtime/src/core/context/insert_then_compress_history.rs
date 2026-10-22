//! FreshWindow replacement history 的稳定上下文重装与 token 计量。

use super::WINDOW_ROLLOVER_CHECKPOINT_PREFIX;
#[cfg(test)]
use crate::agent_model_tool_history::normalize_provider_history;
use crate::token_estimator::estimated_message_tokens_with_capabilities;
use crate::{
    AgentModelMessage, AgentModelMessageSource, CanonicalCheckpoint, ModelToolDefinition,
    PromptCacheProviderWireFormat, agent_session_stable_context::is_cross_workspace_stable_message,
};

/// FreshWindow replacement history。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedHistory {
    /// replacement history 消息。
    pub messages: Vec<AgentModelMessage>,
    /// 新窗口的 provider-visible token 数。
    pub after_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FreshWindowContext {
    stable_messages: Vec<AgentModelMessage>,
    active_turn_tail: Vec<AgentModelMessage>,
}

impl FreshWindowContext {
    /// 从已规范化的 provider-visible 历史捕获稳定消息与活动 turn 尾部。
    ///
    /// 调用方必须传入与归档、token 计量同源的规范化历史，本函数不再重复规范化。
    pub(super) fn capture_from_normalized(provider_messages: &[AgentModelMessage]) -> Self {
        let stable_messages = provider_messages
            .iter()
            .filter(|message| is_cross_workspace_stable_message(message))
            .cloned()
            .collect();
        let active_turn_tail = active_turn_tail(provider_messages);
        Self {
            stable_messages,
            active_turn_tail,
        }
    }

    /// 仅供测试：从任意历史捕获上下文。
    #[cfg(test)]
    pub(super) fn capture(messages: &[AgentModelMessage], supports_vision: bool) -> Self {
        let normalized = normalize_provider_history(messages, supports_vision);
        Self::capture_from_normalized(normalized.messages.as_ref())
    }
}

fn active_turn_tail(messages: &[AgentModelMessage]) -> Vec<AgentModelMessage> {
    let Some(user_index) = messages
        .iter()
        .rposition(AgentModelMessage::is_real_user_turn)
    else {
        return Vec::new();
    };
    let mut start = user_index;
    while start > 0 {
        let candidate = &messages[start - 1];
        if is_cross_workspace_stable_message(candidate)
            || candidate.source != AgentModelMessageSource::Context
        {
            break;
        }
        start -= 1;
    }
    messages[start..=user_index]
        .iter()
        .filter(|message| !is_cross_workspace_stable_message(message))
        .cloned()
        .collect()
}

pub(super) fn build_fresh_window_history_from_normalized(
    provider_messages: &[AgentModelMessage],
    checkpoint: &CanonicalCheckpoint,
    captured_context: Option<&FreshWindowContext>,
    provider_format: PromptCacheProviderWireFormat,
    model_tools: &[ModelToolDefinition],
    supports_vision: bool,
) -> CompactedHistory {
    let mut messages = captured_context
        .map(|context| context.stable_messages.clone())
        .unwrap_or_else(|| {
            provider_messages
                .iter()
                .filter(|message| is_cross_workspace_stable_message(message))
                .cloned()
                .collect()
        });
    messages.push(AgentModelMessage::conversation_checkpoint(format!(
        "{WINDOW_ROLLOVER_CHECKPOINT_PREFIX}\n{}",
        checkpoint.as_str()
    )));
    if let Some(context) = captured_context {
        messages.extend(context.active_turn_tail.clone());
    }
    let after_tokens = messages.iter().fold(0_u64, |tokens, message| {
        tokens.saturating_add(estimated_message_tokens_with_capabilities(
            message,
            provider_format,
            model_tools,
            supports_vision,
        ))
    });
    CompactedHistory {
        messages,
        after_tokens,
    }
}
