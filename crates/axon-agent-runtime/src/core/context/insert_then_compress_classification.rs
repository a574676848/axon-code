//! 压缩历史使用的消息分类辅助函数。

use crate::{AgentModelMessage, AgentModelMessageKind, AgentModelMessageSource};

pub(crate) fn is_real_conversation_message(message: &AgentModelMessage) -> bool {
    message.source == AgentModelMessageSource::Conversation
        && message.retention_class != crate::RetentionClass::P3Derivable
        && !is_compaction_marker(message)
}

pub(crate) fn is_compaction_marker(message: &AgentModelMessage) -> bool {
    matches!(
        message.kind,
        AgentModelMessageKind::RuntimeNote(_) | AgentModelMessageKind::ConversationCheckpoint
    )
}

pub(crate) fn compressible_len(messages: &[AgentModelMessage]) -> usize {
    messages
        .iter()
        .filter(|message| is_real_conversation_message(message))
        .count()
}
