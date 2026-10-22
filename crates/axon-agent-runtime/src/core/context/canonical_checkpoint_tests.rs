//! Canonical checkpoint 完整性门禁测试。

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

fn checkpoint_summary(tools: &str) -> String {
    format!(
        "P0:\nTASK:\n完成当前任务\nCONSTRAINTS:\n保留用户约束\nVERIFIED:\n已完成基础验证\nOPEN:\n继续下一步\nTOOLS:\n{tools}"
    )
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

#[test]
fn checkpoint_missing_or_empty_required_field_keeps_original_history() {
    let original = base_messages();
    let missing_field_summary =
        "P0:\nTASK:\n完成当前任务\nCONSTRAINTS:\n保留约束\nOPEN:\n继续\nTOOLS:\n- none";
    let (outcome, messages, failure) = compress(original.clone(), missing_field_summary.to_owned());
    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert!(&messages[..original.len()] == original.as_slice());
    assert!(messages.len() > original.len());
    assert!(failure.contains("VERIFIED:"));

    let empty_open_summary = "P0:\nTASK:\n完成当前任务\nCONSTRAINTS:\n保留约束\nVERIFIED:\n已验证\nOPEN:\nTOOLS:\n- none";
    let (outcome, messages, failure) = compress(original.clone(), empty_open_summary.to_owned());
    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert!(&messages[..original.len()] == original.as_slice());
    assert!(messages.len() > original.len());
    assert!(failure.contains("OPEN:"));
}

#[test]
fn canonical_checkpoint_requires_each_unsettled_tool_call_id() {
    let mut original = base_messages();
    original.push(
        AgentModelMessage::text("assistant", "").with_tool_calls(vec![
            AgentModelToolCallMetadata {
                id: "pending-call".to_owned(),
                name: "builtin.glob".to_owned(),
                input: "{}".into(),
            },
        ]),
    );

    let error = CanonicalCheckpoint::parse(&checkpoint_summary("- none"), &original)
        .expect_err("原始历史中的未配对 tool call 必须要求 checkpoint ID");
    assert!(error.contains("pending-call"));
}

#[test]
fn checkpoint_accepts_each_unsettled_tool_call_id() {
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

    let (outcome, compressed, _) = compress(messages, checkpoint_summary("- none"));

    assert_eq!(outcome, CompressionOutcome::Compressed);
    assert!(compressed.iter().all(|message| {
        message
            .provider_metadata
            .as_ref()
            .and_then(|metadata| metadata.tool_result.as_ref())
            .is_none()
    }));
}

#[test]
fn checkpoint_rejects_stale_tool_id_after_normalization() {
    let mut original = base_messages();
    original.push(
        AgentModelMessage::text("assistant", "").with_tool_calls(vec![
            AgentModelToolCallMetadata {
                id: "pending-call".to_owned(),
                name: "builtin.glob".to_owned(),
                input: "{}".into(),
            },
        ]),
    );

    let (outcome, messages, failure) =
        compress(original.clone(), checkpoint_summary("- ID: pending-call"));

    assert_eq!(outcome, CompressionOutcome::Prompted);
    assert!(&messages[..original.len()] == original.as_slice());
    assert!(messages.len() > original.len());
    assert!(failure.contains("TOOLS"));
}

#[test]
fn checkpoint_uses_normalized_history_for_unsettled_tool_call() {
    let mut original = base_messages();
    original.push(
        AgentModelMessage::text("assistant", "").with_tool_calls(vec![
            AgentModelToolCallMetadata {
                id: "pending-call".to_owned(),
                name: "builtin.glob".to_owned(),
                input: "{}".into(),
            },
        ]),
    );

    let (outcome, compressed, failure) = compress(original, checkpoint_summary("- none"));

    assert_eq!(outcome, CompressionOutcome::Compressed);
    assert!(failure.contains("记忆窗口切换完成"));
    assert!(compressed.iter().all(|message| {
        message
            .provider_metadata
            .as_ref()
            .and_then(|metadata| metadata.tool_result.as_ref())
            .is_none()
    }));
}
