use super::*;
use crate::AgentModelToolCallMetadata;

#[test]
fn valid_history_uses_borrowed_fast_path() {
    let messages = vec![call_message("call-1"), result_message("call-1", "ok", true)];

    assert!(matches!(
        normalize_tool_history(&messages),
        Cow::Borrowed(_)
    ));
}

#[test]
fn provider_normalize_uses_kind_for_checkpoint_deduplication() {
    let messages = [
        AgentModelMessage::text("user", "<conversation-checkpoint> user fact"),
        AgentModelMessage::conversation_checkpoint("old checkpoint"),
        AgentModelMessage::conversation_checkpoint("latest checkpoint"),
    ];
    let normalized = normalize_provider_history(&messages, true);

    assert_eq!(normalized.messages.len(), 2);
    assert!(normalized.messages[0].content.contains("user fact"));
    assert_eq!(
        normalized.messages[1].content,
        AgentModelMessage::conversation_checkpoint("latest checkpoint").content
    );
    assert!(
        normalized
            .reasons
            .contains(&PROVIDER_HISTORY_REASON_CHECKPOINT_DEDUPLICATED.to_owned())
    );
}

#[test]
fn provider_normalize_canonicalizes_roles_and_tool_metadata() {
    let messages = [
        AgentModelMessage::text(" USER ", "请求"),
        AgentModelMessage::text(" user ", "").with_tool_calls(vec![AgentModelToolCallMetadata {
            id: " call-1 ".to_owned(),
            name: " builtin.search ".to_owned(),
            input: "{}".into(),
        }]),
        AgentModelMessage::tool_result(" call-1 ", "完成", true),
    ];
    let normalized = normalize_provider_history(&messages, true);

    assert_eq!(normalized.messages[0].role, ROLE_USER);
    assert_eq!(normalized.messages[1].role, ROLE_ASSISTANT);
    assert_eq!(tool_calls(&normalized.messages[1])[0].id, "call-1");
    assert_eq!(
        tool_calls(&normalized.messages[1])[0].name,
        "builtin.search"
    );
    assert_eq!(normalized.messages[2].role, ROLE_TOOL);
    assert_eq!(
        tool_result(&normalized.messages[2]).unwrap().call_id,
        "call-1"
    );
    assert_eq!(
        normalized.messages[2].source_ref.as_deref(),
        Some("tool-call://call-1")
    );
}

#[test]
fn provider_normalize_preserves_archived_tool_result_source_ref() {
    let mut result = AgentModelMessage::tool_result("call-1", "完成", true)
        .with_source_ref("tool://builtin.shell/archive");
    result.retention_class = crate::RetentionClass::P1State;
    let messages = [call_message("call-1"), result];
    let normalized = normalize_provider_history(&messages, true);

    assert_eq!(
        normalized.messages[1].retention_class,
        crate::RetentionClass::P2Evidence
    );
    assert_eq!(
        normalized.messages[1].source_ref.as_deref(),
        Some("tool://builtin.shell/archive")
    );
}

#[test]
fn provider_normalize_splits_conflicting_tool_metadata_without_loss() {
    let mut message = call_message("call-1").with_source_ref("tool-call://legacy-call");
    message
        .provider_metadata
        .as_mut()
        .expect("tool call metadata")
        .tool_result = Some(AgentModelToolResultMetadata {
        call_id: "call-1".to_owned(),
        output: "完成".to_owned(),
        success: true,
    });

    let messages = [message];
    let normalized = normalize_provider_history(&messages, true);
    assert_eq!(normalized.messages.len(), 2);
    assert!(has_tool_calls(&normalized.messages[0]));
    assert!(has_tool_result(&normalized.messages[1]));
    assert_eq!(
        normalized.messages[0].retention_class,
        crate::RetentionClass::P1State
    );
    assert_eq!(
        normalized.messages[1].retention_class,
        crate::RetentionClass::P2Evidence
    );
    assert!(normalized.messages[0].source_ref.is_none());
    assert_eq!(
        normalized.messages[1].source_ref.as_deref(),
        Some("tool-call://call-1")
    );
    assert!(
        normalized
            .reasons
            .contains(&PROVIDER_HISTORY_REASON_TOOL_METADATA_SPLIT.to_owned())
    );
}

#[test]
fn provider_normalize_clears_empty_metadata_without_emitting_tool_role() {
    let mut message = AgentModelMessage::text("tool", "保留事实");
    message.provider_metadata = Some(crate::AgentModelMessageProviderMetadata {
        tool_calls: Some(Vec::new()),
        anthropic_thinking_blocks: Some(Vec::new()),
        ..Default::default()
    });

    let messages = [message];
    let normalized = normalize_provider_history(&messages, true);
    assert_eq!(normalized.messages[0].role, ROLE_USER);
    assert!(normalized.messages[0].provider_metadata.is_none());
    assert!(
        normalized
            .reasons
            .contains(&PROVIDER_HISTORY_REASON_METADATA_NORMALIZED.to_owned())
    );
}

#[test]
fn missing_result_gets_stable_aborted_output() {
    let normalized = normalize_tool_history(&[call_message("call-1")]).into_owned();
    let result = tool_result(&normalized[1]).expect("缺失 output 应被补齐");

    assert_eq!(result.call_id, "call-1");
    assert_eq!(result.output, ABORTED_TOOL_OUTPUT);
    assert!(!result.success);
}

#[test]
fn duplicate_and_orphan_items_are_removed() {
    let normalized = normalize_tool_history(&[
        call_message("call-1"),
        call_message("call-1"),
        result_message("orphan", "ignored", true),
        result_message("call-1", "first", true),
        result_message("call-1", "duplicate", true),
    ])
    .into_owned();

    assert_eq!(normalized.len(), 2);
    assert_eq!(tool_calls(&normalized[0]).len(), 1);
    assert_eq!(tool_result(&normalized[1]).unwrap().output, "first");
}

#[test]
fn result_before_call_is_replaced_with_aborted_output_after_call() {
    let normalized = normalize_tool_history(&[
        result_message("call-1", "invalid", true),
        call_message("call-1"),
    ])
    .into_owned();

    assert_eq!(normalized.len(), 2);
    assert_eq!(tool_calls(&normalized[0])[0].id, "call-1");
    let result = tool_result(&normalized[1]).expect("call 后必须补齐 output");
    assert_eq!(result.output, ABORTED_TOOL_OUTPUT);
    assert!(!result.success);
}

#[test]
fn oldest_tool_unit_removal_preserves_interleaved_messages() {
    let mut messages = vec![
        call_message("call-1"),
        AgentModelMessage::text("user", "独立消息"),
        result_message("call-1", "ok", true),
        AgentModelMessage::text("assistant", "继续"),
    ];

    assert!(remove_oldest_model_history_unit(&mut messages));

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "独立消息");
    assert_eq!(messages[1].content, "继续");
}

fn call_message(call_id: &str) -> AgentModelMessage {
    AgentModelMessage::text("assistant", "").with_tool_calls(vec![AgentModelToolCallMetadata {
        id: call_id.to_owned(),
        name: "builtin.glob".to_owned(),
        input: "{}".into(),
    }])
}

fn result_message(call_id: &str, output: &str, success: bool) -> AgentModelMessage {
    AgentModelMessage::tool_result(call_id, output, success)
}
