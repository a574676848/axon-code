use super::{canonical_checkpoint, policy};
use crate::insert_then_compress::WindowRolloverPolicyRuntime;
use crate::{AgentModelMessage, AgentModelMessageKind};

#[test]
fn compaction_preserves_stable_prefix_and_reloads_active_turn_context() {
    let mut runtime = WindowRolloverPolicyRuntime::new(policy(4_000));
    let messages = vec![
        AgentModelMessage::context("system", "稳定系统"),
        AgentModelMessage::context(
            "system",
            "<assistant-profile-context>soul</assistant-profile-context>",
        ),
        AgentModelMessage::context(
            "developer",
            "<user-agents-context>global rules</user-agents-context>",
        ),
        AgentModelMessage::context(
            "developer",
            "# AGENTS.md instructions\n<INSTRUCTIONS>workspace rules</INSTRUCTIONS>",
        ),
        AgentModelMessage::context(
            "developer",
            "<agent-context-snapshot>ledger=task memory=memory result=result</agent-context-snapshot>",
        ),
        AgentModelMessage::text("user", "当前任务"),
        AgentModelMessage::text("assistant", "处理中"),
    ];
    runtime.refresh_fresh_window_context(&messages);

    let rebuilt =
        runtime.rebuild_with_compression(&messages, &canonical_checkpoint(&messages, "轮转摘要"));

    let prefix = rebuilt
        .messages
        .iter()
        .take(3)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        prefix,
        vec![
            "稳定系统",
            "<assistant-profile-context>soul</assistant-profile-context>",
            "<user-agents-context>global rules</user-agents-context>",
        ]
    );
    assert!(
        rebuilt
            .messages
            .iter()
            .any(|message| { message.content.contains("# AGENTS.md instructions") })
    );
    assert!(
        rebuilt
            .messages
            .iter()
            .any(|message| message.content.contains("ledger=task"))
    );
    let checkpoint_index = rebuilt
        .messages
        .iter()
        .position(|message| message.kind == AgentModelMessageKind::ConversationCheckpoint)
        .expect("新窗口应包含 checkpoint");
    let workspace_context_index = rebuilt
        .messages
        .iter()
        .position(|message| message.content.contains("# AGENTS.md instructions"))
        .expect("active turn Workspace 上下文应被重装");
    assert!(checkpoint_index < workspace_context_index);
    assert_eq!(
        rebuilt
            .messages
            .last()
            .map(|message| message.content.as_str()),
        Some("当前任务")
    );
}
