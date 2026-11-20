//! 压缩 Provider 计量回归测试。

use crate::insert_then_compress::{
    CompressionOutcome, CompressionPolicy, WindowRolloverPolicyRuntime,
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

/// 自动换窗连续失败必须退避：同一读数不得逐轮重试。
///
/// 失败原因通常在同一上下文读数上持续存在，逐轮重试会让每轮都付一次整份语料的
/// 归档与回滚成本；而读数单调增长，退避线最终一定会被越过，自救不会永久放弃。
#[test]
fn repeated_auto_rollover_failures_back_off_until_context_grows() {
    let mut runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: 1_000_000,
        context_window: 2_000_000,
    });
    let messages = vec![
        AgentModelMessage::text("user", "目标"),
        AgentModelMessage::text("assistant", "进展"),
    ];
    // 强制线是 trigger 加回退缓冲，这里取一个确定越线的读数。
    let failed_at = 1_100_000_u64;
    assert!(
        runtime.should_compress(&messages, Some(failed_at)),
        "越过强制线应触发自动换窗"
    );

    runtime.note_auto_rollover_failure(failed_at);
    assert!(
        !runtime.should_compress(&messages, Some(failed_at)),
        "同一读数不得立即重试"
    );
    assert!(
        !runtime.should_compress(&messages, Some(failed_at + 4_095)),
        "未涨过一个退避步长仍不重试"
    );
    assert!(
        runtime.should_compress(&messages, Some(failed_at + 4_096)),
        "涨过退避线后必须再试"
    );
}

/// 换窗成功后失败计数清零，新窗口不受旧窗口的退避影响。
#[test]
fn successful_rollover_clears_failure_backoff() {
    let mut runtime = WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: 1_000_000,
        context_window: 2_000_000,
    });
    let messages = vec![
        AgentModelMessage::text("user", "目标"),
        AgentModelMessage::text("assistant", "进展"),
    ];
    let failed_at = 1_100_000_u64;
    runtime.note_auto_rollover_failure(failed_at);
    assert!(!runtime.should_compress(&messages, Some(failed_at)));

    runtime.record_outcome(CompressionOutcome::Compressed);
    assert!(
        runtime.should_compress(&messages, Some(failed_at)),
        "换窗成功后失败计数必须清零"
    );
}
