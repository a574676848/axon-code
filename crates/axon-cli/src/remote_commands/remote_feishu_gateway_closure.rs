use std::path::Path;

use serde::Serialize;

use super::{
    RemoteGatewayCommand,
    remote_feishu_gateway::{
        execute_feishu_gateway_daemon,
        remote_feishu_gateway_execution::remote_feishu_gateway_lanes::GatewayLaneScheduler,
    },
};

const GATEWAY_TICK_LIMIT: usize = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FeishuGatewayClosureReport {
    pub gateway_ticks: usize,
    pub gateway_input_events: usize,
    pub gateway_agent_executed_events: usize,
    pub gateway_reply_enqueued_events: usize,
    pub gateway_session_reset_events: usize,
    pub gateway_agent_failed_events: usize,
    pub worker_executed_items: usize,
    pub worker_delivered_count: usize,
    pub worker_feedback_enqueued: usize,
    pub feedback_executed_items: usize,
    pub feedback_completed_count: usize,
    pub feedback_retryable_count: usize,
    pub feedback_permanent_count: usize,
}

/// 构造 daemon shell 模式的单次 tick 命令。
///
/// 车道调度器在 worker 生命周期内长期存活，命令也必须与之同生命周期，否则每个 tick
/// 重建命令会让车道拿不到稳定的会话与事件源绑定。
pub(super) fn build_gateway_daemon_command(
    session_id: u64,
    event_source_file: &Path,
) -> RemoteGatewayCommand {
    RemoteGatewayCommand {
        session_id,
        event_json: None,
        event_file: Some(event_source_file.to_path_buf()),
        daemon: true,
        interval_ms: None,
        max_ticks: Some(GATEWAY_TICK_LIMIT),
        cursor_file: None,
        daemon_endpoint: super::REMOTE_FEISHU_DAEMON_ENDPOINT.to_owned(),
    }
}

pub(super) fn run_feishu_gateway_closure_tick(
    command: &RemoteGatewayCommand,
    scheduler: &GatewayLaneScheduler,
) -> Result<FeishuGatewayClosureReport, String> {
    let gateway = execute_feishu_gateway_daemon(command, scheduler)?;
    Ok(FeishuGatewayClosureReport {
        gateway_ticks: gateway.completed_ticks,
        gateway_input_events: gateway.input_events,
        gateway_agent_executed_events: gateway.agent_executed_events,
        gateway_reply_enqueued_events: gateway.reply_enqueued_events,
        gateway_session_reset_events: gateway.session_reset_events,
        gateway_agent_failed_events: gateway.agent_failed_events,
        ..FeishuGatewayClosureReport::default()
    })
}

pub(super) fn accumulate_closure_report(
    totals: &mut FeishuGatewayClosureReport,
    tick: &FeishuGatewayClosureReport,
) {
    totals.gateway_ticks += tick.gateway_ticks;
    totals.gateway_input_events += tick.gateway_input_events;
    totals.gateway_agent_executed_events += tick.gateway_agent_executed_events;
    totals.gateway_reply_enqueued_events += tick.gateway_reply_enqueued_events;
    totals.gateway_session_reset_events += tick.gateway_session_reset_events;
    totals.gateway_agent_failed_events += tick.gateway_agent_failed_events;
    totals.worker_executed_items += tick.worker_executed_items;
    totals.worker_delivered_count += tick.worker_delivered_count;
    totals.worker_feedback_enqueued += tick.worker_feedback_enqueued;
    totals.feedback_executed_items += tick.feedback_executed_items;
    totals.feedback_completed_count += tick.feedback_completed_count;
    totals.feedback_retryable_count += tick.feedback_retryable_count;
    totals.feedback_permanent_count += tick.feedback_permanent_count;
}
