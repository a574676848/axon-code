//! 完成通知驱动的主 turn 自动继续（合同 §5.1.5 TUI 面）。
//!
//! 判据是「TUI 空闲会话收到通知后无需用户输入即继续」：通知一到，空闲会话必须
//! 接上 daemon 已由该通知驱动的主 turn；已有活跃回合时不得另起一轮。

use super::super::resume_notice_driven_turn;
use super::test_support::{TestRuntimeAdapter, test_launch_request, test_state};
use super::{AgentTuiRuntimeAdapter, RuntimeSubmissionDriver};
use crate::r#loop::agent_tui::AgentTuiActiveTurn;

/// 记录 adapter 被查询次数的包装：用于断言「没有活动回合时不得另起一轮」。
#[derive(Clone, Default)]
struct CountingAdapter {
    inner: TestRuntimeAdapter,
    load_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl AgentTuiRuntimeAdapter for CountingAdapter {
    fn contract(&self) -> crate::surface::runtime_adapter::RuntimeAdapterContract {
        AgentTuiRuntimeAdapter::contract(&self.inner)
    }

    fn submit(
        &self,
        submission: &super::AgentTuiSubmit,
        cancel_token: &super::AgentTuiCancelToken,
        emit: &mut super::RuntimeEventEmitter<'_, crate::r#loop::agent_tui::AgentTuiRuntimeEvent>,
    ) -> Result<
        crate::r#loop::agent_tui::AgentTuiRunOutput,
        crate::r#loop::agent_tui::AgentTuiSubmitFailure,
    > {
        AgentTuiRuntimeAdapter::submit(&self.inner, submission, cancel_token, emit)
    }

    fn load_active_turn(
        &self,
        request: &crate::r#loop::agent_tui::AgentTuiLaunchRequest,
    ) -> Option<AgentTuiActiveTurn> {
        self.load_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        AgentTuiRuntimeAdapter::load_active_turn(&self.inner, request)
    }
}

/// 空闲会话收到通知后必须接上 daemon 的活动回合，而不是等用户输入。
#[test]
fn idle_session_resumes_the_notice_driven_turn() {
    let request = test_launch_request();
    let mut state = test_state();
    let adapter = TestRuntimeAdapter {
        active_turn: Some(AgentTuiActiveTurn::new("完成通知驱动的回合")),
        ..TestRuntimeAdapter::default()
    };
    let mut driver = RuntimeSubmissionDriver::new(adapter.clone());

    assert!(!driver.is_active(), "前置条件：会话必须处于空闲");

    let resumed = resume_notice_driven_turn(&mut state, &mut driver, &adapter, &request);

    assert!(resumed, "空闲会话必须接上通知驱动的主 turn");
    assert!(driver.is_active(), "接上之后必须存在活跃提交");
}

/// 已有活跃回合时不得重复接上：否则同一条通知会被消费两次。
#[test]
fn active_submission_is_not_resumed_again() {
    let request = test_launch_request();
    let mut state = test_state();
    let adapter = TestRuntimeAdapter {
        active_turn: Some(AgentTuiActiveTurn::new("完成通知驱动的回合")),
        ..TestRuntimeAdapter::default()
    };
    let mut driver = RuntimeSubmissionDriver::new(adapter.clone());
    assert!(resume_notice_driven_turn(
        &mut state,
        &mut driver,
        &adapter,
        &request
    ));

    let resumed_again = resume_notice_driven_turn(&mut state, &mut driver, &adapter, &request);

    assert!(!resumed_again, "已有活跃回合时不得再次接上");
}

/// 反例：daemon 没有活动回合时不得另起一轮：通知的消费属于 daemon 的投递状态机。
#[test]
fn absent_active_turn_does_not_start_a_new_round() {
    let request = test_launch_request();
    let mut state = test_state();
    let adapter = CountingAdapter {
        inner: TestRuntimeAdapter::default(),
        load_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    };
    let mut driver = RuntimeSubmissionDriver::new(adapter.clone());

    let resumed = resume_notice_driven_turn(&mut state, &mut driver, &adapter, &request);

    assert!(!resumed, "没有可恢复回合时不得另起一轮");
    assert!(!driver.is_active(), "不得因通知凭空产生活跃提交");
    assert_eq!(
        adapter
            .load_calls
            .load(std::sync::atomic::Ordering::Relaxed),
        1,
        "必须真实查询过 daemon 的活动回合"
    );
}
