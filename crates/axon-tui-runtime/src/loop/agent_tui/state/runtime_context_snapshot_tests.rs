//! 上下文读数不粘连：未知就是未知，不沿用上一次非零值。

use crate::app_shell::context_snapshot::AgentTuiContextSnapshot;
use crate::r#loop::agent_tui::AgentTuiTokenUsage;

use super::runtime_flow_test_support::test_state;

fn snapshot(current_context_tokens: Option<u64>) -> AgentTuiContextSnapshot {
    AgentTuiContextSnapshot::new(current_context_tokens, Some(200_000), None, None)
}

#[test]
fn tui_context_reading_does_not_stick_to_previous_value() {
    let mut state = test_state();

    // 先读到一份真实占用。
    assert!(state.apply_context_snapshot(snapshot(Some(135_917))));
    assert_eq!(
        state.context_snapshot().current_context_tokens,
        Some(135_917)
    );

    // 下一帧没有占用读数：新帧值为零不得把上一次的非零值留着冒充现在。
    assert!(state.apply_context_snapshot(snapshot(None)));
    assert_eq!(
        state.context_snapshot().current_context_tokens,
        None,
        "未知必须覆盖旧读数，而不是保留上一次非零值"
    );
    assert!(!state.context_snapshot().has_context_reading());

    // 同样的未知读数不会重复触发重绘。
    assert!(!state.apply_context_snapshot(snapshot(None)));
}

#[test]
fn new_session_drops_previous_context_snapshot() {
    let mut state = test_state();
    assert!(state.apply_context_snapshot(snapshot(Some(135_917))));
    assert!(state.context_snapshot().has_context_reading());

    // `/new` 之后头部不得再显示上一条会话的上下文与剩余量。
    state.start_new_session();
    assert!(
        !state.context_snapshot().has_context_reading(),
        "新建会话必须清空上一条会话的上下文读数"
    );
    assert_eq!(
        state.context_snapshot().remaining_tokens(Some(200_000)),
        None
    );
}

#[test]
fn usage_frames_do_not_reintroduce_sticky_context_reading() {
    // 用量帧的「最近一次请求」读数同样按最后一条覆盖：没有读数时回到未知。
    let known = AgentTuiTokenUsage::new(500, 0, 0, 100, 0).with_current_context_tokens(60_655);
    let unknown = AgentTuiTokenUsage::new(900, 0, 0, 200, 0);
    assert_eq!(unknown.current_context_tokens(), None);
    assert_eq!(
        known.fold(unknown).current_context_tokens(),
        None,
        "在跑流没有占用读数时不得沿用已收口流的旧值"
    );
    assert_eq!(
        unknown.fold(known).current_context_tokens(),
        Some(60_655),
        "有读数的一侧仍然生效"
    );
}
