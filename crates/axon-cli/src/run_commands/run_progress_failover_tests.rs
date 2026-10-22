//! 渠道 failover 的 run progress 投影身份回归。
//!
//! failover 会在同一个 run 内重跑整个 provider turn loop，因此 turn 序号必须由 run 级
//! 水位分配：复用上一尝试的 turn 序号会让两段正文落到同一个 part 身份上。

use super::*;

#[test]
fn failover_attempts_get_disjoint_provider_turn_epochs_and_assistant_parts() {
    let mut events = Vec::new();
    let mut emit = |event| events.push(event);
    let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());

    // 第一次渠道尝试：turn 1..=3。
    let first_base = sink.begin_provider_attempt();
    assert_eq!(first_base, 1);
    for turn_id in first_base..=3 {
        sink.observe_provider_turn(turn_id);
    }
    sink.assistant_delta(
        axon_agent_runtime::agent_run_assistant_part_id(3),
        "provider-message:3",
        "第一次尝试的残缺正文",
    );

    // 渠道 failover 重跑 turn loop：第二次尝试必须从上一尝试之后接着编号。
    let second_base = sink.begin_provider_attempt();
    assert_eq!(second_base, 4);
    assert_ne!(
        axon_agent_runtime::agent_run_assistant_part_id(first_base),
        axon_agent_runtime::agent_run_assistant_part_id(second_base)
    );
    sink.assistant_delta(
        axon_agent_runtime::agent_run_assistant_part_id(second_base),
        "provider-message:4",
        "第二次尝试的残缺正文",
    );
    sink.observe_provider_turn(second_base);

    // 迟到的低序号观测不得让水位回退。
    sink.observe_provider_turn(1);
    assert_eq!(sink.begin_provider_attempt(), 5);

    // 失败诊断记录按尝试分段：不再把两次尝试的无分隔拼接当成同一段正文。
    assert_eq!(
        sink.partial_assistant_content(),
        "第一次尝试的残缺正文\n第二次尝试的残缺正文"
    );
}
