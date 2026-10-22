//! Turn aborted fragment。

use axon_context_fragments::PromptFragment;

/// Turn aborted fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnAbortedFragment {
    /// 中止原因。
    pub reason: String,
}

impl TurnAbortedFragment {
    /// 创建新的 turn aborted fragment。
    #[must_use]
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

impl PromptFragment for TurnAbortedFragment {
    fn metadata(&self) -> axon_context_fragments::PromptFragmentMetadata {
        axon_context_fragments::PromptFragmentMetadata::dynamic_user()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<turn-aborted>", "</turn-aborted>")
    }

    fn body(&self) -> String {
        format!("\nReason: {}\n", self.reason)
    }
}
