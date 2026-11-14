//! 交接摘要合同测试：内容自由、只守预算，格式不得阻断换窗。

use axon_domain::TurnId;

use crate::insert_then_compress::{CompressionOutcome, WindowRolloverPolicyRuntime};
use crate::{
    AgentModelMessage, AgentModelToolCallMetadata, CanonicalCheckpoint, CompressionPolicy,
    WindowRolloverRuntime,
};

fn base_messages() -> Vec<AgentModelMessage> {
    vec![
        AgentModelMessage::context("system", "sys"),
        AgentModelMessage::text("user", "完成当前任务"),
        AgentModelMessage::text("assistant", "正在处理".repeat(10000)),
    ]
}

fn compress(
    messages: Vec<AgentModelMessage>,
    summary: String,
) -> (CompressionOutcome, Vec<AgentModelMessage>, String) {
    let mut rollover = move |history: &[AgentModelMessage]| {
        CanonicalCheckpoint::parse(&summary, history)
            .map_err(axon_domain::AxonError::runtime_failure)
    };
    let mut compression = WindowRolloverRuntime {
        runtime: WindowRolloverPolicyRuntime::new(CompressionPolicy {
            trigger_tokens: 40_000,
            context_window: 50_000,
        }),
        rollover: &mut rollover,
        last_projection_metrics: None,
    };
    let mut messages = messages.into();
    let mut events = Vec::new();
    let outcome =
        compression.maybe_compress_requested(&mut messages, Some(40_000), TurnId(1), &mut events);
    let failure = events.last().expect("换窗应记录终态事件").summary.clone();
    (outcome, messages.into_messages(), failure)
}

/// 自由格式摘要必须直接放行：六段标题不是合同。
#[test]
fn checkpoint_accepts_free_form_summary() {
    let original = base_messages();
    let summary = "本轮把换窗门禁改成只看预算，正文随便写，结论已落盘。";

    let (outcome, messages, failure) = compress(original.clone(), summary.to_owned());

    assert_eq!(outcome, CompressionOutcome::Compressed, "{failure}");
    assert!(
        messages.len() <= original.len(),
        "压缩后不得比原窗口更长：{} vs {}",
        messages.len(),
        original.len()
    );
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("本轮把换窗门禁改成只看预算")),
        "摘要正文必须原样进入新窗口"
    );
}

/// 空摘要仍然拒绝：没有任何交接内容时不能静默丢弃整段历史。
#[test]
fn checkpoint_rejects_empty_summary() {
    let original = base_messages();

    let (outcome, messages, failure) = compress(original.clone(), "   ".to_owned());

    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert!(&messages[..original.len()] == original.as_slice());
    assert!(failure.contains("内容为空"), "{failure}");
}

/// 超预算摘要仍然拒绝换窗：新窗口一开就满额等于没换窗。
#[test]
fn checkpoint_rejects_summary_over_budget() {
    let original = base_messages();
    let oversized = "长".repeat(20_000);

    let (outcome, messages, failure) = compress(original.clone(), oversized);

    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert!(&messages[..original.len()] == original.as_slice());
    assert!(failure.contains("超过预算"), "{failure}");
}

/// 悬空 tool call 不再要求摘要列出 ID：一致性由历史重建处理，不由自由文本兜底。
#[test]
fn checkpoint_accepts_unsettled_tool_call_without_listing_ids() {
    let mut messages = base_messages();
    messages[2].retention_class = crate::RetentionClass::P3Derivable;
    let mut pending = AgentModelMessage::text("assistant", "").with_tool_calls(vec![
        AgentModelToolCallMetadata {
            id: "pending-call".to_owned(),
            name: "builtin.glob".to_owned(),
            input: "{}".into(),
        },
    ]);
    pending.retention_class = crate::RetentionClass::P3Derivable;
    messages.push(pending);
    messages.push(AgentModelMessage::text("assistant", "尾部证据"));

    let (outcome, _, failure) = compress(messages, "只写自由文本，不列 call ID。".to_owned());

    assert_eq!(outcome, CompressionOutcome::Compressed, "{failure}");
}
