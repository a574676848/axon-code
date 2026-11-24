//! `agent_model_message` 的消息合同与历史快照边界测试。

use super::*;
use crate::AgentRuntimeNoteKind;

#[test]
fn provider_file_preview_is_bounded_on_utf8_boundary() {
    let preview = "界".repeat(MAX_PROVIDER_FILE_PREVIEW_BYTES);
    let bounded = bound_provider_file_preview(&preview);
    assert!(bounded.len() <= MAX_PROVIDER_FILE_PREVIEW_BYTES);
    assert!(bounded.is_char_boundary(bounded.len()));
}

#[test]
fn shared_history_snapshot_clones_storage_until_mutation() {
    let original = AgentModelHistorySnapshot::from(vec![AgentModelMessage::text("user", "任务")]);
    let mut mutated = original.clone();

    assert!(original.shares_storage_with(&mutated));
    mutated
        .make_mut()
        .push(AgentModelMessage::text("assistant", "结果"));

    assert!(!original.shares_storage_with(&mutated));
    assert_eq!(original.len(), 1);
    assert_eq!(mutated.into_messages().len(), 2);
}

#[test]
fn real_user_turn_boundary_excludes_context_and_control_messages() {
    let regular = AgentModelMessage::text("user", "真实请求");
    let context = AgentModelMessage::context("user", "上下文");
    let mut checkpoint = AgentModelMessage::text("user", "checkpoint");
    checkpoint.kind = AgentModelMessageKind::ConversationCheckpoint;

    assert!(regular.is_real_user_turn());
    assert!(!context.is_real_user_turn());
    assert!(!checkpoint.is_real_user_turn());
}

#[test]
fn dropping_user_turns_preserves_prefix_and_only_copies_on_change() {
    let original = AgentModelHistorySnapshot::from(vec![
        AgentModelMessage::text("system", "系统"),
        AgentModelMessage::text("user", "第一轮"),
        AgentModelMessage::text("assistant", "第一轮结果"),
        AgentModelMessage::text("user", "第二轮"),
        AgentModelMessage::text("assistant", "第二轮结果"),
    ]);
    let mut rolled_back = original.clone();

    assert!(rolled_back.drop_last_n_user_turns(1));
    assert!(!original.shares_storage_with(&rolled_back));
    assert_eq!(
        rolled_back
            .as_slice()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["系统", "第一轮", "第一轮结果"]
    );

    let mut prefix_after_full_rollback = original.clone();
    assert!(prefix_after_full_rollback.drop_last_n_user_turns(usize::MAX));
    assert_eq!(
        prefix_after_full_rollback
            .as_slice()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        vec!["系统"]
    );

    let prefix_only =
        AgentModelHistorySnapshot::from(vec![AgentModelMessage::text("system", "系统")]);
    let mut unchanged = prefix_only.clone();
    assert!(!unchanged.drop_last_n_user_turns(1));
    assert!(prefix_only.shares_storage_with(&unchanged));
}

#[test]
fn plain_text_message_is_conversation_regular() {
    let message = AgentModelMessage::text("user", "当前请求");

    assert_eq!(message.source, AgentModelMessageSource::Conversation);
    assert_eq!(message.kind, AgentModelMessageKind::Regular);
    assert!(!message.cache_excluded);
    assert!(message.is_real_user_turn());
}

#[test]
fn context_helper_marks_source_but_keeps_regular_kind() {
    let message = AgentModelMessage::context("developer", "<runtime-constraints/>");

    assert_eq!(message.source, AgentModelMessageSource::Context);
    assert_eq!(message.kind, AgentModelMessageKind::Regular);
    assert!(!message.cache_excluded);
    assert!(!message.is_real_user_turn());
}

#[test]
fn cache_excluded_does_not_mark_context_source() {
    let message = AgentModelMessage::cache_excluded("developer", "<environment-context/>");

    assert!(message.cache_excluded);
    assert_eq!(message.source, AgentModelMessageSource::Conversation);
    assert_eq!(message.kind, AgentModelMessageKind::Regular);
}

#[test]
fn runtime_note_is_user_role_but_not_real_user_turn() {
    let message =
        AgentModelMessage::runtime_note(AgentRuntimeNoteKind::PayloadBudgetGate, "预算提醒");

    assert_eq!(message.role, "user");
    assert_eq!(
        message.kind,
        AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::PayloadBudgetGate)
    );
    assert!(!message.is_real_user_turn());
}

#[test]
fn conversation_checkpoint_is_user_role_but_not_real_user_turn() {
    let message = AgentModelMessage::conversation_checkpoint("交接摘要");

    assert_eq!(message.role, "user");
    assert_eq!(message.kind, AgentModelMessageKind::ConversationCheckpoint);
    assert!(!message.is_real_user_turn());
}

#[test]
fn assistant_and_tool_messages_are_not_real_user_turns() {
    assert!(!AgentModelMessage::text("assistant", "答复").is_real_user_turn());
    assert!(!AgentModelMessage::text("tool", "结果").is_real_user_turn());
    assert!(!AgentModelMessage::text("system", "规则").is_real_user_turn());
}

/// 五类构造器的三维取值必须与契约一一对应：来源、语义类别、缓存策略各管一维。
#[test]
fn constructor_matrix_pins_source_kind_and_cache_flag() {
    let regular = AgentModelMessage::text("user", "真实用户请求");
    assert_eq!(regular.source, AgentModelMessageSource::Conversation);
    assert_eq!(regular.kind, AgentModelMessageKind::Regular);
    assert!(!regular.cache_excluded);
    assert!(regular.is_real_user_turn());

    let stable = AgentModelMessage::context("developer", "<user-agents-context/>");
    assert_eq!(stable.source, AgentModelMessageSource::Context);
    assert_eq!(stable.kind, AgentModelMessageKind::Regular);
    assert!(!stable.cache_excluded);
    assert_eq!(stable.retention_class, crate::RetentionClass::P1State);

    let injected = AgentModelMessage::injected_context("developer", "<environment-context/>");
    assert_eq!(injected.source, AgentModelMessageSource::Context);
    assert_eq!(injected.kind, AgentModelMessageKind::InjectedContext);
    assert!(injected.cache_excluded);
    assert_eq!(injected.retention_class, crate::RetentionClass::P3Derivable);

    let note = AgentModelMessage::runtime_note(AgentRuntimeNoteKind::PayloadBudgetGate, "提醒");
    assert_eq!(note.source, AgentModelMessageSource::Conversation);
    assert!(matches!(note.kind, AgentModelMessageKind::RuntimeNote(_)));
    assert!(note.cache_excluded);

    let checkpoint = AgentModelMessage::conversation_checkpoint("P0:\nTASK:\n摘要");
    assert_eq!(
        checkpoint.kind,
        AgentModelMessageKind::ConversationCheckpoint
    );
    assert!(checkpoint.cache_excluded);
}

/// 注入上下文即使 role=user 也不是用户发言：判定只认 source + kind。
#[test]
fn injected_context_never_counts_as_real_user_turn() {
    for role in ["user", "developer", "system", "assistant", "tool"] {
        let message = AgentModelMessage::injected_context(role, "注入内容");
        assert!(
            !message.is_real_user_turn(),
            "role={role} 的注入内容不得被当成用户请求"
        );
    }
}
