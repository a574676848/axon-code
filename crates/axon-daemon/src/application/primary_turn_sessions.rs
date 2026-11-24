//! Primary Turn 复合车道分组、结果归并与等待策略。

use super::primary_turn_service::{PrimaryTurnServiceError, PrimaryTurnTickReport};
use axon_session::{InteractionProcessingLease, interaction_session_id};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

const SESSION_RESULT_CHANNEL_CLOSED_ERROR: &str = "Primary turn session 结果通道意外关闭";

pub(super) fn group_sessions(
    leases: Vec<InteractionProcessingLease>,
) -> BTreeMap<String, Vec<InteractionProcessingLease>> {
    let mut sessions = BTreeMap::new();
    for lease in leases {
        let key = interaction_session_id(&lease.interaction);
        sessions.entry(key).or_insert_with(Vec::new).push(lease);
    }
    sessions
}

/// 把一个 session 的执行报告并入本轮汇总。
pub(super) fn merge_session_report(
    report: &mut PrimaryTurnTickReport,
    session_report: PrimaryTurnTickReport,
) {
    report.claimed = report.claimed.saturating_add(session_report.claimed);
    report.understood = report.understood.saturating_add(session_report.understood);
    report.rejected = report.rejected.saturating_add(session_report.rejected);
    report.failures.extend(session_report.failures);
}

/// session 结果通道的等待结论。
pub(super) enum SessionReportWait {
    /// 某个 session 已结束并回传报告。
    Report(Result<PrimaryTurnTickReport, PrimaryTurnServiceError>),
    /// 等待窗口内没有 session 结束，协调线程应重新尝试领取新到工作。
    Idle,
}

/// 只等待「任意一个 session」结束。
///
/// `idle_window` 为空表示当前没有空闲 session 槽位：此时新工作无处投递，直接阻塞等待；
/// 否则使用有界窗口，让窗口超时后重新领取空闲 session 的新消息，避免为最慢的 session 空等。
pub(super) fn await_session_report(
    receiver: &mpsc::Receiver<Result<PrimaryTurnTickReport, PrimaryTurnServiceError>>,
    idle_window: Option<Duration>,
) -> Result<SessionReportWait, PrimaryTurnServiceError> {
    let Some(window) = idle_window else {
        return receiver
            .recv()
            .map(SessionReportWait::Report)
            .map_err(|_| session_result_channel_closed());
    };
    match receiver.recv_timeout(window) {
        Ok(result) => Ok(SessionReportWait::Report(result)),
        Err(RecvTimeoutError::Timeout) => Ok(SessionReportWait::Idle),
        Err(RecvTimeoutError::Disconnected) => Err(session_result_channel_closed()),
    }
}

fn session_result_channel_closed() -> PrimaryTurnServiceError {
    PrimaryTurnServiceError::Runner(SESSION_RESULT_CHANNEL_CLOSED_ERROR.to_owned())
}
