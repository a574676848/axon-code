//! 压缩 Provider 计量回归测试。

use crate::insert_then_compress::{
    CompressionPolicy, WindowRolloverPolicyRuntime,
    estimated_history_tokens_for_provider_with_capabilities,
};
use crate::{
    AgentModelMessage, AgentModelToolCallMetadata, CanonicalCheckpoint,
    PromptCacheProviderWireFormat,
};

fn policy(trigger_tokens: u64) -> CompressionPolicy {
    CompressionPolicy {
        trigger_tokens,
        context_window: trigger_tokens.saturating_mul(2),
    }
}

fn canonical_checkpoint(messages: &[AgentModelMessage], task: &str) -> CanonicalCheckpoint {
    let summary = format!(
        "P0:\nTASK:\n{task}\nCONSTRAINTS:\n保持已知约束\nVERIFIED:\n已完成已知验证\nOPEN:\n继续下一步\nTOOLS:\n- none"
    );
    CanonicalCheckpoint::parse(&summary, messages).expect("无活动工具调用的 checkpoint 应通过门禁")
}

#[test]
fn rebuild_after_tokens_matches_provider_estimator_for_tool_tail() {
    let runtime = WindowRolloverPolicyRuntime::new(policy(10_000)).with_provider_wire_capabilities(
        PromptCacheProviderWireFormat::AnthropicMessages,
        Vec::new(),
        false,
    );
    let messages = vec![
        AgentModelMessage::context("system", "sys"),
        AgentModelMessage::text("user", "old"),
        AgentModelMessage::text("user", "latest"),
        AgentModelMessage::text("assistant", "calling").with_tool_calls(vec![
            AgentModelToolCallMetadata {
                id: "call-1".to_owned(),
                name: "search".to_owned(),
                input: serde_json::json!({"q": "axon"}).into(),
            },
        ]),
        AgentModelMessage::tool_result("call-1", "result", true),
    ];
    let checkpoint = canonical_checkpoint(&messages, "summary");

    let rebuilt = runtime.rebuild_with_compression(&messages, &checkpoint);
    let expected = estimated_history_tokens_for_provider_with_capabilities(
        &rebuilt.messages,
        PromptCacheProviderWireFormat::AnthropicMessages,
        &[],
        false,
    );

    assert_eq!(rebuilt.after_tokens, expected);
}
