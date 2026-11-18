//! 预算分档提示与步均采样回归测试。

use super::{CompressionPolicy, WindowRolloverPolicyRuntime};
use crate::token_budget_guard::{StepPace, TokenBudgetRuntimePolicy};
use crate::{
    AgentModelHistorySnapshot, AgentModelMessage, AgentModelMessageKind,
    AgentModelToolCallMetadata, AgentRuntimeNoteKind,
};

fn runtime(context_window: u64) -> WindowRolloverPolicyRuntime {
    WindowRolloverPolicyRuntime::new(CompressionPolicy {
        trigger_tokens: context_window / 10 * 8,
        context_window,
    })
}

#[test]
fn warn_headroom_tracks_steps_instead_of_absolute_tokens() {
    // 同一步幅下，1M 与 200k 窗口的预警提前量一致，折算出的步数也一致。
    for window in [200_000_u64, 1_000_000] {
        let trigger = window / 10 * 8;
        let headroom = TokenBudgetRuntimePolicy::warn_headroom(trigger, window, Some(12_000));
        assert_eq!(headroom, 48_000, "窗口 {window} 的提前量应按步幅换算");
        assert_eq!(headroom / 12_000, 4, "提前量应覆盖固定的 4 步");
    }
    // 没有采样时退回窗口比例，短窗口再由绝对下限兜底。
    assert_eq!(
        TokenBudgetRuntimePolicy::warn_headroom(800_000, 1_000_000, None),
        30_000
    );
    assert_eq!(
        TokenBudgetRuntimePolicy::warn_headroom(160_000, 200_000, None),
        6_144
    );
}

#[test]
fn warn_headroom_is_capped_so_one_huge_step_cannot_warn_at_window_start() {
    // 单步 200k 时按 4 步换算会超过触发线本身，必须被触发线的一半封顶。
    let headroom = TokenBudgetRuntimePolicy::warn_headroom(160_000, 200_000, Some(200_000));
    assert_eq!(headroom, 80_000);
    assert!(
        !TokenBudgetRuntimePolicy::should_emit_warn(160_000, 1_000, headroom),
        "封顶之后不允许在窗口刚开启时就发出预警"
    );
}

#[test]
fn step_pace_uses_median_and_resets_when_usage_shrinks() {
    let mut pace = StepPace::default();
    assert_eq!(pace.observe(1_000), None, "第一次观察只建立基线");
    assert_eq!(pace.observe(2_000), Some(1_000));
    assert_eq!(pace.observe(52_000), Some(50_000), "一次大文件读取");
    assert_eq!(pace.observe(53_000), Some(1_000));
    assert_eq!(
        pace.tokens_per_step(),
        Some(1_000),
        "中位数必须抗住单次离群步幅"
    );
    assert_eq!(pace.observe(100), None, "用量下降表示刚换窗");
    assert_eq!(pace.sample_count(), 0);
    assert_eq!(pace.tokens_per_step(), None);
}

#[test]
fn warn_fires_by_step_pace_and_only_once_per_window() {
    let mut runtime = runtime(1_000_000);
    for current in [100_000, 112_000, 124_000] {
        runtime.observe_context(current);
    }
    assert_eq!(runtime.step_pace.tokens_per_step(), Some(12_000));

    let mut messages: AgentModelHistorySnapshot = vec![
        AgentModelMessage::text("user", "task"),
        AgentModelMessage::runtime_note(
            AgentRuntimeNoteKind::MemoryHygiene,
            super::notice::midway_note(),
        ),
    ]
    .into();
    // 保值提示已经发过：这里只观察收尾预警的边界。
    runtime.refresh_rollover_markers(&messages);
    assert!(
        !runtime.maybe_emit_budget_notice(&mut messages, Some(751_999)),
        "提前量之外不应提醒"
    );
    assert!(
        runtime.maybe_emit_budget_notice(&mut messages, Some(752_000)),
        "到达提前量应发出收尾预警"
    );
    assert_eq!(messages.len(), 3);
    assert!(matches!(
        messages.as_slice()[2].kind,
        AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::PayloadBudgetGate)
    ));
    assert!(!runtime.maybe_emit_budget_notice(&mut messages, Some(760_000)));
    assert_eq!(messages.len(), 3, "同一窗口只提醒一次");
}

#[test]
fn midway_fires_on_steps_even_when_budget_is_wide_open() {
    let mut runtime = runtime(4_000_000);
    let mut messages: AgentModelHistorySnapshot =
        vec![AgentModelMessage::text("user", "task")].into();
    for _ in 0..8 {
        messages.push(AgentModelMessage::text("assistant", "step"));
    }
    runtime.refresh_rollover_markers(&messages);
    assert_eq!(runtime.steps_since_notes, 8);

    assert!(
        runtime.maybe_emit_budget_notice(&mut messages, Some(10_000)),
        "距上次便签写入的步数达标时应提醒记录，哪怕预算还很宽裕"
    );
    let note = messages.as_slice().last().expect("应追加一条提示");
    assert!(matches!(
        note.kind,
        AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::MemoryHygiene)
    ));
    assert!(!runtime.maybe_emit_budget_notice(&mut messages, Some(10_000)));
}

#[test]
fn notes_write_resets_the_forgetting_clock() {
    let mut runtime = runtime(4_000_000);
    let mut messages = Vec::new();
    for _ in 0..8 {
        messages.push(AgentModelMessage::text("assistant", "step"));
    }
    messages.push(
        AgentModelMessage::text("assistant", "write").with_tool_calls(vec![
            AgentModelToolCallMetadata {
                id: "call-notes".to_owned(),
                name: "builtin.notes_append_to_file".to_owned(),
                input: serde_json::json!({"path": "checkpoint.md", "text": "进展"}).into(),
            },
        ]),
    );
    messages.push(AgentModelMessage::tool_result("call-notes", "ok", true));

    runtime.refresh_rollover_markers(&messages);
    assert_eq!(runtime.steps_since_notes, 0, "刚写过便签就不再催");

    messages.push(AgentModelMessage::text("assistant", "step"));
    messages.push(AgentModelMessage::text("assistant", "step"));
    runtime.refresh_rollover_markers(&messages);
    assert_eq!(runtime.steps_since_notes, 2);
}

#[test]
fn midway_marker_is_restored_from_window_history() {
    let messages = vec![AgentModelMessage::runtime_note(
        AgentRuntimeNoteKind::MemoryHygiene,
        super::notice::midway_note(),
    )];
    let mut runtime = runtime(4_000_000);
    runtime.refresh_rollover_markers(&messages);
    assert!(
        runtime.midway_emitted,
        "重启后必须从历史识别出已经发过的保值提示"
    );
}

#[test]
fn warn_takes_priority_when_both_tiers_are_due_in_one_turn() {
    let mut runtime = runtime(1_000_000);
    for current in [100_000, 112_000, 124_000] {
        runtime.observe_context(current);
    }
    let mut messages: AgentModelHistorySnapshot =
        vec![AgentModelMessage::text("user", "task")].into();
    for _ in 0..8 {
        messages.push(AgentModelMessage::text("assistant", "step"));
    }
    runtime.refresh_rollover_markers(&messages);

    // 这一轮同时满足保值条件（8 步未写便签）与收尾条件（进入提前量）。
    assert!(runtime.maybe_emit_budget_notice(&mut messages, Some(752_000)));
    let note = messages.as_slice().last().expect("应追加一条提示");
    assert!(
        matches!(
            note.kind,
            AgentModelMessageKind::RuntimeNote(AgentRuntimeNoteKind::PayloadBudgetGate)
        ),
        "收尾预警优先，不能先发保值提示把预算信号吞掉"
    );
    assert!(runtime.midway_emitted, "收尾预警之后不再补发保值提示");
    assert!(!runtime.maybe_emit_budget_notice(&mut messages, Some(760_000)));
}
