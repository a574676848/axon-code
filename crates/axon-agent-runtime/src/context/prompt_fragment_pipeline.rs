//! Prompt fragment pipeline：统一解释 fragment 元数据并投影为模型消息。

use crate::{AgentModelMessage, RetentionClass};
use axon_context_fragments::{
    PromptFragment, PromptFragmentBudget, PromptFragmentLifecycle, PromptFragmentMetadata,
};

/// Prompt fragment pipeline。
///
/// 该 pipeline 只承载 Axon 注入的 prompt fragments。用户最终输入仍由
/// `AgentModelPayloadRuntime` 独立构造，避免把真实 user prompt 误标为系统注入内容。
#[derive(Debug, Default)]
pub struct PromptFragmentPipeline {
    entries: Vec<PromptFragmentPipelineEntry>,
}

impl PromptFragmentPipeline {
    /// 创建空 pipeline。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加原始文本片段。
    pub fn push_text(&mut self, metadata: PromptFragmentMetadata, content: impl Into<String>) {
        self.entries.push(PromptFragmentPipelineEntry {
            metadata,
            content: content.into(),
        });
    }

    /// 追加渲染后的 typed fragment。
    pub fn push_fragment(&mut self, fragment: Box<dyn PromptFragment>) {
        self.push_text(fragment.metadata(), fragment.render());
    }

    /// 批量追加 typed fragments。
    pub fn extend_fragments(&mut self, fragments: Vec<Box<dyn PromptFragment>>) {
        for fragment in fragments {
            self.push_fragment(fragment);
        }
    }

    /// 将 pipeline 投影为 provider-visible messages。
    #[must_use]
    pub fn into_messages(self) -> Vec<AgentModelMessage> {
        self.entries
            .into_iter()
            .map(PromptFragmentPipelineEntry::into_message)
            .collect()
    }
}

#[derive(Debug)]
struct PromptFragmentPipelineEntry {
    metadata: PromptFragmentMetadata,
    content: String,
}

impl PromptFragmentPipelineEntry {
    fn into_message(self) -> AgentModelMessage {
        let message = AgentModelMessage::context(self.metadata.role.as_str(), self.content);
        let retention_class = retention_class_for_fragment(&self.metadata);
        let message = AgentModelMessage {
            retention_class,
            ..message
        };
        if should_exclude_from_cache(&self.metadata) {
            AgentModelMessage {
                cache_excluded: true,
                ..message
            }
        } else {
            message
        }
    }
}

fn should_exclude_from_cache(metadata: &PromptFragmentMetadata) -> bool {
    metadata.lifecycle == PromptFragmentLifecycle::Ephemeral
        || metadata.budget == PromptFragmentBudget::Unbudgeted
}

fn retention_class_for_fragment(metadata: &PromptFragmentMetadata) -> RetentionClass {
    if metadata.lifecycle == PromptFragmentLifecycle::Ephemeral
        && metadata.budget == PromptFragmentBudget::Unbudgeted
    {
        RetentionClass::P3Derivable
    } else if metadata.stability == axon_context_fragments::PromptFragmentStability::Stable {
        RetentionClass::P1State
    } else {
        RetentionClass::P2Evidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_projects_fragment_role_and_cache_policy() {
        let mut pipeline = PromptFragmentPipeline::new();
        pipeline.push_text(
            PromptFragmentMetadata::stable_system(),
            "[Axon Identity]\nstable system",
        );
        pipeline.push_text(
            PromptFragmentMetadata::stable_developer(),
            "<rules>developer rules</rules>",
        );
        pipeline.push_text(
            PromptFragmentMetadata::dynamic_cacheable_user(),
            "<context>workspace context</context>",
        );
        pipeline.push_text(
            PromptFragmentMetadata::dynamic_unbudgeted_developer(),
            "<runtime-constraints>control</runtime-constraints>",
        );

        let messages = pipeline.into_messages();

        assert_eq!(messages[0].role, "system");
        assert!(!messages[0].cache_excluded);
        assert_eq!(messages[1].role, "developer");
        assert!(!messages[1].cache_excluded);
        assert_eq!(messages[2].role, "user");
        assert!(!messages[2].cache_excluded);
        assert_eq!(messages[3].role, "developer");
        assert!(messages[3].cache_excluded);
    }
}
