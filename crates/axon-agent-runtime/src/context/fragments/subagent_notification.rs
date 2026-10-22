//! Subagent notification fragment。

use axon_context_fragments::PromptFragment;

/// Subagent notification fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentNotificationFragment {
    /// Subagent ID。
    pub subagent_id: String,
    /// 通知内容。
    pub message: String,
}

impl SubagentNotificationFragment {
    /// 创建新的 subagent notification fragment。
    #[must_use]
    pub fn new(subagent_id: String, message: String) -> Self {
        Self {
            subagent_id,
            message,
        }
    }
}

impl PromptFragment for SubagentNotificationFragment {
    fn metadata(&self) -> axon_context_fragments::PromptFragmentMetadata {
        axon_context_fragments::PromptFragmentMetadata::dynamic_user()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<subagent-notification>", "</subagent-notification>")
    }

    fn body(&self) -> String {
        format!("\nSubagent {}: {}\n", self.subagent_id, self.message)
    }
}
