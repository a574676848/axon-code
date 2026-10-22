use super::*;
use crate::{AgentModelMessage, ProviderVisibleCacheBreakReason, ProviderVisibleCacheShape};

#[test]
fn provider_wire_keeps_global_prefix_and_puts_workspace_context_after_history() {
    let global_stable_messages = [
        AgentModelMessage::text("system", "SYSTEM"),
        AgentModelMessage::text(
            "system",
            "<assistant-profile-context>SOUL</assistant-profile-context>",
        ),
        AgentModelMessage::text(
            "developer",
            "<user-agents-context>USER_AGENTS</user-agents-context>",
        ),
        AgentModelMessage::text(
            "developer",
            "<personal-memory-context>PERSONAL_MEMORY</personal-memory-context>",
        ),
    ];
    let expected_dynamic_texts = [
        "<user-agents-context>USER_AGENTS</user-agents-context>",
        "<personal-memory-context>PERSONAL_MEMORY</personal-memory-context>",
        "HISTORY_USER",
        "HISTORY_ASSISTANT",
        "# AGENTS.md instructions\n\n<INSTRUCTIONS>WORKSPACE</INSTRUCTIONS>",
        "<available-skills-context>SKILLS</available-skills-context>",
        "<agent-context-snapshot>DYNAMIC</agent-context-snapshot>",
    ];

    for protocol in [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        let mut input = request_input(Vec::new());
        input.protocol = protocol;
        input.messages = global_stable_messages
            .iter()
            .cloned()
            .chain([
                AgentModelMessage::text("user", "HISTORY_USER"),
                AgentModelMessage::text("assistant", "HISTORY_ASSISTANT"),
                AgentModelMessage::cache_excluded(
                    "developer",
                    "# AGENTS.md instructions\n\n<INSTRUCTIONS>WORKSPACE</INSTRUCTIONS>",
                ),
                AgentModelMessage::cache_excluded(
                    "developer",
                    "<available-skills-context>SKILLS</available-skills-context>",
                ),
                AgentModelMessage::cache_excluded(
                    "developer",
                    "<agent-context-snapshot>DYNAMIC</agent-context-snapshot>",
                ),
            ])
            .collect::<Vec<_>>()
            .into();
        refresh_prompt_cache_plan(&mut input);
        let plan = ProviderStreamingHttpRequestRuntime
            .plan(input)
            .expect("三种 provider wire 都应生成请求");
        let body: serde_json::Value =
            serde_json::from_str(&plan.body_text).expect("请求 body 应为 JSON");

        let actual = provider_visible_texts(protocol, &body);
        match protocol {
            ProviderStreamProtocol::OpenAiResponses => {
                assert_eq!(
                    actual.first().copied(),
                    Some("SYSTEM\n\n<assistant-profile-context>SOUL</assistant-profile-context>")
                );
                assert_eq!(&actual[1..], expected_dynamic_texts);
            }
            ProviderStreamProtocol::OpenAiCompatible
            | ProviderStreamProtocol::AnthropicMessages => {
                let expected = [
                    "SYSTEM",
                    "<assistant-profile-context>SOUL</assistant-profile-context>",
                    expected_dynamic_texts[0],
                    expected_dynamic_texts[1],
                    expected_dynamic_texts[2],
                    expected_dynamic_texts[3],
                    expected_dynamic_texts[4],
                    expected_dynamic_texts[5],
                    expected_dynamic_texts[6],
                ];
                assert_eq!(actual, expected);
            }
        }
    }
}

#[test]
fn provider_cache_shape_marks_appended_turn_without_changing_stable_prefix() {
    for protocol in [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        let stable_messages = [
            AgentModelMessage::text("system", "SYSTEM"),
            AgentModelMessage::text(
                "system",
                "<assistant-profile-context>SOUL</assistant-profile-context>",
            ),
            AgentModelMessage::text(
                "developer",
                "<user-agents-context>USER_AGENTS</user-agents-context>",
            ),
        ];
        let mut first = request_input(Vec::new());
        first.protocol = protocol;
        first.messages = stable_messages
            .iter()
            .cloned()
            .chain([
                AgentModelMessage::text("user", "HISTORY_USER_1"),
                AgentModelMessage::text("assistant", "HISTORY_ASSISTANT_1"),
            ])
            .collect::<Vec<_>>()
            .into();
        refresh_prompt_cache_plan(&mut first);
        let first_plan = ProviderStreamingHttpRequestRuntime
            .plan(first.clone())
            .expect("首轮请求应生成");
        let first_shape =
            ProviderVisibleCacheShape::from_serialized_request(protocol, &first_plan.body_text)
                .unwrap();

        first.messages = stable_messages
            .iter()
            .cloned()
            .chain([
                AgentModelMessage::text("user", "HISTORY_USER_1"),
                AgentModelMessage::text("assistant", "HISTORY_ASSISTANT_1"),
                AgentModelMessage::text("user", "HISTORY_USER_2"),
                AgentModelMessage::text("assistant", "HISTORY_ASSISTANT_2"),
            ])
            .collect::<Vec<_>>()
            .into();
        refresh_prompt_cache_plan(&mut first);
        let second_plan = ProviderStreamingHttpRequestRuntime
            .plan(first)
            .expect("追加 turn 后请求应生成");
        let second_shape =
            ProviderVisibleCacheShape::from_serialized_request(protocol, &second_plan.body_text)
                .unwrap();

        assert_eq!(
            second_shape.canonical_prefix_hash,
            first_shape.canonical_prefix_hash
        );
        let comparison = second_shape.compare(Some(&first_shape));
        if protocol != ProviderStreamProtocol::AnthropicMessages {
            assert_eq!(
                comparison.reason,
                ProviderVisibleCacheBreakReason::HistoryAppended
            );
            assert_eq!(
                comparison.common_prefix_messages,
                first_shape.message_hashes.len()
            );
        }
    }
}

#[test]
fn provider_cache_shape_is_equivalent_across_dynamic_entry_variants() {
    let build_shape = |history: &[(&str, &str)]| {
        let mut input = request_input(Vec::new());
        input.protocol = ProviderStreamProtocol::OpenAiCompatible;
        input.messages = [
            AgentModelMessage::text("system", "SYSTEM"),
            AgentModelMessage::text(
                "system",
                "<assistant-profile-context>SOUL</assistant-profile-context>",
            ),
            AgentModelMessage::text(
                "developer",
                "<user-agents-context>USER_AGENTS</user-agents-context>",
            ),
        ]
        .into_iter()
        .chain(history.iter().flat_map(|(user, assistant)| {
            [
                AgentModelMessage::text("user", *user),
                AgentModelMessage::text("assistant", *assistant),
            ]
        }))
        .collect::<Vec<_>>()
        .into();
        refresh_prompt_cache_plan(&mut input);
        let plan = ProviderStreamingHttpRequestRuntime
            .plan(input)
            .expect("入口请求应生成");
        ProviderVisibleCacheShape::from_serialized_request(
            ProviderStreamProtocol::OpenAiCompatible,
            &plan.body_text,
        )
        .expect("provider body 应可提取缓存形状")
    };

    let first = build_shape(&[("入口 A 用户问题", "入口 A 助手回答")]);
    let second = build_shape(&[
        ("入口 B 用户问题", "入口 B 助手回答"),
        ("入口 B 后续问题", "入口 B 后续回答"),
    ]);

    assert_eq!(first.canonical_prefix_hash, second.canonical_prefix_hash);
    assert_ne!(first.prompt_hash, second.prompt_hash);
    let comparison = second.compare(Some(&first));
    assert_eq!(
        comparison.reason,
        ProviderVisibleCacheBreakReason::MessagePrefixChanged
    );
}

#[test]
fn provider_cache_shape_stays_append_only_when_injected_context_is_replayed() {
    // Anthropic 的滚动 tail 断点会按轮改写上一条消息的 cache_control 标记，字节哈希形状
    // 因此记为前缀变化；该标记不进入 token 内容，不改变 Anthropic 的前缀缓存命中。
    for protocol in [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
    ] {
        let head = [
            AgentModelMessage::context("system", "SYSTEM"),
            AgentModelMessage::context(
                "system",
                "<assistant-profile-context>SOUL</assistant-profile-context>",
            ),
            AgentModelMessage::context(
                "developer",
                "<user-agents-context>USER_AGENTS</user-agents-context>",
            ),
        ];
        // 每轮注入块由装配器生成并落库，下一轮按原字节回放。
        let turn_one_injected = AgentModelMessage::cache_excluded(
            "developer",
            "<environment-context>local_time=10:00 shell=pwsh</environment-context>",
        );

        let mut first = request_input(Vec::new());
        first.protocol = protocol;
        first.messages = head
            .iter()
            .cloned()
            .chain([
                turn_one_injected.clone(),
                AgentModelMessage::text("user", "PROMPT_1"),
            ])
            .collect::<Vec<_>>()
            .into();
        refresh_prompt_cache_plan(&mut first);
        let first_protocol = first.protocol;
        let first_plan = ProviderStreamingHttpRequestRuntime
            .plan(first)
            .expect("首轮请求应生成");
        let first_shape = ProviderVisibleCacheShape::from_serialized_request(
            first_protocol,
            &first_plan.body_text,
        )
        .unwrap();

        let mut second = request_input(Vec::new());
        second.protocol = protocol;
        second.messages = head
            .iter()
            .cloned()
            .chain([
                turn_one_injected,
                AgentModelMessage::text("user", "PROMPT_1"),
                AgentModelMessage::text("assistant", "ANSWER_1"),
                AgentModelMessage::cache_excluded(
                    "developer",
                    "<environment-context>local_time=10:01 shell=pwsh</environment-context>",
                ),
                AgentModelMessage::text("user", "PROMPT_2"),
            ])
            .collect::<Vec<_>>()
            .into();
        refresh_prompt_cache_plan(&mut second);
        let second_plan = ProviderStreamingHttpRequestRuntime
            .plan(second)
            .expect("次轮请求应生成");
        let second_shape =
            ProviderVisibleCacheShape::from_serialized_request(protocol, &second_plan.body_text)
                .unwrap();

        let comparison = second_shape.compare(Some(&first_shape));
        assert_eq!(
            comparison.reason,
            ProviderVisibleCacheBreakReason::HistoryAppended,
            "{protocol:?} 回放注入块后必须仍是只追加"
        );
        assert_eq!(
            comparison.common_prefix_messages,
            first_shape.message_hashes.len(),
            "{protocol:?} 上一轮 Provider 可见消息必须全部留在前缀里"
        );
    }
}

fn provider_visible_texts(protocol: ProviderStreamProtocol, body: &serde_json::Value) -> Vec<&str> {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => {
            let mut texts = vec![body["instructions"].as_str().unwrap_or_default()];
            texts.extend(
                body["input"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|item| item["content"].as_array().into_iter().flatten())
                    .filter_map(|part| part["text"].as_str()),
            );
            texts
        }
        ProviderStreamProtocol::OpenAiCompatible => body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|message| message["content"].as_str())
            .collect(),
        ProviderStreamProtocol::AnthropicMessages => {
            let mut texts = body["system"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>();
            texts.extend(
                body["messages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|message| message["content"].as_array().into_iter().flatten())
                    .filter_map(|block| block["text"].as_str()),
            );
            texts
        }
    }
}
