use std::{thread, time::Duration};

use serde::Serialize;

use self::remote_feishu_gateway_execution::remote_feishu_gateway_lanes::GatewayLaneScheduler;
use self::remote_feishu_gateway_input::{
    RemoteFeishuGatewayCursorState, read_cursor_state, read_gateway_input_events_after_cursor,
    reset_gateway_event_source_if_fully_acknowledged, resolve_cursor_file, write_cursor_state,
};
use super::{
    RemoteCliContext, RemoteGatewayCommand,
    remote_feishu_gateway_report::{
        DispatchCounts, MessageKindCounts, dispatch_counts, message_kind_counts,
    },
};
use axon_domain::RuntimeFailure;
use axon_remote_feishu::REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS;

#[path = "remote_feishu_gateway_execution.rs"]
pub(super) mod remote_feishu_gateway_execution;
#[path = "remote_feishu_gateway_input.rs"]
mod remote_feishu_gateway_input;

pub(super) const REMOTE_FEISHU_GATEWAY_PAIRING_ID: &str = "pairing-feishu";
const REMOTE_FEISHU_GATEWAY_DAEMON_DEFAULT_INTERVAL_MS: u64 = 1_000;
const REMOTE_FEISHU_GATEWAY_EVENT_REPORT_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuGatewayCliReport {
    pub status: &'static str,
    pub session_id: u64,
    pub transport: &'static str,
    pub input_events: usize,
    pub enqueued_events: usize,
    pub tick_count: usize,
    pub persisted_records: usize,
    pub pending_events: usize,
    pub active_chat_turns: usize,
    pub accepted_events: usize,
    pub duplicate_events: usize,
    pub rejected_events: usize,
    pub dispatch_planned_events: usize,
    pub dispatch_skipped_events: usize,
    pub agent_execution_required_events: usize,
    pub agent_executed_events: usize,
    pub processing_started_events: usize,
    pub processing_failed_events: usize,
    pub followup_queued_events: usize,
    pub followup_drained_events: usize,
    pub session_reset_events: usize,
    pub reply_enqueued_events: usize,
    pub agent_failed_events: usize,
    pub message_text_events: usize,
    pub message_command_events: usize,
    pub released_turns: usize,
    pub event_reports: Vec<RemoteFeishuGatewayCliEventReport>,
    pub network_send_allowed: bool,
    pub raw_payload_forward_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuGatewayDaemonReport {
    pub status: &'static str,
    pub session_id: u64,
    pub transport: &'static str,
    pub daemon: bool,
    pub event_source: &'static str,
    pub interval_ms: u64,
    pub max_ticks: Option<usize>,
    pub completed_ticks: usize,
    pub empty_ticks: usize,
    pub acked_offset: u64,
    pub cursor_file: String,
    pub input_events: usize,
    pub enqueued_events: usize,
    pub persisted_records: usize,
    pub accepted_events: usize,
    pub duplicate_events: usize,
    pub rejected_events: usize,
    pub dispatch_planned_events: usize,
    pub dispatch_skipped_events: usize,
    pub agent_execution_required_events: usize,
    pub agent_executed_events: usize,
    pub processing_started_events: usize,
    pub processing_failed_events: usize,
    pub followup_queued_events: usize,
    pub followup_drained_events: usize,
    pub session_reset_events: usize,
    pub reply_enqueued_events: usize,
    pub agent_failed_events: usize,
    pub message_text_events: usize,
    pub message_command_events: usize,
    pub event_reports: Vec<RemoteFeishuGatewayCliEventReport>,
    pub network_send_allowed: bool,
    pub raw_payload_forward_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuGatewayCliEventReport {
    pub request_id: String,
    pub chat_id_sha256: String,
    pub governance_status: &'static str,
    pub governance_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_kind: Option<&'static str>,
    pub dispatch_planned: bool,
    pub agent_execution_required: bool,
    pub agent_executed: bool,
    pub processing_started: bool,
    pub processing_failed: bool,
    pub followup_queued: bool,
    pub followup_drained: bool,
    pub session_reset: bool,
    pub reply_enqueued: bool,
    pub agent_failed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_run_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_session_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_dispatch_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_failure: Option<RuntimeFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_error_summary: Option<String>,
    pub turn_released: bool,
}

/// 单个 daemon tick：结算车道 → 落定连续确认前缀 → 有界读取并投递下一批。
///
/// 投递不等待车道完成，因此单个 chat 的慢网络不会阻塞其他 chat 的读取与治理；
/// 事件源本身就是背压载体，未确认区间在重启后按原顺序重放。
pub(super) fn execute_feishu_gateway_daemon(
    command: &RemoteGatewayCommand,
    scheduler: &GatewayLaneScheduler,
) -> Result<RemoteFeishuGatewayDaemonReport, String> {
    if command.event_json.is_some() {
        return Err("daemon shell 模式只支持 --event-file 事件源".to_owned());
    }
    let interval_ms = command
        .interval_ms
        .unwrap_or(REMOTE_FEISHU_GATEWAY_DAEMON_DEFAULT_INTERVAL_MS);
    if interval_ms == 0 {
        return Err("daemon shell interval-ms 必须大于 0".to_owned());
    }
    let Some(event_file) = command.event_file.as_ref() else {
        return Err("daemon shell 模式需要 --event-file".to_owned());
    };
    let cursor_file = resolve_cursor_file(command, event_file);
    let mut cursor = read_cursor_state(&cursor_file)?;
    let mut completed_ticks = 0usize;
    let mut empty_ticks = 0usize;
    let mut totals = GatewayDaemonTotals::default();
    loop {
        if command
            .max_ticks
            .is_some_and(|max_ticks| completed_ticks >= max_ticks)
        {
            break;
        }
        let completion = scheduler.take_completion();
        let mut dispatched = 0usize;
        let can_accept = scheduler.can_accept_batch();
        if can_accept {
            let (events, next_acked_offset) = read_gateway_input_events_after_cursor(
                event_file,
                scheduler.read_offset(),
                REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS,
            )?;
            if next_acked_offset > scheduler.read_offset() {
                dispatched += events.len();
                // 整批都是坏行时也要投递：否则读偏移与确认前缀会永久停在这里。
                scheduler.dispatch(next_acked_offset, events);
            }
        }
        if scheduler.note_read_window(!can_accept) {
            // 只在刚进入饱和时告警一次：持续饱和期间每个 tick 都告警会淹没有效日志。
            axon_logger::warning(&format!(
                "Feishu gateway 读窗口已满，入站读取已暂停直到最慢车道确认: unacked_lines={}, in_flight={}, active_lanes={}",
                completion.acked_lag_lines,
                scheduler.in_flight_events(),
                scheduler.active_lanes()
            ));
        }
        // 只有整批完成才推进 cursor；空洞会让未处理事件被跳过。
        if completion.acked_offset > cursor.acked_offset {
            cursor.acked_offset = completion.acked_offset;
            write_cursor_state(&cursor_file, &cursor)?;
            if compact_acknowledged_event_source(event_file, &cursor_file, &mut cursor) {
                scheduler.reset_after_source_compaction();
            }
        }
        if dispatched > 0 || completion.completed > 0 {
            // 车道观测走 debug：正常生命周期日志保持安静，排障时可单独打开。
            axon_logger::debug(&format!(
                "Feishu gateway tick: dispatched={dispatched}, completed={}, acked_lag_lines={}, lane_queue_max={}, lane_wait_max_ms={}, lane_task_max_ms={}, saturated_ticks={}, in_flight={}, active_lanes={}",
                completion.completed,
                completion.acked_lag_lines,
                completion.max_queue_depth,
                completion.max_wait_ms,
                completion.max_task_ms,
                completion.saturated_ticks,
                scheduler.in_flight_events(),
                scheduler.active_lanes()
            ));
        }
        let tick = build_gateway_cli_report(GatewayCliReportInput {
            session_id: command.session_id,
            input_events: dispatched,
            tick_count: completion.completed,
            persisted_records: completion.persisted_records,
            pending_events: scheduler.in_flight_events(),
            active_chat_turns: scheduler.active_lanes(),
            released_turns: completion.released_turns,
            kind_counts: message_kind_counts(&completion.event_reports),
            dispatch_counts: dispatch_counts(&completion.event_reports),
            event_reports: completion.event_reports,
        });
        if dispatched == 0 && completion.completed == 0 {
            empty_ticks += 1;
        }
        completed_ticks += 1;
        totals.accumulate(&tick);
        if command.max_ticks.is_none() {
            thread::sleep(Duration::from_millis(interval_ms));
        }
    }
    Ok(build_gateway_daemon_report(GatewayDaemonReportInput {
        command,
        interval_ms,
        completed_ticks,
        empty_ticks,
        acked_offset: cursor.acked_offset,
        cursor_file: cursor_file.to_string_lossy().to_string(),
        totals,
    }))
}

fn compact_acknowledged_event_source(
    event_file: &std::path::Path,
    cursor_file: &std::path::Path,
    cursor: &mut RemoteFeishuGatewayCursorState,
) -> bool {
    match reset_gateway_event_source_if_fully_acknowledged(event_file, cursor_file, cursor) {
        Ok(compacted) => compacted,
        Err(error) => {
            axon_logger::warning(&format!(
                "Feishu gateway 已确认事件源回收失败，将在后续 tick 重试: {error}"
            ));
            false
        }
    }
}

#[derive(Default)]
struct GatewayDaemonTotals {
    input_events: usize,
    enqueued_events: usize,
    persisted_records: usize,
    accepted_events: usize,
    duplicate_events: usize,
    rejected_events: usize,
    dispatch_planned_events: usize,
    dispatch_skipped_events: usize,
    agent_execution_required_events: usize,
    agent_executed_events: usize,
    processing_started_events: usize,
    processing_failed_events: usize,
    followup_queued_events: usize,
    followup_drained_events: usize,
    session_reset_events: usize,
    reply_enqueued_events: usize,
    agent_failed_events: usize,
    message_text_events: usize,
    message_command_events: usize,
    event_reports: Vec<RemoteFeishuGatewayCliEventReport>,
}

impl GatewayDaemonTotals {
    fn accumulate(&mut self, tick: &RemoteFeishuGatewayCliReport) {
        self.input_events += tick.input_events;
        self.enqueued_events += tick.enqueued_events;
        self.persisted_records += tick.persisted_records;
        self.accepted_events += tick.accepted_events;
        self.duplicate_events += tick.duplicate_events;
        self.rejected_events += tick.rejected_events;
        self.dispatch_planned_events += tick.dispatch_planned_events;
        self.dispatch_skipped_events += tick.dispatch_skipped_events;
        self.agent_execution_required_events += tick.agent_execution_required_events;
        self.agent_executed_events += tick.agent_executed_events;
        self.processing_started_events += tick.processing_started_events;
        self.processing_failed_events += tick.processing_failed_events;
        self.followup_queued_events += tick.followup_queued_events;
        self.followup_drained_events += tick.followup_drained_events;
        self.session_reset_events += tick.session_reset_events;
        self.reply_enqueued_events += tick.reply_enqueued_events;
        self.agent_failed_events += tick.agent_failed_events;
        self.message_text_events += tick.message_text_events;
        self.message_command_events += tick.message_command_events;
        self.event_reports
            .extend(tick.event_reports.iter().cloned());
        let overflow = self
            .event_reports
            .len()
            .saturating_sub(REMOTE_FEISHU_GATEWAY_EVENT_REPORT_CAPACITY);
        if overflow > 0 {
            self.event_reports.drain(..overflow);
        }
    }
}

struct GatewayDaemonReportInput<'a> {
    command: &'a RemoteGatewayCommand,
    interval_ms: u64,
    completed_ticks: usize,
    empty_ticks: usize,
    acked_offset: u64,
    cursor_file: String,
    totals: GatewayDaemonTotals,
}

fn build_gateway_daemon_report(
    input: GatewayDaemonReportInput<'_>,
) -> RemoteFeishuGatewayDaemonReport {
    RemoteFeishuGatewayDaemonReport {
        status: "remote_feishu_gateway_daemon_ready",
        session_id: input.command.session_id,
        transport: "feishu",
        daemon: true,
        event_source: "file",
        interval_ms: input.interval_ms,
        max_ticks: input.command.max_ticks,
        completed_ticks: input.completed_ticks,
        empty_ticks: input.empty_ticks,
        acked_offset: input.acked_offset,
        cursor_file: input.cursor_file,
        input_events: input.totals.input_events,
        enqueued_events: input.totals.enqueued_events,
        persisted_records: input.totals.persisted_records,
        accepted_events: input.totals.accepted_events,
        duplicate_events: input.totals.duplicate_events,
        rejected_events: input.totals.rejected_events,
        dispatch_planned_events: input.totals.dispatch_planned_events,
        dispatch_skipped_events: input.totals.dispatch_skipped_events,
        agent_execution_required_events: input.totals.agent_execution_required_events,
        agent_executed_events: input.totals.agent_executed_events,
        processing_started_events: input.totals.processing_started_events,
        processing_failed_events: input.totals.processing_failed_events,
        followup_queued_events: input.totals.followup_queued_events,
        followup_drained_events: input.totals.followup_drained_events,
        session_reset_events: input.totals.session_reset_events,
        reply_enqueued_events: input.totals.reply_enqueued_events,
        agent_failed_events: input.totals.agent_failed_events,
        message_text_events: input.totals.message_text_events,
        message_command_events: input.totals.message_command_events,
        event_reports: input.totals.event_reports,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
    }
}

pub(super) struct GatewayCliReportInput {
    pub(super) session_id: u64,
    pub(super) input_events: usize,
    pub(super) tick_count: usize,
    pub(super) persisted_records: usize,
    pub(super) pending_events: usize,
    pub(super) active_chat_turns: usize,
    pub(super) released_turns: usize,
    pub(super) event_reports: Vec<RemoteFeishuGatewayCliEventReport>,
    pub(super) kind_counts: MessageKindCounts,
    pub(super) dispatch_counts: DispatchCounts,
}

pub(super) fn build_gateway_cli_report(
    input: GatewayCliReportInput,
) -> RemoteFeishuGatewayCliReport {
    RemoteFeishuGatewayCliReport {
        status: "remote_feishu_gateway_tick_ready",
        session_id: input.session_id,
        transport: "feishu",
        input_events: input.input_events,
        enqueued_events: input.event_reports.len() + input.pending_events,
        tick_count: input.tick_count,
        persisted_records: input.persisted_records,
        pending_events: input.pending_events,
        active_chat_turns: input.active_chat_turns,
        accepted_events: status_count(&input.event_reports, "accepted"),
        duplicate_events: status_count(&input.event_reports, "duplicateDropped"),
        rejected_events: rejected_status_count(&input.event_reports),
        dispatch_planned_events: input.dispatch_counts.planned,
        dispatch_skipped_events: input.dispatch_counts.skipped,
        agent_execution_required_events: input.dispatch_counts.agent_execution_required,
        agent_executed_events: input.dispatch_counts.agent_executed,
        processing_started_events: input.dispatch_counts.processing_started,
        processing_failed_events: input.dispatch_counts.processing_failed,
        followup_queued_events: input.dispatch_counts.followup_queued,
        followup_drained_events: input.dispatch_counts.followup_drained,
        session_reset_events: input.dispatch_counts.session_reset,
        reply_enqueued_events: input.dispatch_counts.reply_enqueued,
        agent_failed_events: input.dispatch_counts.agent_failed,
        message_text_events: input.kind_counts.text_events,
        message_command_events: input.kind_counts.command_events,
        released_turns: input.released_turns,
        event_reports: input.event_reports,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
    }
}

fn status_count(reports: &[RemoteFeishuGatewayCliEventReport], status: &str) -> usize {
    reports
        .iter()
        .filter(|report| report.governance_status == status)
        .count()
}

fn rejected_status_count(reports: &[RemoteFeishuGatewayCliEventReport]) -> usize {
    reports
        .iter()
        .filter(|report| {
            matches!(
                report.governance_status,
                "rejectedByAllowlist"
                    | "rejectedByBlacklist"
                    | "rejectedByMentionGate"
                    | "ignoredSelfEvent"
            )
        })
        .count()
}
