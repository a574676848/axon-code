//! Agent pane context fragment。

use axon_context_fragments::{PromptFragment, PromptFragmentMetadata};

/// Agent pane projection fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPaneContextFragment {
    content: String,
}

impl AgentPaneContextFragment {
    /// 创建 Agent pane context fragment。
    #[must_use]
    pub fn new(content: String) -> Self {
        Self { content }
    }
}

impl PromptFragment for AgentPaneContextFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<agent-pane-context>", "</agent-pane-context>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content.trim())
    }
}
