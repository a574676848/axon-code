//! User shell command fragment。

use axon_context_fragments::PromptFragment;

/// User shell command fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserShellCommandFragment {
    /// 命令文本。
    pub command: String,
}

impl UserShellCommandFragment {
    /// 创建新的 user shell command fragment。
    #[must_use]
    pub fn new(command: String) -> Self {
        Self { command }
    }
}

impl PromptFragment for UserShellCommandFragment {
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<user-shell-command>", "</user-shell-command>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.command)
    }
}
