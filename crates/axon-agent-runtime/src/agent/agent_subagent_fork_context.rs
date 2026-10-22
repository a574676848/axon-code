//! 子代理 fork 上下文继承策略。
//!
//! 本模块对齐 Codex MultiAgentV2 的 fork 语义，但只依赖 Axon 已有的
//! [`AgentModelMessage`]。它在真实线程执行链路接入前，先把可验证的
//! transcript 裁剪与污染过滤规则固化到 runtime 层。

use axon_domain::{AxonError, AxonResult};

use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, SESSION_STABLE_SOUL_END_MARKER,
    SESSION_STABLE_SOUL_START_MARKER, SUBAGENT_TOOL_FORK_TURNS_FIELD,
    agent_session_stable_context::contains_dynamic_workspace_marker,
};

/// fork 上下文已规划。
pub const SUBAGENT_FORK_CONTEXT_REASON_PLANNED: &str = "subagent_fork_context:planned";
/// 未继承父上下文。
pub const SUBAGENT_FORK_CONTEXT_REASON_NONE: &str = "subagent_fork_context:none";
/// 继承过滤后的全量父上下文。
pub const SUBAGENT_FORK_CONTEXT_REASON_FULL_HISTORY: &str = "subagent_fork_context:full_history";
/// 继承过滤后的最近 N 个 turn。
pub const SUBAGENT_FORK_CONTEXT_REASON_LAST_N_TURNS: &str = "subagent_fork_context:last_n_turns";
/// 已过滤工具调用与工具结果。
pub const SUBAGENT_FORK_CONTEXT_REASON_TOOL_CHATTER_FILTERED: &str =
    "subagent_fork_context:tool_chatter_filtered";
/// 已过滤父代理 usage hint。
pub const SUBAGENT_FORK_CONTEXT_REASON_USAGE_HINT_FILTERED: &str =
    "subagent_fork_context:usage_hint_filtered";
/// 已过滤主代理 Soul。
pub const SUBAGENT_FORK_CONTEXT_REASON_SOUL_FILTERED: &str = "subagent_fork_context:soul_filtered";
/// 已注入子代理 usage hint。
pub const SUBAGENT_FORK_CONTEXT_REASON_CHILD_USAGE_HINT_ADDED: &str =
    "subagent_fork_context:child_usage_hint_added";

/// 子代理 fork 继承模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentForkTurns {
    /// 不继承父 transcript。
    None,
    /// 继承过滤后的全量 transcript。
    All,
    /// 继承过滤后的最近 N 个 user turn。
    Last(usize),
}

impl SubagentForkTurns {
    /// 解析 `spawn_agent.fork_turns` 字段。
    pub fn parse(value: Option<&str>) -> AxonResult<Self> {
        let value = value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("none");
        if value.eq_ignore_ascii_case("none") {
            return Ok(Self::None);
        }
        if value.eq_ignore_ascii_case("all") {
            return Ok(Self::All);
        }
        let turns = value.parse::<usize>().map_err(|_| invalid_fork_turns())?;
        if turns == 0 {
            return Err(invalid_fork_turns());
        }
        Ok(Self::Last(turns))
    }
}

/// 子代理 fork 上下文输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentForkContextInput {
    /// 父代理当前模型消息快照。
    pub parent_messages: Vec<AgentModelMessage>,
    /// fork turn 继承模式。
    pub fork_turns: SubagentForkTurns,
    /// 需要从父上下文中过滤的 usage hint 文本。
    pub parent_usage_hint_texts: Vec<String>,
    /// 可选子代理 usage hint。
    pub child_usage_hint_text: Option<String>,
    /// 是否启用 usage hint 注入。
    pub usage_hint_enabled: bool,
}

/// 子代理 fork 上下文输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentForkContextReport {
    /// 子代理可继承的消息。
    pub messages: Vec<AgentModelMessage>,
    /// 原始消息数量。
    pub original_count: usize,
    /// 保留消息数量。
    pub retained_count: usize,
    /// 丢弃消息数量。
    pub dropped_count: usize,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 子代理 fork 上下文 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct SubagentForkContextRuntime;

impl SubagentForkContextRuntime {
    /// 规划子代理继承上下文。
    pub fn plan(&self, input: SubagentForkContextInput) -> AxonResult<SubagentForkContextReport> {
        let original_count = input.parent_messages.len();
        let mut reasons = vec![SUBAGENT_FORK_CONTEXT_REASON_PLANNED.to_owned()];
        let mut messages = match input.fork_turns {
            SubagentForkTurns::None => {
                reasons.push(SUBAGENT_FORK_CONTEXT_REASON_NONE.to_owned());
                Vec::new()
            }
            SubagentForkTurns::All => {
                reasons.push(SUBAGENT_FORK_CONTEXT_REASON_FULL_HISTORY.to_owned());
                filter_stable_messages(
                    input.parent_messages.clone(),
                    &input.parent_usage_hint_texts,
                )
            }
            SubagentForkTurns::Last(turns) => {
                reasons.push(SUBAGENT_FORK_CONTEXT_REASON_LAST_N_TURNS.to_owned());
                let filtered = filter_stable_messages(
                    input.parent_messages.clone(),
                    &input.parent_usage_hint_texts,
                );
                retain_last_user_turns_with_prelude(filtered, turns)
            }
        };
        append_filter_reasons(original_count, &messages, &input, &mut reasons);
        maybe_append_child_usage_hint(&mut messages, &input, &mut reasons);
        let retained_count = messages.len();
        Ok(SubagentForkContextReport {
            messages,
            original_count,
            retained_count,
            dropped_count: original_count.saturating_sub(retained_count),
            reasons,
        })
    }
}

fn filter_stable_messages(
    messages: Vec<AgentModelMessage>,
    usage_hint_texts: &[String],
) -> Vec<AgentModelMessage> {
    messages
        .into_iter()
        .filter(|message| keep_stable_message(message, usage_hint_texts))
        .collect()
}

fn keep_stable_message(message: &AgentModelMessage, usage_hint_texts: &[String]) -> bool {
    if message.source == crate::AgentModelMessageSource::Context
        || message.retention_class == crate::RetentionClass::P3Derivable
        || contains_dynamic_workspace_marker(&message.content)
        || is_soul_context(message)
        || is_parent_usage_hint(message, usage_hint_texts)
    {
        return false;
    }
    match message.role.as_str() {
        "system" | "developer" | "user" => true,
        "assistant" => is_final_assistant_text(message),
        _ => false,
    }
}

fn is_soul_context(message: &AgentModelMessage) -> bool {
    let content = message.content.trim();
    message.role == "system"
        && content.starts_with(SESSION_STABLE_SOUL_START_MARKER)
        && content.ends_with(SESSION_STABLE_SOUL_END_MARKER)
}

fn is_final_assistant_text(message: &AgentModelMessage) -> bool {
    message.provider_metadata.is_none() && !message.content.trim().is_empty()
}

fn is_parent_usage_hint(message: &AgentModelMessage, usage_hint_texts: &[String]) -> bool {
    message.role == "developer"
        && usage_hint_texts
            .iter()
            .any(|usage_hint| usage_hint == &message.content)
}

fn retain_last_user_turns_with_prelude(
    messages: Vec<AgentModelMessage>,
    turns: usize,
) -> Vec<AgentModelMessage> {
    let snapshot = AgentModelHistorySnapshot::from(messages);
    let first_tail_index = snapshot.first_index_for_last_user_turns(turns);
    snapshot
        .into_messages()
        .into_iter()
        .enumerate()
        .filter_map(|(index, message)| {
            (index >= first_tail_index || is_prelude_message(&message)).then_some(message)
        })
        .collect()
}

fn is_prelude_message(message: &AgentModelMessage) -> bool {
    matches!(message.role.as_str(), "system" | "developer")
}

fn append_filter_reasons(
    original_count: usize,
    messages: &[AgentModelMessage],
    input: &SubagentForkContextInput,
    reasons: &mut Vec<String>,
) {
    if original_count > messages.len() {
        reasons.push(SUBAGENT_FORK_CONTEXT_REASON_TOOL_CHATTER_FILTERED.to_owned());
    }
    if input
        .parent_messages
        .iter()
        .any(|message| is_parent_usage_hint(message, &input.parent_usage_hint_texts))
    {
        reasons.push(SUBAGENT_FORK_CONTEXT_REASON_USAGE_HINT_FILTERED.to_owned());
    }
    if input.parent_messages.iter().any(is_soul_context) {
        reasons.push(SUBAGENT_FORK_CONTEXT_REASON_SOUL_FILTERED.to_owned());
    }
}

fn maybe_append_child_usage_hint(
    messages: &mut Vec<AgentModelMessage>,
    input: &SubagentForkContextInput,
    reasons: &mut Vec<String>,
) {
    if !input.usage_hint_enabled || matches!(input.fork_turns, SubagentForkTurns::None) {
        return;
    }
    let Some(hint) = input
        .child_usage_hint_text
        .as_deref()
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
    else {
        return;
    };
    if messages
        .iter()
        .any(|message| message.role == "developer" && message.content == hint)
    {
        return;
    }
    messages.push(AgentModelMessage::cache_excluded("developer", hint));
    reasons.push(SUBAGENT_FORK_CONTEXT_REASON_CHILD_USAGE_HINT_ADDED.to_owned());
}

fn invalid_fork_turns() -> AxonError {
    AxonError::runtime_failure(SUBAGENT_TOOL_FORK_TURNS_FIELD)
}

#[cfg(test)]
mod tests {
    use crate::{AgentModelMessageProviderMetadata, AgentModelToolCallMetadata};

    use super::*;

    const ROOT_HINT: &str = "root multi-agent hint";
    const CHILD_HINT: &str = "child sub-agent hint";

    #[test]
    fn fork_turns_parse_matches_spawn_agent_contract() {
        assert_eq!(
            SubagentForkTurns::parse(None).unwrap(),
            SubagentForkTurns::None
        );
        assert_eq!(
            SubagentForkTurns::parse(Some("none")).unwrap(),
            SubagentForkTurns::None
        );
        assert_eq!(
            SubagentForkTurns::parse(Some("2")).unwrap(),
            SubagentForkTurns::Last(2)
        );
        assert!(SubagentForkTurns::parse(Some("0")).is_err());
        assert!(SubagentForkTurns::parse(Some("bad")).is_err());
    }

    #[test]
    fn full_history_filters_tool_chatter_and_replaces_usage_hint() {
        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::All, sample_parent_messages()))
            .unwrap();

        assert_eq!(
            roles_and_content(&report.messages),
            [
                ("system", "base"),
                ("user", "first"),
                ("assistant", "first answer"),
                ("user", "second"),
                ("assistant", "second answer"),
                ("developer", CHILD_HINT),
            ]
        );
        assert!(
            report
                .reasons
                .contains(&SUBAGENT_FORK_CONTEXT_REASON_TOOL_CHATTER_FILTERED.to_owned())
        );
        assert!(
            report
                .reasons
                .contains(&SUBAGENT_FORK_CONTEXT_REASON_USAGE_HINT_FILTERED.to_owned())
        );
        assert!(
            report
                .reasons
                .contains(&SUBAGENT_FORK_CONTEXT_REASON_CHILD_USAGE_HINT_ADDED.to_owned())
        );
    }

    #[test]
    fn last_n_turns_keeps_prelude_and_recent_user_turns_only() {
        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::Last(1), sample_parent_messages()))
            .unwrap();

        assert_eq!(
            roles_and_content(&report.messages),
            [
                ("system", "base"),
                ("user", "second"),
                ("assistant", "second answer"),
                ("developer", CHILD_HINT),
            ]
        );
    }

    #[test]
    fn none_does_not_inherit_parent_messages_or_child_hint() {
        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::None, sample_parent_messages()))
            .unwrap();

        assert!(report.messages.is_empty());
        assert_eq!(report.retained_count, 0);
        assert!(
            report
                .reasons
                .contains(&SUBAGENT_FORK_CONTEXT_REASON_NONE.to_owned())
        );
    }

    #[test]
    fn last_n_turns_ignores_control_messages_when_counting_user_turns() {
        let mut parent_messages = sample_parent_messages();
        parent_messages.push(AgentModelMessage::conversation_checkpoint("checkpoint"));
        parent_messages.push(AgentModelMessage::runtime_note(
            crate::AgentRuntimeNoteKind::PayloadBudgetGate,
            "窗口预算提醒",
        ));

        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::Last(1), parent_messages))
            .unwrap();

        let contents = report
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert!(contents.contains(&"second"));
        assert!(!contents.contains(&"first"));
        assert!(report.messages.iter().any(|message| {
            message.kind == crate::AgentModelMessageKind::ConversationCheckpoint
        }));
        assert!(report.messages.iter().all(|message| {
            !matches!(
                message.kind,
                crate::AgentModelMessageKind::RuntimeNote(
                    crate::AgentRuntimeNoteKind::PayloadBudgetGate
                )
            )
        }));
    }

    #[test]
    fn fork_filters_parent_context_and_p3_messages_but_keeps_checkpoint() {
        let mut parent_messages = sample_parent_messages();
        parent_messages.push(AgentModelMessage::context("user", "parent context"));
        parent_messages.push(AgentModelMessage::runtime_note(
            crate::AgentRuntimeNoteKind::DeferredDiagnostics,
            "parent diagnostics",
        ));
        parent_messages.push(AgentModelMessage::conversation_checkpoint("checkpoint"));

        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::All, parent_messages))
            .unwrap();

        assert!(report.messages.iter().all(|message| {
            message.content != "parent context"
                && !matches!(
                    message.kind,
                    crate::AgentModelMessageKind::RuntimeNote(
                        crate::AgentRuntimeNoteKind::DeferredDiagnostics
                    )
                )
        }));
        assert!(report.messages.iter().any(|message| {
            message.kind == crate::AgentModelMessageKind::ConversationCheckpoint
        }));
    }

    #[test]
    fn fork_never_inherits_dynamic_workspace_context() {
        let mut parent_messages = sample_parent_messages();
        parent_messages.splice(
            1..1,
            [
                AgentModelMessage::text(
                    "developer",
                    "# AGENTS.md instructions for old\n<INSTRUCTIONS>旧规则</INSTRUCTIONS>",
                ),
                AgentModelMessage::text(
                    "user",
                    "<available-skills-context>旧 Skill</available-skills-context>",
                ),
                AgentModelMessage::text(
                    "system",
                    "<available-mcp-context>旧 MCP</available-mcp-context>",
                ),
                AgentModelMessage::text(
                    "developer",
                    "<workspace-transition-context>旧切换回执</workspace-transition-context>",
                ),
                AgentModelMessage::text("user", "请检查文本中的 cwd=E:/workspace"),
            ],
        );

        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::All, parent_messages))
            .unwrap();

        assert!(
            report
                .messages
                .iter()
                .all(|message| !contains_dynamic_workspace_marker(&message.content))
        );
        assert!(
            report
                .messages
                .iter()
                .any(|message| message.content == "请检查文本中的 cwd=E:/workspace")
        );
    }

    #[test]
    fn fork_never_inherits_the_parent_soul() {
        let mut parent_messages = sample_parent_messages();
        parent_messages.insert(
            1,
            AgentModelMessage::text(
                "system",
                format!(
                    "{SESSION_STABLE_SOUL_START_MARKER}\n主代理人格\n{SESSION_STABLE_SOUL_END_MARKER}"
                ),
            ),
        );

        let report = SubagentForkContextRuntime
            .plan(input(SubagentForkTurns::All, parent_messages))
            .unwrap();

        assert!(
            report
                .messages
                .iter()
                .all(|message| !is_soul_context(message))
        );
        assert!(
            report
                .reasons
                .contains(&SUBAGENT_FORK_CONTEXT_REASON_SOUL_FILTERED.to_owned())
        );
    }

    fn input(
        fork_turns: SubagentForkTurns,
        parent_messages: Vec<AgentModelMessage>,
    ) -> SubagentForkContextInput {
        SubagentForkContextInput {
            parent_messages,
            fork_turns,
            parent_usage_hint_texts: vec![ROOT_HINT.to_owned()],
            child_usage_hint_text: Some(CHILD_HINT.to_owned()),
            usage_hint_enabled: true,
        }
    }

    fn sample_parent_messages() -> Vec<AgentModelMessage> {
        vec![
            AgentModelMessage::text("system", "base"),
            AgentModelMessage::text("developer", ROOT_HINT),
            AgentModelMessage::text("user", "first"),
            AgentModelMessage::text("assistant", "first answer"),
            assistant_tool_call_message(),
            AgentModelMessage::tool_result("call-1", "{}", true),
            AgentModelMessage::text("user", "second"),
            assistant_with_reasoning("reasoning"),
            AgentModelMessage::text("assistant", "second answer"),
        ]
    }

    fn assistant_tool_call_message() -> AgentModelMessage {
        AgentModelMessage::text("assistant", "").with_tool_calls(vec![AgentModelToolCallMetadata {
            id: "call-1".to_owned(),
            name: "builtin.search".to_owned(),
            input: "{}".into(),
        }])
    }

    fn assistant_with_reasoning(content: &str) -> AgentModelMessage {
        AgentModelMessage {
            provider_metadata: Some(AgentModelMessageProviderMetadata {
                openai_reasoning_content: Some(content.to_owned()),
                tool_calls: None,
                tool_result: None,
                anthropic_thinking_blocks: None,
            }),
            ..AgentModelMessage::text("assistant", content)
        }
    }

    fn roles_and_content(messages: &[AgentModelMessage]) -> Vec<(&str, &str)> {
        messages
            .iter()
            .map(|message| (message.role.as_str(), message.content.as_str()))
            .collect()
    }
}
