//! 待决问答作答的后台发送与结果回投。
//!
//! 作答不是一轮新任务：提问所属 turn 仍在运行，`RuntimeSubmissionDriver::start`
//! 会顶掉活跃提交的事件邮箱并丢掉这一轮后续事件，因此这里与 steer 共用
//! 「后台线程 + 邮箱回投」通道，但不占用本地待提交 steer 队列。
//! 只有发送成功才收起提问面板，失败保留面板与已填内容供原样重试。

use std::{panic, thread};

use super::{
    AgentTuiSteerResult, RuntimeError, RuntimeEventEmitter, RuntimeSubmissionDriver, panic_summary,
};
use crate::r#loop::agent_tui::{AgentTuiRuntimeAdapter, state::AgentTuiState};
use crate::surface::runtime_adapter::RuntimeEventSender;

/// 把待决问答的作答发往统一答案入口。
///
/// 返回 `true` 表示本次作答已被消费（发出或明确落为失败提示），调用方无需再处理。
pub(super) fn answer_question<F>(
    driver: &mut RuntimeSubmissionDriver<F>,
    state: &mut AgentTuiState,
) -> bool
where
    F: AgentTuiRuntimeAdapter,
{
    let Some(submission) = state.take_pending_runtime_submit() else {
        return false;
    };
    if !submission.is_question_answer() {
        state.restore_pending_runtime_submit(submission);
        return false;
    }
    let Some(active) = driver.active.as_ref() else {
        // 没有活跃提交就没有可回投结果的邮箱，不能把作答静默丢弃。
        state.restore_pending_runtime_submit(submission);
        state.push_question_answer_rejected_notice(Some(RuntimeError::runtime(
            "当前没有运行中的 turn，作答未发送",
        )));
        return true;
    };
    let adapter = driver.submit.clone();
    let mailbox = active.mailbox.clone();
    let answer_handle = thread::spawn(move || {
        let response = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let emit_mailbox = mailbox.clone();
            let mut emit_progress = |event| {
                emit_mailbox.push_progress(event);
            };
            let try_emit_mailbox = mailbox.clone();
            let runtime_sender = RuntimeEventSender::new(move |event| {
                try_emit_mailbox.push_progress(event);
                Ok(())
            });
            let mut emitter =
                RuntimeEventEmitter::with_try_emit(&mut emit_progress, runtime_sender);
            adapter.steer(&submission, &mut emitter)
        }))
        .unwrap_or_else(|panic_payload| {
            Err(RuntimeError::worker_panic(panic_summary(panic_payload)))
        });
        let apply = move |state: &mut AgentTuiState| match response {
            Ok(AgentTuiSteerResult::Accepted) => state.settle_question_answer_submission(),
            Ok(AgentTuiSteerResult::Unsupported) | Ok(AgentTuiSteerResult::Rejected) => {
                state.push_question_answer_rejected_notice(None);
            }
            Err(error) => state.push_question_answer_rejected_notice(Some(error)),
        };
        mailbox.push_apply_steer(Box::new(apply));
    });
    if let Some(active) = driver.active.as_mut() {
        active.worker_handles.push(answer_handle);
    }
    true
}
