//! 用量读数按 provider 流记账的投影测试。
//!
//! 用量帧是**每条流的累计快照**，同一条流的快照会被重复投递：流内覆盖、流收口并入累计，
//! 只有跨流才相加。逐帧累加会翻倍，只保留最后一条流会漏掉前面各流。

use super::runtime_flow_test_support::{test_state, test_submission};
use crate::r#loop::agent_tui::run_output::AgentTuiTokenUsage;
use crate::r#loop::agent_tui::wire_event_test_support::{
    streaming_completed_event, usage_update_event,
};

#[test]
fn provider_stream_usage_frames_fold_instead_of_double_counting() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("统计 token"));
    let first = AgentTuiTokenUsage::new(596, 0, 0, 69, 1_525).with_current_context_tokens(60_655);

    state.apply_runtime_event(usage_update_event(first));
    // 共享 producer 会把同一条流的最终快照再投一次：读数必须保持不变。
    state.apply_runtime_event(usage_update_event(first));
    assert_eq!(state.last_turn_usage().total_tokens(), 596 + 69 + 1_525);

    state.apply_runtime_event(streaming_completed_event("cpa", "gemini-3.8-flash", 1));
    // 第二条流的快照累加到第一条之上，而不是把它替换掉（替换会只剩 3_092 + 3 + 164）。
    state.apply_runtime_event(usage_update_event(
        AgentTuiTokenUsage::new(3_092, 60_984, 0, 3, 164).with_current_context_tokens(64_049),
    ));

    assert_eq!(
        state.last_turn_usage().total_tokens(),
        596 + 69 + 1_525 + 3_092 + 60_984 + 3 + 164
    );
    assert_eq!(state.last_turn_usage().output_tokens, 72);
    assert_eq!(state.last_turn_usage().reasoning_output_tokens, 1_689);
    assert_eq!(state.last_turn_usage().current_context_tokens(), 64_049);
}

#[test]
fn resetting_a_turn_clears_the_folded_totals_too() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("统计 token"));
    state.apply_runtime_event(usage_update_event(AgentTuiTokenUsage::new(10, 0, 0, 5, 0)));
    state.apply_runtime_event(streaming_completed_event("cpa", "gemini-3.8-flash", 1));

    state.start_runtime_submit(&test_submission("下一轮"));

    // 上一轮的读数留在 header 上直到新一轮产生观测，但累计不得带进新一轮。
    state.apply_runtime_event(usage_update_event(AgentTuiTokenUsage::new(7, 0, 0, 2, 0)));
    assert_eq!(
        state.last_turn_usage().total_tokens(),
        9,
        "新一轮读数不得带上上一轮的累计"
    );
}
