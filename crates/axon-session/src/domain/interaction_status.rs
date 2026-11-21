//! Interaction 状态口径：落库取值、协议帧与终态判定共用的一处定义。
//!
//! 领域枚举 `axon_domain::InteractionStatus` 是唯一来源。SQL 派生列、网关协议帧与
//! 终态判定此前各自写了一份取值表，改一处忘一处就会让「服务端说已结束、界面说在跑」
//! 重新出现。这里把两套消费方需要的东西集中一处，并用穷尽 match 约束：
//! 枚举新增分支时本模块编译失败，不可能静默漂移。

use axon_domain::{ExecutionStatus, InteractionStatus};

/// `InteractionStatus` 的全部取值，顺序与枚举声明一致。
///
/// 供 SQL 生成、协议帧与测试遍历：需要「所有取值」时不得再各自手写一份。
pub const ALL_STATUSES: [InteractionStatus; 5] = [
    InteractionStatus::Received,
    InteractionStatus::Processing,
    InteractionStatus::Understood,
    InteractionStatus::Rejected,
    InteractionStatus::Interrupted,
];

/// 已形成结论的 Interaction 状态：终态集合的唯一来源。
pub const TERMINAL_STATUSES: [InteractionStatus; 3] = [
    InteractionStatus::Understood,
    InteractionStatus::Rejected,
    InteractionStatus::Interrupted,
];

/// `ExecutionStatus` 的全部取值，顺序与枚举声明一致。
pub const ALL_EXECUTION_STATUSES: [ExecutionStatus; 7] = [
    ExecutionStatus::Queued,
    ExecutionStatus::Running,
    ExecutionStatus::Waiting,
    ExecutionStatus::Uncertain,
    ExecutionStatus::Completed,
    ExecutionStatus::Failed,
    ExecutionStatus::Cancelled,
];

/// 已形成结论的 Execution 状态：`ExecutionStatus::is_conclusive` 的枚举化。
///
/// 判据本身在领域层（`is_conclusive`），这里只是它的遍历形式，供 INV-2 的
/// 检查 SQL 与测试生成状态词表；两者不一致时相关测试会失败。
pub const CONCLUSIVE_EXECUTION_STATUSES: [ExecutionStatus; 4] = [
    ExecutionStatus::Completed,
    ExecutionStatus::Failed,
    ExecutionStatus::Cancelled,
    ExecutionStatus::Uncertain,
];

/// 尚未形成结论的 Execution 状态：`CONCLUSIVE_EXECUTION_STATUSES` 的补集。
pub const IN_FLIGHT_EXECUTION_STATUSES: [ExecutionStatus; 3] = [
    ExecutionStatus::Queued,
    ExecutionStatus::Running,
    ExecutionStatus::Waiting,
];

/// Interaction 状态的落库与协议稳定取值。
#[must_use]
pub const fn token(status: InteractionStatus) -> &'static str {
    match status {
        InteractionStatus::Received => "received",
        InteractionStatus::Processing => "processing",
        InteractionStatus::Understood => "understood",
        InteractionStatus::Rejected => "rejected",
        InteractionStatus::Interrupted => "interrupted",
    }
}

/// 从落库取值还原状态；不属于 Interaction 词汇时返回 `None`。
#[must_use]
pub fn from_token(value: &str) -> Option<InteractionStatus> {
    ALL_STATUSES
        .into_iter()
        .find(|status| token(*status) == value)
}

/// 是否已形成终态。
///
/// 终态判定只读这一处：网关收口帧、订阅基线与索引页派生列共用同一份结论，
/// 各自写一份 `matches!` 就会出现「库里是终态、帧里还在跑」。
#[must_use]
pub const fn is_terminal(status: InteractionStatus) -> bool {
    matches!(
        status,
        InteractionStatus::Understood
            | InteractionStatus::Rejected
            | InteractionStatus::Interrupted
    )
}

/// 执行侧的结论状态派生到 Interaction 口径。
///
/// `completed` 是正常收口，`failed` 与 `cancelled` 是没做成，`uncertain` 是外部结果
/// 未知：四种结论状态在 Interaction 口径下只有两个归宿，成功或中断。非结论状态返回
/// `None`，调用方据此拒绝把在途状态当成结论。
#[must_use]
pub const fn derived_from_conclusive(status: ExecutionStatus) -> Option<InteractionStatus> {
    match status {
        ExecutionStatus::Completed => Some(InteractionStatus::Understood),
        ExecutionStatus::Failed | ExecutionStatus::Cancelled | ExecutionStatus::Uncertain => {
            Some(InteractionStatus::Interrupted)
        }
        ExecutionStatus::Queued | ExecutionStatus::Running | ExecutionStatus::Waiting => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_every_interaction_status_token() {
        for status in ALL_STATUSES {
            assert_eq!(from_token(token(status)), Some(status), "取值必须可往返");
        }
        assert_eq!(from_token("unknown"), None);
    }

    #[test]
    fn terminal_set_is_derived_from_the_enum() {
        for status in ALL_STATUSES {
            let expected = TERMINAL_STATUSES.contains(&status);
            assert_eq!(is_terminal(status), expected, "终态集合与谓词必须同源");
        }
    }

    #[test]
    fn conclusive_statuses_follow_domain() {
        for status in CONCLUSIVE_EXECUTION_STATUSES {
            assert!(status.is_conclusive(), "结论集合里只能放结论状态");
            assert!(
                derived_from_conclusive(status).is_some(),
                "每个结论状态都必须能派生 Interaction 口径"
            );
        }
        for status in IN_FLIGHT_EXECUTION_STATUSES {
            assert!(!status.is_conclusive(), "在途状态不得混进结论集合");
            assert_eq!(derived_from_conclusive(status), None);
        }
        // 两个集合必须是全部取值的划分：新增枚举分支时这里失败，不会漏一处。
        let mut covered = 0;
        for status in ALL_EXECUTION_STATUSES {
            let conclusive = CONCLUSIVE_EXECUTION_STATUSES.contains(&status);
            let in_flight = IN_FLIGHT_EXECUTION_STATUSES.contains(&status);
            assert!(conclusive ^ in_flight, "状态必须恰好落在一个集合里");
            assert_eq!(conclusive, status.is_conclusive(), "划分必须与领域判据一致");
            covered += 1;
        }
        assert_eq!(covered, ALL_EXECUTION_STATUSES.len());
    }
}
