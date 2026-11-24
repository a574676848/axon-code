//! 完成通知的 daemon 面链路（合同 §5.1.5 daemon 面）。
//!
//! daemon 面不新建投递设施：完成通知派发时提交一条持久 steer Interaction，
//! 主 turn 因此自行继续；`interaction_turn_runner` 再按 source 前缀认领它并结算投递。
//! 本文件把这条链路的两个端点（派发侧生成键 / 消费侧识别键）直接钉在一起：
//! 两端一旦漂移，通知就会「派发了但主 turn 不认识」，而既有端到端用例只在
//! 同一进程内跑，未必能暴露前缀不一致。

use super::*;

/// 派发侧生成的 source 必须能被消费侧原样还原成同一个 Execution。
#[test]
fn completion_steer_keys_round_trip_from_producer_to_consumer() {
    let execution_id = ExecutionId::new("execution-chain-1").expect("Execution ID");

    let source_id = completion_steer_source_id(&execution_id);

    assert_eq!(
        completion_steer_execution_id(&source_id),
        Some(execution_id.clone()),
        "消费侧必须能从派发侧写入的 source 还原 Execution"
    );
}

/// 同一 Execution 的交互身份是确定的：重复派发不会产生第二条主 turn 通知。
#[test]
fn completion_steer_interaction_identity_is_deterministic() {
    let execution_id = ExecutionId::new("execution-chain-2").expect("Execution ID");

    let first = completion_steer_interaction_id(&execution_id);
    let second = completion_steer_interaction_id(&execution_id);

    assert_eq!(first, second, "同一 Execution 的 steer 交互 ID 必须稳定");
    assert_ne!(
        first,
        completion_steer_source_id(&execution_id),
        "交互 ID 与 source ID 属于不同命名空间，不得混用"
    );
}

/// 反例：普通用户输入不得被误认成完成通知驱动的主 turn。
#[test]
fn unrelated_sources_are_never_claimed_as_completion_steers() {
    for source in ["", "user-input-1", "cron-delivery-1", "steer-result-1"] {
        assert_eq!(
            completion_steer_execution_id(source),
            None,
            "非完成通知来源不得被认领：{source}"
        );
    }
}
