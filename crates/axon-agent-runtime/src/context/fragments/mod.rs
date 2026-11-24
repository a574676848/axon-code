//! Context fragments 模块。
//!
//! 实现各类 context fragment，每个 fragment 负责特定领域的上下文注入。

mod agent_context_snapshot;
mod agent_pane_context;
mod agent_prompt_control;
mod document_symbol_context;
mod hook_additional_context;
mod subagent_notification;
mod turn_aborted;
mod user_shell_command;

pub use agent_context_snapshot::AgentContextSnapshotFragment;
pub use agent_pane_context::AgentPaneContextFragment;
pub use agent_prompt_control::{
    AgentContextWindowGuidanceFragment, AgentDelegationPolicyFragment, AgentPromptContextFragment,
    AgentRuntimeConstraintsFragment, AgentSystemPromptFragment, AgentToolPolicyFragment,
    AgentToolUsageGuidanceFragment, EXTERNAL_CONTENT_TRUST_BOUNDARY,
};
pub use document_symbol_context::DocumentSymbolContextFragment;
pub use hook_additional_context::HookAdditionalContextFragment;
pub use subagent_notification::SubagentNotificationFragment;
pub use turn_aborted::TurnAbortedFragment;
pub use user_shell_command::UserShellCommandFragment;
