//! Skill context fragments。

use axon_agent_runtime::{
    AVAILABLE_SKILLS_END_MARKER, AVAILABLE_SKILLS_START_MARKER, PromptFragment,
    PromptFragmentMetadata,
};

/// 可用技能上下文 fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AvailableSkillsDeveloperFragment {
    content: String,
}

impl AvailableSkillsDeveloperFragment {
    /// 构造可用技能上下文 fragment。
    #[must_use]
    pub(crate) fn optional(content: impl Into<String>) -> Option<Self> {
        let content = content.into();
        let content = content.trim().to_owned();
        (!content.is_empty()).then_some(Self { content })
    }
}

impl PromptFragment for AvailableSkillsDeveloperFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (AVAILABLE_SKILLS_START_MARKER, AVAILABLE_SKILLS_END_MARKER)
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}

/// 已选技能上下文 fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedSkillDeveloperFragment {
    content: String,
}

impl SelectedSkillDeveloperFragment {
    /// 构造已选技能上下文 fragment。
    #[must_use]
    pub(crate) fn optional(content: impl Into<String>) -> Option<Self> {
        let content = content.into();
        let content = content.trim().to_owned();
        (!content.is_empty()).then_some(Self { content })
    }
}

impl PromptFragment for SelectedSkillDeveloperFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<selected-skill-context>", "</selected-skill-context>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}
