use std::fmt::Write as _;

use crate::display_units::format_duration_millis_compact;
use crate::remote_commands::remote_feishu::{
    RemoteFeishuDeliveryReport, RemoteFeishuSimulationReport, RemoteFeishuStatusReport,
    RemoteFeishuWorklistReport, RemoteRenderReport,
};
use crate::remote_commands::remote_feishu_sdk_loop::RemoteFeishuSdkLoopDaemonReport;
use crate::{
    CliRunResult, OutputFormat,
    management_commands::json_result,
    remote_commands::{
        RemoteAuditReport, RemoteOutboxReport, RemoteRevokeReport, RemoteStatusReport,
        RemoteSubscriptionsReport,
    },
};

pub(super) fn render_status(format: OutputFormat, report: &RemoteStatusReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote: {}\n- subscriptions: total={} active={}\n- outbox: total={} pending={}\n- network_send_allowed: {}\n",
                report.status,
                report.total_subscription_records,
                report.active_subscription_records,
                report.total_outbox_records,
                report.pending_outbox_records,
                report.network_send_allowed,
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_subscriptions(
    format: OutputFormat,
    report: &RemoteSubscriptionsReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.subscriptions.is_empty() {
                stdout.push_str("未找到活跃远控订阅\n");
            } else {
                for subscription in &report.subscriptions {
                    let _ = writeln!(
                        stdout,
                        "session={}\ttransport={}\tkinds={}",
                        subscription.session_id,
                        subscription.transport,
                        subscription.kinds.join(","),
                    );
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

pub(super) fn render_outbox(format: OutputFormat, report: &RemoteOutboxReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.dispatches.is_empty() {
                stdout.push_str("未找到远控通知 outbox 记录\n");
            } else {
                for dispatch in &report.dispatches {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\t{}\tdeliveries={}",
                        dispatch.dispatch_id,
                        dispatch.transport,
                        dispatch.status,
                        dispatch.delivery_count
                    );
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

pub(super) fn render_revoke(format: OutputFormat, report: &RemoteRevokeReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Remote subscription revoked\n- session: {}\n- revoked: {}\n",
                report.subscription.session_id, report.revoked
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_audit(format: OutputFormat, report: &RemoteAuditReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote audit: {}\n- subscriptions: latest={} active={} revoked={}\n- outbox: total={} pending={}\n- network_send_allowed: {}\n",
                report.status,
                report.latest_subscription_records,
                report.active_subscription_records,
                report.revoked_subscription_records,
                report.total_outbox_records,
                report.pending_outbox_records,
                report.network_send_allowed
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_remote_render(
    format: OutputFormat,
    report: &RemoteRenderReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote render: {}\n- session: {}\n- adapter: {}\n- content_type: {}\n- payload_chars: {}\n",
                report.status,
                report.session_id,
                report.adapter,
                report.content_type,
                report.payload.chars().count()
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_remote_feishu_status(
    format: OutputFormat,
    report: &RemoteFeishuStatusReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote feishu-websocket: {}\n- enabled: {}\n- app_id_configured: {}\n- app_secret_env_configured: {}\n- allowed_sender_count: {}\n- require_mention: {}\n- bot_identity_hydrated: {}\n- bot_open_id: {}\n- bot_name: {}\n- websocket_url: {}\n- notification_sending_enabled: {}\n",
                report.status,
                report.enabled,
                report.app_id_configured,
                report.app_secret_env_configured,
                report.allowed_sender_count,
                report.require_mention,
                report.bot_identity_hydrated,
                report.bot_open_id.as_deref().unwrap_or(""),
                report.bot_name.as_deref().unwrap_or(""),
                report.websocket_url,
                report.notification_sending_enabled
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_remote_feishu_simulation(
    format: OutputFormat,
    report: &RemoteFeishuSimulationReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote feishu-simulate: {}\n- mode: {}\n- session: {}\n- governance_status: {}\n- payload_kind: {}\n- response_only: {}\n",
                report.status,
                report.mode,
                report.session_id,
                report.governance_status,
                report.payload_kind,
                report.response_only
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_remote_feishu_delivery(
    format: OutputFormat,
    report: &RemoteFeishuDeliveryReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote feishu-delivery: {}\n- session: {}\n- token_url: {}\n- message_url: {}\n- network_send_enabled: {}\n- executed: {}\n- agent_reply_local_media: {}\n- delivered_message_count: {}\n",
                report.status,
                report.session_id,
                report.token_url,
                report.message_url,
                report.network_send_enabled,
                report.executed,
                report.agent_reply_local_media,
                report.delivered_message_count
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_remote_feishu_worklist(
    format: OutputFormat,
    report: &RemoteFeishuWorklistReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote feishu-worklist: {}\n- total: {}\n- pending: {}\n- retryable: {}\n- governance_threads: {}\n- processing: {}\n- queued: {}\n- rejected: {}\n",
                report.status,
                report.total_records,
                report.pending_count,
                report.retryable_count,
                report.governance_threads,
                report.processing_threads,
                report.queued_threads,
                report.rejected_threads
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_remote_feishu_sdk_loop_daemon(
    format: OutputFormat,
    report: &RemoteFeishuSdkLoopDaemonReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon remote feishu-sdk-loop-daemon: {}\n- session: {}\n- mode: {}\n- websocket_url: {}\n- max_events: {:?}\n- wait: {}\n- stop_after_first_event: {}\n- completed_ticks: {}\n- empty_ticks: {}\n- acked_offset: {}\n- sdk_received_binary_frames: {}\n- sdk_received_data_frames: {}\n- sdk_skipped_data_frames: {}\n- input_payloads: {}\n- mapped: {}\n- appended: {}\n- received_acked: {}\n- received_ack_failed: {}\n- closure_gateway_input: {}\n- closure_agent_executed: {}\n- closure_reply_enqueued: {}\n- closure_session_reset: {}\n- closure_worker_delivered: {}\n- closure_feedback_completed: {}\n- closure_failed: {}\n- event_source: {}\n",
                report.status,
                report.session_id,
                report.sdk_connection_mode,
                report.websocket_url,
                report.max_events,
                report
                    .wait_ms
                    .map_or_else(|| "-".to_owned(), format_duration_millis_compact),
                report.stop_after_first_event,
                report.completed_ticks,
                report.empty_ticks,
                report.acked_offset,
                report.sdk_received_binary_frames,
                report.sdk_received_data_frames,
                report.sdk_skipped_data_frames,
                report.input_payloads,
                report.mapped_events,
                report.appended_events,
                report.received_acked_events,
                report.received_ack_failed_events,
                report.closure_gateway_input_events,
                report.closure_agent_executed_events,
                report.closure_reply_enqueued_events,
                report.closure_session_reset_events,
                report.closure_worker_delivered_count,
                report.closure_feedback_completed_count,
                report.closure_failed_events,
                report.event_source_file
            ),
            stderr: String::new(),
        },
    }
}
