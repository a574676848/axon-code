//! 会话稳定提示上下文合同，由 payload 与 prompt-cache 规划共同使用。

use axon_domain::{AxonError, AxonResult};

use crate::{AgentModelMessage, AgentModelMessageSource, RetentionClass};

const SYSTEM_ROLE: &str = "system";
const DEVELOPER_ROLE: &str = "developer";

/// AGENTS.md fragment start marker.
pub const WORKSPACE_INSTRUCTIONS_START_MARKER: &str = "# AGENTS.md instructions";
/// AGENTS.md fragment end marker.
pub const WORKSPACE_INSTRUCTIONS_END_MARKER: &str = "</INSTRUCTIONS>";
/// Personal Memory fragment start marker.
pub const SESSION_STABLE_PERSONAL_MEMORY_START_MARKER: &str = "<personal-memory-context>";
/// Personal Memory fragment end marker.
pub const SESSION_STABLE_PERSONAL_MEMORY_END_MARKER: &str = "</personal-memory-context>";
/// Available Skill Catalog fragment start marker.
pub const AVAILABLE_SKILLS_START_MARKER: &str = "<available-skills-context>";
/// Available Skill Catalog fragment end marker.
pub const AVAILABLE_SKILLS_END_MARKER: &str = "</available-skills-context>";
/// Active assistant profile reference start marker.
pub const SESSION_STABLE_SOUL_START_MARKER: &str = "<assistant-profile-context>";
/// Active assistant profile reference end marker.
pub const SESSION_STABLE_SOUL_END_MARKER: &str = "</assistant-profile-context>";
/// User Agents fragment start marker.
pub const SESSION_STABLE_USER_AGENTS_START_MARKER: &str = "<user-agents-context>";
/// User Agents fragment end marker.
pub const SESSION_STABLE_USER_AGENTS_END_MARKER: &str = "</user-agents-context>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionStableContextKind {
    UserAgents,
    PersonalMemory,
    Soul,
}

pub(crate) fn session_stable_context_kind(
    message: &AgentModelMessage,
) -> Option<SessionStableContextKind> {
    if !is_context_state_message(message) {
        return None;
    }
    let kind = context_kind_from_markers(&message.content)?;
    (message.role == context_kind_role(kind)).then_some(kind)
}

/// 判断消息是否允许跨 Workspace 进入稳定前缀。
///
/// 该白名单是压缩重建的边界：不能仅依赖 `cache_excluded=false`，否则遗漏标记的
/// Workspace 动态片段会在窗口轮转后永久污染稳定前缀。
pub(crate) fn is_cross_workspace_stable_message(message: &AgentModelMessage) -> bool {
    if !is_context_state_message(message) {
        return false;
    }
    if matches!(
        session_stable_context_kind(message),
        Some(SessionStableContextKind::UserAgents)
            | Some(SessionStableContextKind::PersonalMemory)
            | Some(SessionStableContextKind::Soul)
    ) {
        return true;
    }
    if !matches!(message.role.as_str(), SYSTEM_ROLE | DEVELOPER_ROLE) {
        return false;
    }
    !contains_dynamic_workspace_marker(&message.content)
        && !contains_workspace_runtime_hint(&message.content)
}

fn is_context_state_message(message: &AgentModelMessage) -> bool {
    !message.cache_excluded
        && message.source == AgentModelMessageSource::Context
        && message.retention_class == RetentionClass::P1State
}

pub(crate) fn contains_dynamic_workspace_marker(content: &str) -> bool {
    if content.contains(crate::agent_prompt_engineering::AXON_RUNTIME_CONSTRAINTS_HEADER) {
        return true;
    }
    const DYNAMIC_MARKERS: [&str; 19] = [
        WORKSPACE_INSTRUCTIONS_START_MARKER,
        AVAILABLE_SKILLS_START_MARKER,
        "<assistant-settings-context>",
        "<workspace-context",
        "<workspace-transition-context>",
        "<available-mcp-context",
        "<agent-pane-context>",
        "<agent-context-snapshot>",
        "<workspace-memory-context>",
        "<workspace-facts>",
        "<workspace-ledger>",
        "<workspace-goal>",
        "<selected-skill-context>",
        "<document-symbol-context>",
        "<capability-routing>",
        "<active-goal",
        "<prompt-context>",
        "<hook-additional-context>",
        "<persona-state-context>",
    ];
    DYNAMIC_MARKERS
        .iter()
        .any(|marker| content.contains(marker))
}

fn contains_workspace_runtime_hint(content: &str) -> bool {
    const RUNTIME_HINTS: [&str; 4] = ["cwd=", "CWD:", "git branch", "pane="];
    RUNTIME_HINTS.iter().any(|hint| content.contains(hint))
}

fn context_kind_from_markers(content: &str) -> Option<SessionStableContextKind> {
    if has_markers(
        content,
        SESSION_STABLE_USER_AGENTS_START_MARKER,
        SESSION_STABLE_USER_AGENTS_END_MARKER,
    ) {
        return Some(SessionStableContextKind::UserAgents);
    }
    if has_markers(
        content,
        SESSION_STABLE_PERSONAL_MEMORY_START_MARKER,
        SESSION_STABLE_PERSONAL_MEMORY_END_MARKER,
    ) {
        return Some(SessionStableContextKind::PersonalMemory);
    }
    if has_markers(
        content,
        SESSION_STABLE_SOUL_START_MARKER,
        SESSION_STABLE_SOUL_END_MARKER,
    ) {
        return Some(SessionStableContextKind::Soul);
    }
    None
}

const fn context_kind_role(kind: SessionStableContextKind) -> &'static str {
    match kind {
        SessionStableContextKind::Soul => SYSTEM_ROLE,
        SessionStableContextKind::UserAgents | SessionStableContextKind::PersonalMemory => {
            DEVELOPER_ROLE
        }
    }
}

pub(crate) fn validate_session_stable_context_messages(
    messages: &[AgentModelMessage],
) -> AxonResult<()> {
    if messages
        .iter()
        .all(|message| session_stable_context_kind(message).is_some())
    {
        return Ok(());
    }
    Err(AxonError::runtime_failure(
        "session-stable context 只允许 Soul、用户级 AGENTS 与全局 Personal Memory；所有消息必须可缓存、role 正确且 marker 完整，Workspace、Skill、Pane/CWD、Config 与 Context Snapshot 必须作为动态上下文",
    ))
}

fn has_markers(content: &str, start_marker: &str, end_marker: &str) -> bool {
    let content = content.trim();
    content.starts_with(start_marker) && content.ends_with(end_marker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_accepts_all_session_stable_context_messages() {
        let messages = vec![
            AgentModelMessage::context(
                "developer",
                "<user-agents-context>\n全局规则\n</user-agents-context>",
            ),
            AgentModelMessage::context(
                "developer",
                "<personal-memory-context>\n用户偏好中文\n</personal-memory-context>",
            ),
            AgentModelMessage::context(
                "system",
                "<assistant-profile-context>\nsoul-1\n</assistant-profile-context>",
            ),
        ];

        assert!(validate_session_stable_context_messages(&messages).is_ok());
    }

    #[test]
    fn validation_rejects_workspace_skill_pane_and_snapshot_messages() {
        let dynamic_context = [
            AgentModelMessage::text(
                "developer",
                "# AGENTS.md instructions\n<INSTRUCTIONS>规则</INSTRUCTIONS>",
            ),
            AgentModelMessage::text(
                "developer",
                "<available-skills-context>skill-a</available-skills-context>",
            ),
            AgentModelMessage::text(
                "developer",
                "<agent-pane-context>cwd=E:/workspace</agent-pane-context>",
            ),
            AgentModelMessage::text(
                "developer",
                "<agent-context-snapshot>cwd=E:/workspace</agent-context-snapshot>",
            ),
        ];

        for message in dynamic_context {
            assert!(session_stable_context_kind(&message).is_none());
            assert!(validate_session_stable_context_messages(&[message]).is_err());
        }
    }

    #[test]
    fn cross_workspace_stable_message_is_explicit_whitelist() {
        let stable = AgentModelMessage::context("system", "通用系统规则");
        assert!(is_cross_workspace_stable_message(&stable));
        for dynamic in [
            AgentModelMessage::text(
                "developer",
                "# AGENTS.md instructions\n<INSTRUCTIONS>workspace</INSTRUCTIONS>",
            ),
            AgentModelMessage::text(
                "developer",
                "<available-skills-context>workspace</available-skills-context>",
            ),
            AgentModelMessage::text(
                "system",
                "<available-mcp-context>workspace</available-mcp-context>",
            ),
            AgentModelMessage::text(
                "system",
                "<workspace-transition-context>old -> new</workspace-transition-context>",
            ),
            AgentModelMessage::text(
                "system",
                "<runtime-constraints>cwd=E:/workspace</runtime-constraints>",
            ),
        ] {
            assert!(!is_cross_workspace_stable_message(&dynamic));
        }
        assert!(!contains_dynamic_workspace_marker(
            "用户要求检查文本中的 cwd=E:/workspace"
        ));
    }
}
