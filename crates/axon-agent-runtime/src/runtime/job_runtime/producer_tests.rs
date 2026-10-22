//! 生产者接口与统一终态投影的行为测试（合同 §8.1 第 6 条）。

use super::*;

/// 六类终态必须与作业状态一一对应，且标签稳定（通知载荷与事件键依赖它们）。
#[test]
fn six_terminal_kinds_round_trip_through_job_status() {
    let pairs = [
        (JobStatus::Completed, JobTerminalKind::Completed),
        (JobStatus::Refusal, JobTerminalKind::Refusal),
        (JobStatus::Failed, JobTerminalKind::Failed),
        (JobStatus::Killed, JobTerminalKind::Killed),
        (JobStatus::Timeout, JobTerminalKind::Timeout),
        (JobStatus::Unresponsive, JobTerminalKind::Unresponsive),
    ];
    assert_eq!(pairs.len(), 6, "合同 §8.1 第 6 条要求六类终态");
    for (status, kind) in pairs {
        assert_eq!(status.terminal_kind(), Some(kind));
        assert_eq!(kind.status(), status);
    }
    assert_eq!(JobStatus::Running.terminal_kind(), None);
}

/// 生产者标签必须唯一且稳定：事件键把三类 producer 区分开。
#[test]
fn producer_kinds_have_stable_distinct_labels() {
    assert_eq!(JobProducerKind::Shell.as_str(), "shell");
    assert_eq!(JobProducerKind::Subagent.as_str(), "subagent");
    assert_ne!(
        JobProducerKind::Shell.as_str(),
        JobProducerKind::Subagent.as_str()
    );
}

/// 六类终态的稳定标签：`Killed` 只表示显式终止，不得被超时或拒绝复用。
#[test]
fn terminal_kind_labels_are_distinct() {
    let labels = [
        JobTerminalKind::Completed.as_str(),
        JobTerminalKind::Refusal.as_str(),
        JobTerminalKind::Failed.as_str(),
        JobTerminalKind::Killed.as_str(),
        JobTerminalKind::Timeout.as_str(),
        JobTerminalKind::Unresponsive.as_str(),
    ];
    let mut unique = labels.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), labels.len(), "六类终态标签必须互不相同");
}

/// 预算耗尽独立于普通失败：它必须有自己的固定措辞（合同 §5.2.3）。
#[test]
fn budget_exhausted_reason_is_a_distinct_fixed_wording() {
    assert!(
        JOB_REASON_BUDGET_EXHAUSTED.contains("预算耗尽"),
        "预算耗尽必须有固定措辞：{JOB_REASON_BUDGET_EXHAUSTED}"
    );
    assert_ne!(
        JOB_REASON_BUDGET_EXHAUSTED,
        JobTerminalKind::Failed.as_str()
    );
}

/// 退出原因必须被统一截断到有界长度。
#[test]
fn exit_reason_is_bounded_by_character_count() {
    let reason = "长".repeat(crate::runtime_limits::JOB_EXIT_REASON_MAX_CHARS + 40);
    let bounded = bounded_job_reason(&reason);

    assert_eq!(
        bounded.chars().count(),
        crate::runtime_limits::JOB_EXIT_REASON_MAX_CHARS
    );
    assert!(reason.starts_with(&bounded));
}
