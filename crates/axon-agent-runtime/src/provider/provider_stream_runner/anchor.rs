//! Provider streaming prompt cache anchor 规划。

use axon_crypto::sha256_hex;
use axon_domain::AxonResult;

use crate::{
    AgentModelMessage, PromptCacheAnchor, PromptCacheAnchorInput, PromptCacheAnchorRuntime,
    PromptCacheDynamicContextRef, PromptCacheSegmentKind,
};

use super::{
    PROVIDER_STREAMING_RUNNER_POLICY_HASH_SOURCE, PROVIDER_STREAMING_RUNNER_SOURCE_REF,
    ProviderStreamingRunnerContext,
};

pub(super) fn provider_streaming_anchor(
    context: &ProviderStreamingRunnerContext,
) -> AxonResult<PromptCacheAnchor> {
    let system_prompt = context
        .messages
        .iter()
        .find(|message| message.role == "system")
        .map_or("", |message| message.content.as_str());
    let model_profile = format!(
        "provider={}|protocol={:?}|model={}|context_window={}|tools={}|vision={}",
        context.provider_name,
        context.protocol,
        context.model_id,
        context.context_window,
        context.supports_tools,
        context.supports_vision,
    );

    PromptCacheAnchorRuntime.plan(PromptCacheAnchorInput {
        system_prompt_hash: sha256_hex(system_prompt),
        developer_policy_hash: sha256_hex(PROVIDER_STREAMING_RUNNER_POLICY_HASH_SOURCE),
        tool_schema_hashes: context
            .model_tools
            .iter()
            .map(|tool| tool.schema_hash.clone())
            .collect(),
        soul_hash: stable_segment_hash(context, PromptCacheSegmentKind::Soul),
        user_agents_hash: stable_segment_hash(context, PromptCacheSegmentKind::UserAgents),
        personal_memory_hash: stable_segment_hash(context, PromptCacheSegmentKind::PersonalMemory),
        config_hash: stable_segment_hash(context, PromptCacheSegmentKind::Config),
        model_profile_hash: sha256_hex(&model_profile),
        dynamic_context: dynamic_context_audit(&context.messages),
        previous_anchor: None,
    })
}

fn stable_segment_hash(
    context: &ProviderStreamingRunnerContext,
    kind: PromptCacheSegmentKind,
) -> String {
    context
        .prompt_cache_plan
        .segments
        .iter()
        .find(|segment| segment.kind == kind)
        .map(|segment| segment.hash.clone())
        .unwrap_or_else(|| sha256_hex(""))
}

fn dynamic_context_audit(messages: &[AgentModelMessage]) -> Vec<PromptCacheDynamicContextRef> {
    let dynamic = messages
        .iter()
        .filter(|message| message.cache_excluded)
        .map(|message| format!("{}={}", message.role, message.content))
        .collect::<Vec<_>>()
        .join("|");
    if dynamic.is_empty() {
        return Vec::new();
    }
    vec![PromptCacheDynamicContextRef {
        source_ref: PROVIDER_STREAMING_RUNNER_SOURCE_REF.to_owned(),
        summary_hash: sha256_hex(&dynamic),
        token_estimate: u64::try_from(dynamic.chars().count() / 4).unwrap_or(u64::MAX),
    }]
}
