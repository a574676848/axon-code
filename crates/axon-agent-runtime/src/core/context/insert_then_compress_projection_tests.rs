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
        AgentModelMessage::injected_context(
            "developer",
            "# AGENTS.md instructions\n<INSTRUCTIONS>workspace rules</INSTRUCTIONS>",
        ),
        AgentModelMessage::injected_context(
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

/// 换窗边界以 kind 为准：紧邻用户请求的注入带入，遇到 assistant 立即截断。
#[test]
fn fresh_window_keeps_injected_context_and_stops_at_assistant_boundary() {
    let mut runtime = WindowRolloverPolicyRuntime::new(policy(8_000));
    let messages = vec![
        AgentModelMessage::context("system", "稳定系统"),
        AgentModelMessage::injected_context(
            "developer",
            "<environment-context>旧轮环境</environment-context>",
        ),
        AgentModelMessage::text("user", "上一轮问题"),
        AgentModelMessage::text("assistant", "上一轮回答"),
        AgentModelMessage::injected_context(
            "developer",
            "<environment-context>本轮环境</environment-context>",
        ),
        AgentModelMessage::text("user", "本轮问题"),
    ];
    runtime.refresh_fresh_window_context(&messages);

    let rebuilt =
        runtime.rebuild_with_compression(&messages, &canonical_checkpoint(&messages, "轮转摘要"));
    let contents = rebuilt
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();

    assert!(
        contents.iter().any(|content| content.contains("本轮环境")),
        "紧邻用户请求的注入上下文必须随 turn 一起带入：{contents:?}"
    );
    assert!(
        contents.contains(&"本轮问题"),
        "当前用户请求必须在：{contents:?}"
    );
    for leaked in ["上一轮问题", "上一轮回答", "旧轮环境"] {
        assert!(
            !contents.iter().any(|content| content.contains(leaked)),
            "{leaked} 属于更早的 turn，不得被带入新窗口：{contents:?}"
        );
    }
}

/// 端到端：长会话换窗后，仍能回答「换窗前的决定是什么」，且能指向回读来源。
#[test]
fn fresh_window_can_still_answer_the_decision_made_before_rollover() {
    use crate::session_notes::{GoalAnchor, SessionNotes};
    use crate::session_notes_store::NotesStore;

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    // 关键决策写进自由格式工作便签：换窗后要么内联在摘要里，要么能按路径回读。
    let decision = "决定：换窗契约采用模型提交优先、运行时只校验预算";
    NotesStore::write_file(session_dir, "checkpoint.md", decision, 1_000).expect("写入工作便签");
    let notes = SessionNotes::new(GoalAnchor::new("上下文换窗改造", Vec::new()), 1_000);

    let mut runtime = WindowRolloverPolicyRuntime::new(policy(4_000));
    let messages = vec![
        AgentModelMessage::context("system", "稳定系统"),
        AgentModelMessage::context(
            "system",
            "<assistant-profile-context>soul</assistant-profile-context>",
        ),
        AgentModelMessage::text("user", "先看长日志"),
        AgentModelMessage::text("assistant", "x".repeat(4_000)),
        AgentModelMessage::injected_context(
            "developer",
            "<environment-context>shell=pwsh</environment-context>",
        ),
        AgentModelMessage::text("user", "继续，按刚才的决定做"),
    ];
    runtime.refresh_fresh_window_context(&messages);

    let summary = notes
        .try_render_canonical_summary(session_dir, &messages)
        .expect("生成交接摘要");
    assert!(
        summary.contains(decision),
        "短便签正文必须内联进摘要，新窗口才能直接读到决定：{summary}"
    );

    let checkpoint =
        crate::CanonicalCheckpoint::parse(&summary, &messages).expect("摘要必须通过完整性门禁");
    let rebuilt = runtime.rebuild_with_compression(&messages, &checkpoint);
    let window_text = rebuilt
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<String>();

    assert!(
        window_text.contains(decision),
        "新窗口必须能直接回答换窗前的决定：{window_text}"
    );
    assert!(
        window_text.contains("当前窗口交接事实"),
        "新窗口必须带上交接摘要标记：{window_text}"
    );
    assert!(
        !window_text.contains("先看长日志"),
        "更早的 turn 不得被搬进新窗口：{window_text}"
    );
    let read_back =
        NotesStore::read_file(session_dir, "checkpoint.md", None, None).expect("回读工作便签");
    assert!(
        read_back.text.contains(decision),
        "即使摘要被裁剪，也必须能按路径回读到原始决定"
    );
}

/// 端到端：长便签换窗后只留路径，但按路径回读仍能拿到决策原文。
#[test]
fn fresh_window_points_to_long_note_and_decision_stays_readable() {
    use crate::session_notes::{GoalAnchor, SessionNotes};
    use crate::session_notes_store::NotesStore;

    let directory = axon_test_support::test_temp_dir();
    let session_dir = directory.path();
    let decision = "决定：长便签只给路径，细节由新窗口回读";
    let long_note = format!("{decision}\n{}", "补充细节".repeat(2_000));
    NotesStore::write_file(session_dir, "checkpoint.md", &long_note, 1_000).expect("写入长便签");
    let notes = SessionNotes::new(GoalAnchor::new("上下文换窗改造", Vec::new()), 1_000);

    let summary = notes
        .try_render_canonical_summary(session_dir, &[])
        .expect("生成交接摘要");

    assert!(
        summary.contains("便签 checkpoint.md"),
        "长便签必须在摘要里留下可定位的路径：{summary}"
    );
    assert!(!summary.contains(&long_note), "长便签正文不得整段塞进摘要");
    assert!(
        summary.len() <= crate::session_notes::SESSION_NOTES_CHECKPOINT_MAX_BYTES,
        "摘要必须落在字节硬上限内"
    );

    let read_back =
        NotesStore::read_file(session_dir, "checkpoint.md", None, None).expect("回读长便签");
    assert!(
        read_back.text.contains(decision),
        "换窗后必须能按路径读到决策原文"
    );
}
