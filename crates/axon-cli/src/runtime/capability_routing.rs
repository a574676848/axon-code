//! 当前轮次的 Skill 路由提示。

use axon_agent_runtime::{PromptFragment, PromptFragmentMetadata};

/// 当前轮次的 Skill 路由 fragment。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapabilityRoutingFragment {
    content: String,
}

impl CapabilityRoutingFragment {
    /// 根据真实 Skill 推荐生成路由提示。
    #[must_use]
    pub(crate) fn from_runtime(relevant_skills: &str) -> Option<Self> {
        let relevant_skills = relevant_skills.trim();
        if relevant_skills.is_empty() {
            return None;
        }
        let lines = [
            "[Axon Capability Routing]",
            "scope=turn_ephemeral; method=5w1h_pdca",
            relevant_skills,
            "只选择能直接推进当前完成判据的能力。",
        ];
        Some(Self {
            content: lines.join("\n"),
        })
    }
}

impl PromptFragment for CapabilityRoutingFragment {
    fn metadata(&self) -> PromptFragmentMetadata {
        PromptFragmentMetadata::dynamic_unbudgeted_developer()
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<capability-routing>", "</capability-routing>")
    }

    fn body(&self) -> String {
        format!("\n{}\n", self.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::{
        PromptCacheProviderWireFormat, PromptFragmentPipeline, ProviderPromptCachePlanInput,
        ProviderPromptCachePlanRuntime,
    };

    #[test]
    fn capability_routing_requires_relevant_skills() {
        assert!(CapabilityRoutingFragment::from_runtime("   ").is_none());

        let fragment =
            CapabilityRoutingFragment::from_runtime("skill=review | reason=匹配当前任务")
                .expect("存在 Skill 推荐时应生成能力路由");

        assert!(fragment.content.contains("[Axon Capability Routing]"));
        assert!(fragment.content.contains("skill=review"));
        assert!(
            fragment
                .content
                .contains("只选择能直接推进当前完成判据的能力")
        );
        assert!(!fragment.content.contains("MCP"));
    }

    #[test]
    fn capability_routing_is_excluded_from_prompt_cache() {
        let fragment = CapabilityRoutingFragment {
            content: "[Axon Capability Routing]\nskill=review".to_owned(),
        };
        let mut pipeline = PromptFragmentPipeline::new();
        pipeline.push_fragment(Box::new(fragment));

        let messages = pipeline.into_messages();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "developer");
        assert!(messages[0].cache_excluded);
    }

    #[test]
    fn capability_routing_changes_do_not_invalidate_stable_prefix() {
        let first = cache_plan("skill=review");
        let second = cache_plan("skill=release-notes | reason=发布流程");

        assert_eq!(first.namespace_key, second.namespace_key);
        assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    }

    fn cache_plan(content: &str) -> axon_agent_runtime::ProviderPromptCachePlan {
        let fragment = CapabilityRoutingFragment {
            content: content.to_owned(),
        };
        let mut pipeline = PromptFragmentPipeline::new();
        pipeline.push_text(
            PromptFragmentMetadata::stable_system(),
            "stable-system-prompt",
        );
        pipeline.push_fragment(Box::new(fragment));
        pipeline.push_text(
            PromptFragmentMetadata::dynamic_user(),
            "current-user-request",
        );
        ProviderPromptCachePlanRuntime
            .plan(ProviderPromptCachePlanInput {
                provider_name: "test-provider".to_owned(),
                format: PromptCacheProviderWireFormat::OpenAiResponses,
                model_id: "test-model".to_owned(),
                context_window: 8_192,
                capabilities: axon_config::ProviderCapabilities::for_protocol(
                    axon_config::PROVIDER_PROTOCOL_OPENAI,
                ),
                supports_tools: true,
                supports_vision: false,
                messages: pipeline.into_messages().into(),
                model_tools: Vec::new(),
                context_section_count: 0,
            })
            .unwrap()
    }
}
