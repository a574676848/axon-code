//! Runtime prompt fragments shared by model payload assembly surfaces.

use axon_agent_runtime::{
    AgentModelMessage, PromptFragment, PromptFragmentMetadata, PromptFragmentPipeline,
    SESSION_STABLE_PERSONAL_MEMORY_END_MARKER, SESSION_STABLE_PERSONAL_MEMORY_START_MARKER,
};
use axon_memory_read::PersonalMemorySnapshot;

use crate::{
    runtime_skill_context_fragments::{
        AvailableSkillsDeveloperFragment, SelectedSkillDeveloperFragment,
    },
    runtime_workspace_instructions::{
        WorkspaceInstruction, WorkspaceInstructionsDeveloperFragment,
        workspace_instructions_developer_fragment,
    },
};

/// Input for provider-visible runtime context fragments.
pub(crate) struct RuntimePromptFragmentsInput<'a> {
    /// Current workspace directory label.
    pub(crate) cwd: &'a str,
    /// Loaded project-level Workspace AGENTS.md instructions.
    pub(crate) workspace_instructions: &'a [WorkspaceInstruction],
    /// Rendered available skills catalog context.
    pub(crate) available_skills_context: &'a str,
    /// Selected skill body, if the user explicitly selected one.
    pub(crate) selected_skill_prompt: Option<String>,
}

/// Build contextual developer fragments shared by all local model-entry surfaces.
pub(crate) fn runtime_prompt_fragments(
    input: RuntimePromptFragmentsInput<'_>,
) -> Vec<Box<dyn PromptFragment>> {
    let mut fragments: Vec<Box<dyn PromptFragment>> = Vec::new();
    if let Some(fragment) =
        workspace_instructions_developer_fragment(Some(input.cwd), input.workspace_instructions)
    {
        fragments.push(Box::new(fragment));
    }
    if let Some(fragment) =
        AvailableSkillsDeveloperFragment::optional(input.available_skills_context)
    {
        fragments.push(Box::new(fragment));
    }
    if let Some(fragment) = input
        .selected_skill_prompt
        .and_then(SelectedSkillDeveloperFragment::optional)
    {
        fragments.push(Box::new(fragment));
    }
    fragments
}

/// 将当前 Workspace 的动态 fragments 预渲染为 Provider 可见消息。
pub(crate) fn runtime_prompt_fragment_messages(
    input: RuntimePromptFragmentsInput<'_>,
) -> Vec<AgentModelMessage> {
    let mut pipeline = PromptFragmentPipeline::new();
    pipeline.extend_fragments(runtime_prompt_fragments(input));
    pipeline.into_messages()
}

/// 组装主 turn 与后台 Execution 共用的跨 Workspace 稳定上下文消息序列。
pub(crate) fn assemble_cross_workspace_stable_context_messages(
    soul_context_message: Option<&AgentModelMessage>,
    personal_memory: Option<&PersonalMemorySnapshot>,
    include_personal_memory: bool,
    user_agents_message: Option<&AgentModelMessage>,
) -> Vec<AgentModelMessage> {
    let mut messages = Vec::new();
    messages.extend(soul_context_message.cloned());
    messages.extend(user_agents_message.cloned());
    if include_personal_memory
        && let Some(snapshot) = personal_memory
        && let Some(message) = personal_memory_context_message(snapshot)
    {
        messages.push(message);
    }
    messages
}

/// Personal Memory 稳定上下文 fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonalMemoryDeveloperFragment {
    facts: Vec<String>,
}

impl PersonalMemoryDeveloperFragment {
    fn from_snapshot(snapshot: &PersonalMemorySnapshot) -> Option<Self> {
        let facts = snapshot
            .context
            .iter()
            .map(|fact| fact.trim())
            .filter(|fact| !fact.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        (!facts.is_empty()).then_some(Self { facts })
    }
}

impl PromptFragment for PersonalMemoryDeveloperFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::stable_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            SESSION_STABLE_PERSONAL_MEMORY_START_MARKER,
            SESSION_STABLE_PERSONAL_MEMORY_END_MARKER,
        )
    }

    fn body(&self) -> String {
        let facts = self
            .facts
            .iter()
            .map(|fact| format!("- {fact}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n以下是你已经了解并可直接遵循的长期个人信息：\n{facts}\n")
    }
}

/// 将已发布的 Personal Memory 快照渲染为稳定、可缓存的主代理上下文。
pub(crate) fn personal_memory_context_message(
    snapshot: &PersonalMemorySnapshot,
) -> Option<AgentModelMessage> {
    let fragment = PersonalMemoryDeveloperFragment::from_snapshot(snapshot)?;
    let mut pipeline = PromptFragmentPipeline::new();
    pipeline.push_fragment(Box::new(fragment));
    pipeline.into_messages().into_iter().next()
}

fn is_personal_memory_context_message(message: &AgentModelMessage) -> bool {
    message.role == "developer"
        && !message.cache_excluded
        && PersonalMemoryDeveloperFragment::matches_text(&message.content)
}

/// Detect runtime prompt fragments inherited from a parent payload.
pub(crate) fn is_runtime_prompt_fragment_message(message: &AgentModelMessage) -> bool {
    message.role == "developer"
        && (WorkspaceInstructionsDeveloperFragment::matches_text(&message.content)
            || AvailableSkillsDeveloperFragment::matches_text(&message.content)
            || SelectedSkillDeveloperFragment::matches_text(&message.content)
            || is_personal_memory_context_message(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_workspace_instructions::InstructionProvenance;

    #[test]
    fn cross_workspace_stable_messages_are_complete_independent_and_ordered() {
        let soul = AgentModelMessage::text(
            "system",
            "<assistant-profile-context>同一 Soul</assistant-profile-context>",
        );
        let memory = PersonalMemorySnapshot {
            id: "memory-order".to_owned(),
            revision: 1,
            context: vec!["偏好中文".to_owned()],
        };
        let user_agents = AgentModelMessage::text(
            "developer",
            "<user-agents-context>全局规则</user-agents-context>",
        );

        let messages = assemble_cross_workspace_stable_context_messages(
            Some(&soul),
            Some(&memory),
            true,
            Some(&user_agents),
        );

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0], soul);
        assert_eq!(messages[1], user_agents);
        assert!(messages[2].content.contains("<personal-memory-context>"));
        assert!(messages[2].content.contains("偏好中文"));
        assert!(messages.iter().all(|message| !message.cache_excluded));
        assert!(
            messages
                .iter()
                .all(|message| !message.content.contains("# AGENTS.md instructions"))
        );
        assert!(
            messages
                .iter()
                .all(|message| !message.content.contains("<available-skills-context>"))
        );
    }

    #[test]
    fn runtime_prompt_fragments_keep_agents_before_skill_context() {
        let workspace_instructions = vec![WorkspaceInstruction {
            path: "AGENTS.md".to_owned(),
            content: "项目规则".to_owned(),
            provenance: InstructionProvenance::Project,
        }];

        let fragments = runtime_prompt_fragments(RuntimePromptFragmentsInput {
            cwd: "E:/zbg/axon-code",
            workspace_instructions: &workspace_instructions,
            available_skills_context: "name: review",
            selected_skill_prompt: Some("Review body".to_owned()),
        });

        let rendered = fragments
            .iter()
            .map(|fragment| (fragment.role(), fragment.render()))
            .collect::<Vec<_>>();

        assert_eq!(
            rendered.iter().map(|(role, _)| *role).collect::<Vec<_>>(),
            ["developer", "developer", "developer"]
        );
        assert!(rendered[0].1.contains("# AGENTS.md instructions"));
        assert!(rendered[0].1.contains("<INSTRUCTIONS>"));
        assert!(rendered[0].1.contains("项目规则"));
        assert!(rendered[1].1.contains("<available-skills-context>"));
        assert!(rendered[2].1.contains("<selected-skill-context>"));

        let mut pipeline = PromptFragmentPipeline::new();
        pipeline.extend_fragments(runtime_prompt_fragments(RuntimePromptFragmentsInput {
            cwd: "E:/zbg/axon-code",
            workspace_instructions: &workspace_instructions,
            available_skills_context: "name: review",
            selected_skill_prompt: Some("Review body".to_owned()),
        }));
        assert!(
            pipeline
                .into_messages()
                .iter()
                .all(|message| message.role == "developer" && message.cache_excluded)
        );
    }

    #[test]
    fn personal_memory_context_is_stable_and_filterable() {
        let snapshot = PersonalMemorySnapshot {
            id: "personal-memory-test".to_owned(),
            revision: 7,
            context: vec!["用户偏好中文".to_owned(), "默认称呼长官".to_owned()],
        };
        let message = personal_memory_context_message(&snapshot).expect("非空快照应生成消息");

        assert_eq!(message.role, "developer");
        assert!(!message.cache_excluded);
        assert!(message.content.contains("用户偏好中文"));
        assert!(message.content.contains("默认称呼长官"));
        assert!(is_runtime_prompt_fragment_message(&message));
    }
}
