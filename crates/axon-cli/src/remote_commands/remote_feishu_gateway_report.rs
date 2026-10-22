use axon_remote_feishu::RemoteFeishuIncomingMessageKind;

use super::remote_feishu_gateway::RemoteFeishuGatewayCliEventReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MessageKindCounts {
    pub(super) text_events: usize,
    pub(super) command_events: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DispatchCounts {
    pub(super) planned: usize,
    pub(super) skipped: usize,
    pub(super) agent_execution_required: usize,
    pub(super) agent_executed: usize,
    pub(super) processing_started: usize,
    pub(super) processing_failed: usize,
    pub(super) followup_queued: usize,
    pub(super) followup_drained: usize,
    pub(super) session_reset: usize,
    pub(super) reply_enqueued: usize,
    pub(super) agent_failed: usize,
}

pub(super) fn message_kind_counts(
    reports: &[RemoteFeishuGatewayCliEventReport],
) -> MessageKindCounts {
    let mut counts = MessageKindCounts {
        text_events: 0,
        command_events: 0,
    };
    for report in reports {
        match report.message_kind {
            Some(kind) if kind == RemoteFeishuIncomingMessageKind::Text.as_str() => {
                counts.text_events += 1;
            }
            Some(kind) if kind == RemoteFeishuIncomingMessageKind::Command.as_str() => {
                counts.command_events += 1;
            }
            _ => {}
        }
    }
    counts
}

pub(super) fn dispatch_counts(reports: &[RemoteFeishuGatewayCliEventReport]) -> DispatchCounts {
    let mut counts = DispatchCounts {
        planned: 0,
        skipped: 0,
        agent_execution_required: 0,
        agent_executed: 0,
        processing_started: 0,
        processing_failed: 0,
        followup_queued: 0,
        followup_drained: 0,
        session_reset: 0,
        reply_enqueued: 0,
        agent_failed: 0,
    };
    for report in reports {
        if report.dispatch_planned {
            counts.planned += 1;
        } else {
            counts.skipped += 1;
        }
        if report.agent_execution_required {
            counts.agent_execution_required += 1;
        }
        if report.agent_executed {
            counts.agent_executed += 1;
        }
        if report.processing_started {
            counts.processing_started += 1;
        }
        if report.processing_failed {
            counts.processing_failed += 1;
        }
        if report.followup_queued {
            counts.followup_queued += 1;
        }
        if report.followup_drained {
            counts.followup_drained += 1;
        }
        if report.session_reset {
            counts.session_reset += 1;
        }
        if report.reply_enqueued {
            counts.reply_enqueued += 1;
        }
        if report.agent_failed {
            counts.agent_failed += 1;
        }
    }
    counts
}
