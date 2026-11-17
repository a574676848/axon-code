//! `axon sessions read` 的轮次终态标签。

use axon_domain::{
    RUNTIME_FAILURE_CATEGORY_INTERRUPTED, RUNTIME_FAILURE_CODE_INTERRUPTED, RuntimeFailure,
};
use axon_session::TurnStatus;

use super::{Turn, turn_status_label};

fn turn(status: TurnStatus, failure: Option<RuntimeFailure>) -> Turn {
    Turn {
        turn_id: "turn-test".to_owned(),
        sequence: 0,
        status,
        history_items: Vec::new(),
        token_usage_json: None,
        started_at_ms: 1,
        completed_at_ms: None,
        interrupted_at_ms: None,
        failure,
    }
}

#[test]
fn user_interrupt_is_labelled_interrupted_not_failed() {
    // 用户按下停止时 Thread 落成 Failed 加中断类载荷：界面读作「中断」而不是「失败」。
    let interrupted = turn(
        TurnStatus::Failed,
        Some(RuntimeFailure::new(
            RUNTIME_FAILURE_CATEGORY_INTERRUPTED,
            RUNTIME_FAILURE_CODE_INTERRUPTED,
            "本轮已由用户中断。",
            2,
        )),
    );
    assert_eq!(turn_status_label(&interrupted), "Interrupted");
}

#[test]
fn genuine_failure_and_completion_keep_their_own_labels() {
    let failed = turn(
        TurnStatus::Failed,
        Some(RuntimeFailure::new(
            "provider",
            "provider_failed",
            "失败",
            3,
        )),
    );
    assert_eq!(turn_status_label(&failed), "Failed");
    assert_eq!(
        turn_status_label(&turn(TurnStatus::Completed, None)),
        "Completed"
    );
    assert_eq!(
        turn_status_label(&turn(TurnStatus::Interrupted, None)),
        "Interrupted"
    );
}
