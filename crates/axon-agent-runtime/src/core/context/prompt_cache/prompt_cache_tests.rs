use super::{
    PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL, PROMPT_CACHE_WIRE_REASON_HINT_APPLIED,
    PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE, PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY,
    PromptCacheAnchorInput, PromptCacheDynamicContextRef, PromptCacheHintTarget,
    PromptCacheProviderWireFormat, PromptCacheProviderWireInput, PromptCacheProviderWireRuntime,
};

use crate::AgentModelMessage;
use crate::{
    ProviderPromptCachePlan, ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
};

#[test]
fn prompt_cache_wire_adds_anthropic_cache_control_to_system_and_latest_user() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: model_messages().into(),
        model_tools: vec![model_tool()],
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(
        plan.anthropic_system[0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(
        plan.anthropic_messages[0]["content"][0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert!(plan.openai_messages.is_empty());
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::LastSystemPrefix)
    );
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::LastToolSchema)
    );
    assert_eq!(
        plan.anthropic_tools[0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_HINT_APPLIED
    ));
    assert!(!plan.remote_raw_payload_allowed);
}

#[test]
fn prompt_cache_wire_leaves_openai_messages_provider_native() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: model_messages().into(),
        model_tools: vec![model_tool()],
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(plan.openai_messages.len(), 2);
    assert_eq!(plan.openai_messages[0]["role"], "system");
    assert!(plan.openai_messages[0].get("cache_control").is_none());
    assert!(plan.anthropic_system.is_empty());
    assert!(plan.anthropic_tools.is_empty());
    assert!(plan.applied_hints.is_empty());
    assert!(!plan.remote_raw_payload_allowed);
}

#[test]
fn prompt_cache_wire_preserves_openai_tool_result_metadata() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: tool_history("call_search", "status=completed").into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::OpenAiResponses),
    });

    assert_eq!(plan.openai_messages[1]["role"], "tool");
    assert_eq!(plan.openai_messages[1]["tool_call_id"], "call_search");
    assert_eq!(plan.openai_messages[1]["content"], "status=completed");
}

#[test]
fn prompt_cache_wire_preserves_openai_reasoning_content_metadata() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("assistant", "已完成一半")
                .with_openai_reasoning_content("step-by-step"),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::OpenAiResponses),
    });

    assert_eq!(plan.openai_messages[0]["role"], "assistant");
    assert_eq!(plan.openai_messages[0]["reasoning_content"], "step-by-step");
    assert!(plan.openai_messages[0].get("cache_control").is_none());
}

#[test]
fn prompt_cache_wire_skips_empty_tool_result_cache_control() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: tool_history("call_search", "   ").into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(
        plan.anthropic_messages[1]["content"][0]["type"],
        "tool_result"
    );
    assert!(
        plan.anthropic_messages[1]["content"][0]
            .get("cache_control")
            .is_none()
    );
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY
    ));
}

#[test]
fn prompt_cache_wire_projects_anthropic_tool_use_blocks_from_metadata() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("assistant", "").with_tool_calls(vec![
                crate::AgentModelToolCallMetadata {
                    id: "call_search".to_owned(),
                    name: "builtin_search".to_owned(),
                    input: r#"{"query":"panic"}"#.into(),
                },
            ]),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(plan.anthropic_messages[0]["role"], "assistant");
    assert_eq!(plan.anthropic_messages[0]["content"][0]["type"], "tool_use");
    assert_eq!(
        plan.anthropic_messages[0]["content"][0]["id"],
        "call_search"
    );
    assert_eq!(
        plan.anthropic_messages[0]["content"][0]["name"],
        "builtin_search"
    );
    assert_eq!(
        plan.anthropic_messages[0]["content"][0]["input"]["query"],
        "panic"
    );
    assert!(
        plan.anthropic_messages[0]["content"][0]
            .get("cache_control")
            .is_none()
    );
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE
    ));
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY
    ));
}

#[test]
fn prompt_cache_wire_keeps_developer_native_for_openai_responses() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::OpenAiResponses,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "system prompt"),
            AgentModelMessage::text("developer", "developer policy"),
            AgentModelMessage::text("user", "latest user"),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::OpenAiResponses),
    });

    assert_eq!(plan.openai_messages[0]["role"], "system");
    assert_eq!(plan.openai_messages[1]["role"], "developer");
    assert_eq!(plan.openai_messages[2]["role"], "user");
    assert!(plan.anthropic_system.is_empty());
}

#[test]
fn prompt_cache_wire_caps_anthropic_breakpoints_with_tools_system_messages_priority() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "system prompt"),
            AgentModelMessage::text("user", "第一轮提问"),
            AgentModelMessage::text("assistant", "第一轮回答"),
            AgentModelMessage::text("user", "第二轮提问"),
            AgentModelMessage::text("assistant", "第二轮回答"),
            AgentModelMessage::text("user", "第三轮提问"),
        ]
        .into(),
        model_tools: vec![model_tool()],
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(anthropic_cache_control_count(&plan), 4);
    assert_eq!(
        plan.anthropic_tools[0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(
        plan.anthropic_system[0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert!(
        plan.anthropic_messages[0]["content"][0]
            .get("cache_control")
            .is_none(),
        "超出 provider 上限时，较旧消息不应挤占 tools/system 断点"
    );
    assert_eq!(
        plan.anthropic_messages[3]["content"][0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(
        plan.anthropic_messages[4]["content"][0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
}

#[test]
fn prompt_cache_wire_prioritizes_global_stable_suffix_over_tool_breakpoint() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "system prompt"),
            AgentModelMessage::text(
                "system",
                "<assistant-profile-context>same soul</assistant-profile-context>",
            ),
            AgentModelMessage::text(
                "developer",
                "<user-agents-context>global policy</user-agents-context>",
            ),
            AgentModelMessage::text("user", "first request"),
            AgentModelMessage::text("assistant", "first response"),
            AgentModelMessage::text("user", "latest request"),
            AgentModelMessage::cache_excluded(
                "developer",
                "# AGENTS.md instructions\nproject policy",
            ),
        ]
        .into(),
        model_tools: vec![model_tool()],
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(anthropic_cache_control_count(&plan), 4);
    assert_eq!(
        plan.anthropic_system[2]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    let workspace_block = plan
        .anthropic_messages
        .iter()
        .filter_map(|message| message.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .find(|block| {
            block.get("text").and_then(serde_json::Value::as_str)
                == Some("# AGENTS.md instructions\nproject policy")
        })
        .expect("动态 Workspace developer 应在历史之后投影为 Anthropic user block");
    assert!(
        workspace_block.get("cache_control").is_none(),
        "动态 Workspace developer 消息不得获得 cache_control"
    );
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::LastSessionStableContext)
    );
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::LastToolSchema)
    );
}

#[test]
fn prompt_cache_wire_marks_rolling_double_breakpoints_on_message_tail() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage {
                role: "system".to_owned(),
                content: "system prompt".to_owned(),
                provider_metadata: None,
                local_images: Vec::new(),
                local_files: Vec::new(),
                cache_excluded: false,
                source: crate::AgentModelMessageSource::Conversation,
                kind: crate::AgentModelMessageKind::Regular,
                retention_class: crate::RetentionClass::P1State,
                source_ref: None,
            },
            AgentModelMessage {
                role: "user".to_owned(),
                content: "第一轮提问".to_owned(),
                provider_metadata: None,
                local_images: Vec::new(),
                local_files: Vec::new(),
                cache_excluded: false,
                source: crate::AgentModelMessageSource::Conversation,
                kind: crate::AgentModelMessageKind::Regular,
                retention_class: crate::RetentionClass::P1State,
                source_ref: None,
            },
            AgentModelMessage {
                role: "assistant".to_owned(),
                content: "第一轮回答".to_owned(),
                provider_metadata: None,
                local_images: Vec::new(),
                local_files: Vec::new(),
                cache_excluded: false,
                source: crate::AgentModelMessageSource::Conversation,
                kind: crate::AgentModelMessageKind::Regular,
                retention_class: crate::RetentionClass::P1State,
                source_ref: None,
            },
            AgentModelMessage {
                role: "user".to_owned(),
                content: "第二轮提问".to_owned(),
                provider_metadata: None,
                local_images: Vec::new(),
                local_files: Vec::new(),
                cache_excluded: false,
                source: crate::AgentModelMessageSource::Conversation,
                kind: crate::AgentModelMessageKind::Regular,
                retention_class: crate::RetentionClass::P0Contract,
                source_ref: None,
            },
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    // 末尾最近 2 条非 system 消息（assistant + 最新 user）各打一个滚动断点。
    assert_eq!(plan.anthropic_messages.len(), 3);
    assert!(
        plan.anthropic_messages[0]["content"][0]
            .get("cache_control")
            .is_none(),
        "最旧的 user 不应锚定，使其落入可命中前缀"
    );
    assert_eq!(
        plan.anthropic_messages[1]["content"][0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL,
        "上一轮 tail 断点本轮成为 READ 命中前缀"
    );
    assert_eq!(
        plan.anthropic_messages[2]["content"][0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL,
        "最新消息打 WRITE 断点"
    );
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
}

#[test]
fn prompt_cache_wire_does_not_cache_tool_result_breakpoints() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: tool_history("call_search", "status=completed").into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(plan.anthropic_messages[1]["role"], "user");
    assert_eq!(
        plan.anthropic_messages[1]["content"][0]["type"],
        "tool_result"
    );
    assert_eq!(
        plan.anthropic_messages[1]["content"][0]["tool_use_id"],
        "call_search"
    );
    assert_eq!(
        plan.anthropic_messages[1]["content"][0]["content"],
        "status=completed"
    );
    assert!(
        plan.anthropic_messages[1]["content"][0]
            .get("cache_control")
            .is_none()
    );
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY
    ));
}

fn tool_history(call_id: &str, output: &str) -> Vec<AgentModelMessage> {
    vec![
        AgentModelMessage::text("assistant", "").with_tool_calls(vec![
            crate::AgentModelToolCallMetadata {
                id: call_id.to_owned(),
                name: "builtin.search".to_owned(),
                input: "{}".into(),
            },
        ]),
        AgentModelMessage::tool_result(call_id, output, true),
    ]
}

pub(super) fn anchor_input(
    dynamic_context: PromptCacheDynamicContextRef,
) -> PromptCacheAnchorInput {
    PromptCacheAnchorInput {
        system_prompt_hash: "system-v1".to_owned(),
        developer_policy_hash: "developer-v1".to_owned(),
        tool_schema_hashes: vec!["tool-schema-v1".to_owned()],
        soul_hash: "soul-v1".to_owned(),
        user_agents_hash: "user-agents-v1".to_owned(),
        personal_memory_hash: "personal-memory-v1".to_owned(),
        config_hash: "config-v1".to_owned(),
        model_profile_hash: "model-profile-v1".to_owned(),
        dynamic_context: vec![dynamic_context],
        previous_anchor: None,
    }
}

pub(super) fn dynamic_context(
    source_ref: &str,
    summary_hash: &str,
) -> PromptCacheDynamicContextRef {
    PromptCacheDynamicContextRef {
        source_ref: source_ref.to_owned(),
        summary_hash: summary_hash.to_owned(),
        token_estimate: 512,
    }
}

fn model_messages() -> Vec<AgentModelMessage> {
    vec![
        AgentModelMessage {
            role: "system".to_owned(),
            content: "system prompt".to_owned(),
            provider_metadata: None,
            local_images: Vec::new(),
            local_files: Vec::new(),
            cache_excluded: false,
            source: crate::AgentModelMessageSource::Conversation,
            kind: crate::AgentModelMessageKind::Regular,
            retention_class: crate::RetentionClass::P1State,
            source_ref: None,
        },
        AgentModelMessage {
            role: "user".to_owned(),
            content: "latest user".to_owned(),
            provider_metadata: None,
            local_images: Vec::new(),
            local_files: Vec::new(),
            cache_excluded: false,
            source: crate::AgentModelMessageSource::Conversation,
            kind: crate::AgentModelMessageKind::Regular,
            retention_class: crate::RetentionClass::P0Contract,
            source_ref: None,
        },
    ]
}

fn model_tool() -> crate::ModelToolDefinition {
    crate::ModelToolDefinition {
        qualified_name: "builtin.search".to_owned(),
        description: "只读搜索当前工作区。".to_owned(),
        schema_hash: "tool-schema-search".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

fn anthropic_cache_control_count(plan: &super::PromptCacheProviderWirePlan) -> usize {
    let system = plan
        .anthropic_system
        .iter()
        .filter(|block| block.get("cache_control").is_some())
        .count();
    let messages = plan
        .anthropic_messages
        .iter()
        .filter_map(|message| message.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .filter(|block| block.get("cache_control").is_some())
        .count();
    let tools = plan
        .anthropic_tools
        .iter()
        .filter(|tool| tool.get("cache_control").is_some())
        .count();
    system + messages + tools
}

fn has_reason(reasons: &[String], reason: &str) -> bool {
    reasons.iter().any(|item| item == reason)
}

fn test_prompt_cache_plan(format: PromptCacheProviderWireFormat) -> ProviderPromptCachePlan {
    let capabilities = match format {
        PromptCacheProviderWireFormat::AnthropicMessages => {
            axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
            )
        }
        PromptCacheProviderWireFormat::OpenAiResponses => {
            axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI)
        }
        PromptCacheProviderWireFormat::OpenAiCompatibleChat => {
            axon_config::ProviderCapabilities::stateless()
        }
    };
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities,
            format,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: Vec::new().into(),
            model_tools: Vec::new(),
            context_section_count: 0,
        })
        .unwrap()
}
