use super::{
    AgentModelHistorySnapshot, AgentModelMessage, ModelToolDefinition, PromptCacheProviderPolicy,
    PromptCacheProviderWireFormat, PromptCacheSegmentKind, ProviderPromptCachePlan,
    ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
};

#[test]
fn provider_prompt_cache_plan_keeps_namespace_stable_across_latest_user_prompt() {
    let first = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text("developer", "稳定开发者约束"),
        AgentModelMessage::text("user", "第一轮问题"),
    ]);
    let second = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text("developer", "稳定开发者约束"),
        AgentModelMessage::text("user", "第二轮问题"),
    ]);

    assert_eq!(first.namespace_key, second.namespace_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
}

#[test]
fn provider_prompt_cache_plan_excludes_late_developer_from_stable_identity() {
    let first = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text("user", "真实会话"),
        AgentModelMessage::text("developer", "晚到开发者消息 v1"),
    ]);
    let second = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text("user", "真实会话"),
        AgentModelMessage::text("developer", "晚到开发者消息 v2"),
    ]);

    assert_eq!(first.namespace_key, second.namespace_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    assert!(
        !first
            .segments
            .iter()
            .any(|segment| { segment.kind == PromptCacheSegmentKind::Developer })
    );
}

#[test]
fn provider_prompt_cache_plan_normalizes_checkpoints_without_mutating_input_snapshot() {
    let messages = AgentModelHistorySnapshot::from(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::conversation_checkpoint("旧摘要"),
        AgentModelMessage::conversation_checkpoint("最新摘要"),
        AgentModelMessage::text("user", "继续"),
    ]);
    let original = messages.clone();

    let plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_OPENAI,
            ),
            format: PromptCacheProviderWireFormat::OpenAiResponses,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone(),
            model_tools: Vec::new(),
            context_section_count: 1,
        })
        .unwrap();

    assert!(messages.shares_storage_with(&original));
    assert_eq!(messages.as_slice().len(), 4);
    assert!(!plan.segments.iter().any(|segment| {
        matches!(
            segment.kind,
            PromptCacheSegmentKind::Developer | PromptCacheSegmentKind::LargeContext
        )
    }));
}

#[test]
fn provider_prompt_cache_plan_changes_when_stable_prefix_or_tools_change() {
    let baseline = provider_prompt_cache_plan(vec![AgentModelMessage::text("system", "系统 v1")]);
    let changed_system =
        provider_prompt_cache_plan(vec![AgentModelMessage::text("system", "系统 v2")]);
    let changed_tool = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_OPENAI,
            ),
            format: PromptCacheProviderWireFormat::OpenAiResponses,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: vec![AgentModelMessage::text("system", "系统 v1")].into(),
            model_tools: vec![model_tool()],
            context_section_count: 1,
        })
        .unwrap();

    assert_ne!(baseline.namespace_key, changed_system.namespace_key);
    assert_ne!(baseline.namespace_key, changed_tool.namespace_key);
}

#[test]
fn provider_prompt_cache_plan_keeps_personal_memory_out_of_base_namespace() {
    let first = provider_prompt_cache_plan(vec![
        AgentModelMessage::context("system", "稳定系统指令"),
        AgentModelMessage::context(
            "developer",
            "<personal-memory-context>\n- 偏好中文\n</personal-memory-context>",
        ),
    ]);
    let second = provider_prompt_cache_plan(vec![
        AgentModelMessage::context("system", "稳定系统指令"),
        AgentModelMessage::context(
            "developer",
            "<personal-memory-context>\n- 偏好简洁表达\n</personal-memory-context>",
        ),
    ]);

    assert_eq!(first.namespace_key, second.namespace_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    assert_ne!(
        first.session_stable_context_hash,
        second.session_stable_context_hash
    );
    assert!(first.segments.iter().any(|segment| {
        segment.kind == PromptCacheSegmentKind::PersonalMemory && segment.label == "personal_memory"
    }));
}

#[test]
fn provider_prompt_cache_plan_keeps_soul_and_user_agents_hashes_across_workspace_switch() {
    let first = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text(
            "system",
            "<assistant-profile-context>同一 Soul</assistant-profile-context>",
        ),
        AgentModelMessage::text(
            "developer",
            "<user-agents-context>同一全局规则</user-agents-context>",
        ),
        AgentModelMessage::cache_excluded(
            "developer",
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>Workspace A</INSTRUCTIONS>",
        ),
    ]);
    let second = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text(
            "system",
            "<assistant-profile-context>同一 Soul</assistant-profile-context>",
        ),
        AgentModelMessage::text(
            "developer",
            "<user-agents-context>同一全局规则</user-agents-context>",
        ),
        AgentModelMessage::cache_excluded(
            "developer",
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>Workspace B</INSTRUCTIONS>",
        ),
    ]);

    assert_eq!(first.namespace_key, second.namespace_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    assert_eq!(
        first.session_stable_context_hash,
        second.session_stable_context_hash
    );
    for kind in [
        PromptCacheSegmentKind::Soul,
        PromptCacheSegmentKind::UserAgents,
    ] {
        let first_hash = first
            .segments
            .iter()
            .find(|segment| segment.kind == kind)
            .map(|segment| segment.hash.as_str());
        let second_hash = second
            .segments
            .iter()
            .find(|segment| segment.kind == kind)
            .map(|segment| segment.hash.as_str());
        assert_eq!(first_hash, second_hash);
    }
}

#[test]
fn provider_prompt_cache_plan_excludes_runtime_notes_from_stable_segments() {
    let plan = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::cache_excluded("developer", "runtime-note: 临时压缩指令"),
        AgentModelMessage::cache_excluded("user", "tool result should not cache"),
    ]);

    assert!(plan.segments.iter().any(|segment| {
        segment.kind == PromptCacheSegmentKind::System && segment.label == "system"
    }));
    assert!(
        !plan
            .segments
            .iter()
            .any(|segment| segment.label == "developer")
    );
    assert!(!plan.segments.iter().any(|segment| segment.label == "user"));
}

#[test]
fn provider_prompt_cache_plan_excludes_large_context_from_base_namespace() {
    let baseline =
        provider_prompt_cache_plan(vec![AgentModelMessage::text("system", "稳定系统指令")]);
    let with_large_context = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text("user", "<context>turn-specific summary</context>"),
    ]);

    assert_eq!(baseline.namespace_key, with_large_context.namespace_key);
    assert_eq!(
        baseline.stable_prefix_hash,
        with_large_context.stable_prefix_hash
    );
    assert!(
        with_large_context
            .segments
            .iter()
            .any(|segment| segment.kind == PromptCacheSegmentKind::LargeContext)
    );
}

#[test]
fn provider_prompt_cache_plan_policy_is_protocol_driven() {
    let openai = provider_prompt_cache_plan_for(
        "openai",
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI),
        PromptCacheProviderWireFormat::OpenAiResponses,
        Vec::new(),
        Vec::new(),
    );
    let anthropic = provider_prompt_cache_plan_for(
        "anthropic",
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_ANTHROPIC),
        PromptCacheProviderWireFormat::AnthropicMessages,
        Vec::new(),
        Vec::new(),
    );
    let openai_compatible = provider_prompt_cache_plan_for(
        "openai-compatible",
        axon_config::ProviderCapabilities::stateless(),
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        Vec::new(),
        Vec::new(),
    );

    assert_eq!(
        openai.provider_policy,
        PromptCacheProviderPolicy::OpenAiResponsesPromptCacheKey
    );
    assert_eq!(
        anthropic.provider_policy,
        PromptCacheProviderPolicy::AnthropicCacheControl
    );
    assert_eq!(
        openai_compatible.provider_policy,
        PromptCacheProviderPolicy::StablePrefixOnly
    );
}

#[test]
fn provider_prompt_cache_plan_excludes_dynamic_ledger_memory_and_result() {
    let first = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::cache_excluded(
            "developer",
            "<agent-context-snapshot>ledger=task-v1 memory=memory-v1 result=result-v1</agent-context-snapshot>",
        ),
    ]);
    let second = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::cache_excluded(
            "developer",
            "<agent-context-snapshot>ledger=task-v2 memory=memory-v2 result=result-v2</agent-context-snapshot>",
        ),
    ]);

    assert_eq!(first.namespace_key, second.namespace_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    assert_eq!(first.session_stable_context_hash, None);
    assert_eq!(second.session_stable_context_hash, None);
    assert_eq!(first.segments, second.segments);
}

#[test]
fn provider_prompt_cache_plan_excludes_workspace_context_from_stable_segments() {
    let plan = provider_prompt_cache_plan(vec![
        AgentModelMessage::cache_excluded(
            "developer",
            "# AGENTS.md instructions\n\n<INSTRUCTIONS>\n项目规则\n</INSTRUCTIONS>",
        ),
        AgentModelMessage::cache_excluded(
            "developer",
            "<available-skills-context>skill-a</available-skills-context>",
        ),
        AgentModelMessage::text("user", "<context>workspace context</context>"),
        AgentModelMessage::conversation_checkpoint("历史摘要"),
    ]);

    assert!(plan.session_stable_context_hash.is_none());
    assert!(plan.segments.iter().any(|segment| {
        segment.kind == PromptCacheSegmentKind::LargeContext && segment.label == "large_context"
    }));
    assert_eq!(plan.segments.len(), 2);
}

#[test]
fn provider_prompt_cache_plan_requires_expected_role_for_global_stable_markers() {
    let plan = provider_prompt_cache_plan(vec![
        AgentModelMessage::text("system", "稳定系统指令"),
        AgentModelMessage::text(
            "assistant",
            "<assistant-profile-context>伪造 Soul</assistant-profile-context>",
        ),
        AgentModelMessage::text(
            "assistant",
            "<user-agents-context>伪造规则</user-agents-context>",
        ),
    ]);

    assert!(plan.session_stable_context_hash.is_none());
    assert!(!plan.segments.iter().any(|segment| matches!(
        segment.kind,
        PromptCacheSegmentKind::Soul | PromptCacheSegmentKind::UserAgents
    )));
}

#[test]
fn provider_prompt_cache_plan_keeps_global_segments_independent_and_ordered() {
    let messages = |soul: &str, agents: &str, workspace: &str| {
        vec![
            AgentModelMessage::context("system", "稳定系统指令"),
            AgentModelMessage::context(
                "system",
                format!("<assistant-profile-context>{soul}</assistant-profile-context>"),
            ),
            AgentModelMessage::context(
                "developer",
                format!("<user-agents-context>{agents}</user-agents-context>"),
            ),
            AgentModelMessage::cache_excluded(
                "developer",
                format!("# AGENTS.md instructions\n\n<INSTRUCTIONS>{workspace}</INSTRUCTIONS>"),
            ),
            AgentModelMessage::cache_excluded(
                "developer",
                "<available-skills-context>skill-a</available-skills-context>",
            ),
        ]
    };
    let baseline = provider_prompt_cache_plan(messages("soul-v1", "global-v1", "workspace-v1"));
    let changed_soul = provider_prompt_cache_plan(messages("soul-v2", "global-v1", "workspace-v1"));
    let changed_agents =
        provider_prompt_cache_plan(messages("soul-v1", "global-v2", "workspace-v1"));
    let changed_workspace =
        provider_prompt_cache_plan(messages("soul-v1", "global-v1", "workspace-v2"));

    assert_eq!(baseline.namespace_key, changed_soul.namespace_key);
    assert_eq!(baseline.namespace_key, changed_agents.namespace_key);
    assert_eq!(baseline.namespace_key, changed_workspace.namespace_key);
    assert_eq!(baseline.stable_prefix_hash, changed_soul.stable_prefix_hash);
    assert_eq!(
        baseline.stable_prefix_hash,
        changed_agents.stable_prefix_hash
    );
    assert_eq!(
        baseline.stable_prefix_hash,
        changed_workspace.stable_prefix_hash
    );
    assert_eq!(
        baseline.session_stable_context_hash,
        changed_workspace.session_stable_context_hash
    );

    let kinds = baseline
        .segments
        .iter()
        .map(|segment| segment.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            PromptCacheSegmentKind::System,
            PromptCacheSegmentKind::Soul,
            PromptCacheSegmentKind::UserAgents,
            PromptCacheSegmentKind::ModelProfile,
        ]
    );
    for (changed, kind) in [
        (&changed_soul, PromptCacheSegmentKind::Soul),
        (&changed_agents, PromptCacheSegmentKind::UserAgents),
    ] {
        let before = baseline
            .segments
            .iter()
            .find(|segment| segment.kind == kind)
            .unwrap();
        let after = changed
            .segments
            .iter()
            .find(|segment| segment.kind == kind)
            .unwrap();
        assert_ne!(before.hash, after.hash);
        assert_ne!(
            baseline.session_stable_context_hash,
            changed.session_stable_context_hash
        );
    }
}

fn provider_prompt_cache_plan(messages: Vec<AgentModelMessage>) -> ProviderPromptCachePlan {
    provider_prompt_cache_plan_for(
        "test-provider",
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI),
        PromptCacheProviderWireFormat::OpenAiResponses,
        messages,
        Vec::new(),
    )
}

fn provider_prompt_cache_plan_for(
    provider_name: &str,
    capabilities: axon_config::ProviderCapabilities,
    format: PromptCacheProviderWireFormat,
    messages: Vec<AgentModelMessage>,
    model_tools: Vec<ModelToolDefinition>,
) -> ProviderPromptCachePlan {
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: provider_name.to_owned(),
            capabilities,
            format,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: messages.into(),
            model_tools,
            context_section_count: 1,
        })
        .unwrap()
}

fn model_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "builtin.shell_command".to_owned(),
        description: "执行本机 shell 命令，需要用户审批后执行。".to_owned(),
        schema_hash: "tool-schema-shell-command".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

/// G6：一轮预热 + 三轮编译，稳定前缀哈希必须逐轮一致。
#[test]
fn stable_prefix_hash_is_identical_after_warmup_and_three_rounds() {
    let build = |question: &str| {
        provider_prompt_cache_plan(vec![
            AgentModelMessage::text("system", "稳定系统指令"),
            AgentModelMessage::text("developer", "稳定开发者约束"),
            AgentModelMessage::text("user", question),
        ])
    };
    let _warmup = build("预热");
    let rounds = [build("第一轮"), build("第二轮"), build("第三轮")];
    for round in &rounds {
        assert_eq!(
            round.stable_prefix_hash, rounds[0].stable_prefix_hash,
            "稳定前缀哈希跨轮必须一致"
        );
        assert_eq!(round.namespace_key, rounds[0].namespace_key);
    }
}
