//! Primary Turn 复合车道分组、结果归并与等待策略。

use super::primary_turn_service::{PrimaryTurnServiceError, PrimaryTurnTickReport};
use axon_session::{InteractionProcessingLease, interaction_session_lane_id};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

const GLOBAL_LANE_KEY: &str = "global";
const LANE_RESULT_CHANNEL_CLOSED_ERROR: &str = "Primary turn lane 结果通道意外关闭";

pub(super) fn group_lanes(
    leases: Vec<InteractionProcessingLease>,
) -> BTreeMap<String, Vec<InteractionProcessingLease>> {
    let mut lanes = BTreeMap::new();
    for lease in leases {
        let key = interaction_session_lane_id(&lease.interaction)
            .unwrap_or_else(|| GLOBAL_LANE_KEY.to_owned());
        lanes.entry(key).or_insert_with(Vec::new).push(lease);
    }
    lanes
}

/// 把一个 lane 的执行报告并入本轮汇总。
pub(super) fn merge_lane_report(
    report: &mut PrimaryTurnTickReport,
    lane_report: PrimaryTurnTickReport,
) {
    report.claimed = report.claimed.saturating_add(lane_report.claimed);
    report.understood = report.understood.saturating_add(lane_report.understood);
    report.rejected = report.rejected.saturating_add(lane_report.rejected);
    report.failures.extend(lane_report.failures);
}

/// lane 结果通道的等待结论。
pub(super) enum LaneReportWait {
    /// 某个 lane 已结束并回传报告。
    Report(Result<PrimaryTurnTickReport, PrimaryTurnServiceError>),
    /// 等待窗口内没有 lane 结束，协调线程应重新尝试领取新到工作。
    Idle,
}

/// 只等待「任意一个 lane」结束。
///
/// `idle_window` 为空表示当前没有空闲 lane 槽位：此时新工作无处投递，直接阻塞等待；
/// 否则使用有界窗口，让窗口超时后重新领取空闲 lane 的新消息，避免为最慢的 lane 空等。
pub(super) fn await_lane_report(
    receiver: &mpsc::Receiver<Result<PrimaryTurnTickReport, PrimaryTurnServiceError>>,
    idle_window: Option<Duration>,
) -> Result<LaneReportWait, PrimaryTurnServiceError> {
    let Some(window) = idle_window else {
        return receiver
            .recv()
            .map(LaneReportWait::Report)
            .map_err(|_| lane_result_channel_closed());
    };
    match receiver.recv_timeout(window) {
        Ok(result) => Ok(LaneReportWait::Report(result)),
        Err(RecvTimeoutError::Timeout) => Ok(LaneReportWait::Idle),
        Err(RecvTimeoutError::Disconnected) => Err(lane_result_channel_closed()),
    }
}

fn lane_result_channel_closed() -> PrimaryTurnServiceError {
    PrimaryTurnServiceError::Runner(LANE_RESULT_CHANNEL_CLOSED_ERROR.to_owned())
}
