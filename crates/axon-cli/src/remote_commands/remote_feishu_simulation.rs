use axon_domain::{RemoteCapabilityPolicy, SessionId};
use axon_remote_feishu::{
    RemoteFeishuCardActionEvent, RemoteFeishuChatGovernancePolicy, RemoteFeishuChatType,
    RemoteFeishuGovernanceConfig, RemoteFeishuGovernanceDecisionStatus,
    RemoteFeishuGovernanceRuntime, RemoteFeishuGovernanceStoreRuntime, RemoteFeishuIncomingMessage,
    RemoteFeishuIncomingMessageKind,
};
use axon_remote_runtime::RemoteAdapterPayloadKind;

use crate::{CliRunResult, OutputFormat, management_commands::cli_error};

use super::{
    RemoteCliContext, RemoteSimulateCommand, now_ms,
    remote_feishu::{RemoteFeishuSimulationOutcome, RemoteFeishuSimulationReport},
};

pub(super) fn run_remote_simulate_feishu(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteSimulateCommand,
) -> CliRunResult {
    let simulation = match build_feishu_simulation(context, command) {
        Ok(simulation) => simulation,
        Err(error) => return cli_error(error),
    };
    if let Err(error) = persist_feishu_simulation_governance(context, command) {
        return cli_error(error);
    }
    super::remote_render::render_remote_feishu_simulation(format, &simulation)
}

fn build_feishu_simulation(
    context: &RemoteCliContext,
    command: &RemoteSimulateCommand,
) -> Result<RemoteFeishuSimulationReport, String> {
    let outcome = match command.mode.as_str() {
        "message" => simulate_feishu_message(context, command)?,
        "card-action" => simulate_feishu_card_action(context, command)?,
        _ => return Err("simulate-feishu mode 仅支持 message 或 card-action".to_owned()),
    };
    let (report, governance_status, governance_reason) = match outcome {
        RemoteFeishuSimulationOutcome::Accepted {
            report,
            governance_status,
            governance_reason,
        } => (*report, governance_status, governance_reason),
        RemoteFeishuSimulationOutcome::Rejected(report) => return Ok(report),
    };
    let payload_kind = match report.adapter_dispatch.payload_kind {
        RemoteAdapterPayloadKind::Summary => "summary",
        RemoteAdapterPayloadKind::MessagePlan => "messagePlan",
        RemoteAdapterPayloadKind::DeliveryReport => "deliveryReport",
        RemoteAdapterPayloadKind::NotificationSubscription => "notificationSubscription",
        RemoteAdapterPayloadKind::NotificationBatch => "notificationBatch",
    };
    Ok(RemoteFeishuSimulationReport {
        status: "remote_feishu_simulation_ready",
        mode: command.mode.clone(),
        session_id: command.session_id,
        transport: "feishu",
        request_id: report.request_id,
        governance_status,
        governance_reason,
        payload_kind,
        response_only: report.adapter_dispatch.response_only,
        network_send_allowed: report.adapter_dispatch.network_send_allowed,
        raw_body_forward_allowed: report.adapter_dispatch.raw_body_forward_allowed,
    })
}

fn simulate_feishu_message(
    context: &RemoteCliContext,
    command: &RemoteSimulateCommand,
) -> Result<RemoteFeishuSimulationOutcome, String> {
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let existing_records = RemoteFeishuGovernanceStoreRuntime::read_records(
        RemoteFeishuGovernanceStoreRuntime::resolve_store_path(&context.root),
    )
    .map_err(|error| format!("{error:?}"))?;
    let governance_config = governance_config(context);
    let decision = governance
        .handle_message(
            &governance_config,
            &RemoteFeishuIncomingMessage {
                session_id: SessionId(command.session_id),
                client_id: "feishu-client".to_owned(),
                request_id: format!("sim-msg-{}", command.session_id),
                message_id: format!("om-sim-msg-{}", command.session_id),
                chat_id: format!("oc_{}", command.session_id),
                chat_type: RemoteFeishuChatType::Group,
                sender_open_id: command
                    .sender_open_id
                    .clone()
                    .unwrap_or_else(|| "ou_simulated".to_owned()),
                mentions: simulated_mentions(&governance_config),
                text: command.value.clone(),
                media_refs: Vec::new(),
                message_kind: RemoteFeishuIncomingMessageKind::from_text(&command.value),
                reply_context: None,
                session_busy: false,
            },
            "pairing-feishu",
            now_ms(),
            &existing_records,
        )
        .map_err(|error| format!("{error:?}"))?;
    let Some(report) = decision.dispatch_report else {
        return Ok(RemoteFeishuSimulationOutcome::Rejected(
            governance_rejected_simulation(command, "message", &decision),
        ));
    };
    Ok(RemoteFeishuSimulationOutcome::Accepted {
        report: Box::new(report),
        governance_status: governance_status_code(decision.status),
        governance_reason: decision.reason,
    })
}

fn simulate_feishu_card_action(
    context: &RemoteCliContext,
    command: &RemoteSimulateCommand,
) -> Result<RemoteFeishuSimulationOutcome, String> {
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let existing_records = RemoteFeishuGovernanceStoreRuntime::read_records(
        RemoteFeishuGovernanceStoreRuntime::resolve_store_path(&context.root),
    )
    .map_err(|error| format!("{error:?}"))?;
    let governance_config = governance_config(context);
    let decision = governance
        .handle_card_action(
            &governance_config,
            &card_action_event(command),
            "pairing-feishu",
            now_ms(),
            &existing_records,
        )
        .map_err(|error| format!("{error:?}"))?;
    let Some(report) = decision.dispatch_report else {
        return Ok(RemoteFeishuSimulationOutcome::Rejected(
            governance_rejected_simulation(command, "card-action", &decision),
        ));
    };
    Ok(RemoteFeishuSimulationOutcome::Accepted {
        report: Box::new(report),
        governance_status: governance_status_code(decision.status),
        governance_reason: decision.reason,
    })
}

fn governance_rejected_simulation(
    command: &RemoteSimulateCommand,
    mode: &str,
    decision: &axon_remote_feishu::RemoteFeishuGovernanceDecision,
) -> RemoteFeishuSimulationReport {
    RemoteFeishuSimulationReport {
        status: "remote_feishu_simulation_ready",
        mode: mode.to_owned(),
        session_id: command.session_id,
        transport: "feishu",
        request_id: "governance-rejected".to_owned(),
        governance_status: governance_status_code(decision.status),
        governance_reason: decision.reason.clone(),
        payload_kind: "messagePlan",
        response_only: true,
        network_send_allowed: false,
        raw_body_forward_allowed: false,
    }
}

fn persist_feishu_simulation_governance(
    context: &RemoteCliContext,
    command: &RemoteSimulateCommand,
) -> Result<(), String> {
    let governance = RemoteFeishuGovernanceRuntime::new(RemoteCapabilityPolicy::default_sidecar());
    let store_path = RemoteFeishuGovernanceStoreRuntime::resolve_store_path(&context.root);
    let existing_records = RemoteFeishuGovernanceStoreRuntime::read_records(&store_path)
        .map_err(|error| format!("{error:?}"))?;
    let now_ms = now_ms();
    let record = match command.mode.as_str() {
        "message" => {
            let message_governance_config = governance_config(context);
            let decision = governance
                .handle_message(
                    &message_governance_config,
                    &RemoteFeishuIncomingMessage {
                        session_id: SessionId(command.session_id),
                        client_id: "feishu-client".to_owned(),
                        request_id: format!("sim-msg-{}", command.session_id),
                        message_id: format!("om-sim-msg-{}", command.session_id),
                        chat_id: format!("oc_{}", command.session_id),
                        chat_type: RemoteFeishuChatType::Group,
                        sender_open_id: command
                            .sender_open_id
                            .clone()
                            .unwrap_or_else(|| "ou_simulated".to_owned()),
                        mentions: simulated_mentions(&message_governance_config),
                        text: command.value.clone(),
                        media_refs: Vec::new(),
                        message_kind: RemoteFeishuIncomingMessageKind::from_text(&command.value),
                        reply_context: None,
                        session_busy: false,
                    },
                    "pairing-feishu",
                    now_ms,
                    &existing_records,
                )
                .map_err(|error| format!("{error:?}"))?;
            decision.record
        }
        "card-action" => {
            let decision = governance
                .handle_card_action(
                    &governance_config(context),
                    &card_action_event(command),
                    "pairing-feishu",
                    now_ms,
                    &existing_records,
                )
                .map_err(|error| format!("{error:?}"))?;
            decision.record
        }
        _ => return Ok(()),
    };
    RemoteFeishuGovernanceStoreRuntime::append_record(store_path, &record)
        .map_err(|error| format!("{error:?}"))
}

const fn governance_status_code(status: RemoteFeishuGovernanceDecisionStatus) -> &'static str {
    match status {
        RemoteFeishuGovernanceDecisionStatus::Accepted => "accepted",
        RemoteFeishuGovernanceDecisionStatus::DuplicateDropped => "duplicateDropped",
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist => "rejectedByAllowlist",
        RemoteFeishuGovernanceDecisionStatus::RejectedByBlacklist => "rejectedByBlacklist",
        RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate => "rejectedByMentionGate",
        RemoteFeishuGovernanceDecisionStatus::IgnoredSelfEvent => "ignoredSelfEvent",
    }
}

fn simulated_mentions(config: &RemoteFeishuGovernanceConfig) -> Vec<String> {
    if !config.require_mention {
        return Vec::new();
    }
    vec![
        config
            .bot_open_id
            .clone()
            .unwrap_or_else(|| "ou_simulated_bot".to_owned()),
    ]
}

fn governance_config(context: &RemoteCliContext) -> RemoteFeishuGovernanceConfig {
    RemoteFeishuGovernanceConfig {
        allowed_sender_open_ids: context.feishu_websocket.allowed_sender_open_ids.clone(),
        blocked_sender_open_ids: context.feishu_websocket.blocked_sender_open_ids.clone(),
        require_mention: context.feishu_websocket.require_mention,
        bot_open_id: super::remote_feishu_identity::resolve_feishu_bot_identity(context)
            .map(|identity| identity.bot_open_id),
        chat_policies: context
            .feishu_websocket
            .chat_policies
            .iter()
            .map(|policy| RemoteFeishuChatGovernancePolicy {
                chat_id: policy.chat_id.clone(),
                allowed_sender_open_ids: policy.allowed_sender_open_ids.clone(),
                blocked_sender_open_ids: policy.blocked_sender_open_ids.clone(),
                require_mention: policy.require_mention,
            })
            .collect(),
    }
}

fn card_action_event(command: &RemoteSimulateCommand) -> RemoteFeishuCardActionEvent {
    RemoteFeishuCardActionEvent {
        session_id: SessionId(command.session_id),
        client_id: "feishu-client".to_owned(),
        request_id: format!("sim-card-{}", command.session_id),
        message_id: None,
        chat_id: format!("oc_{}", command.session_id),
        operator_open_id: command
            .sender_open_id
            .clone()
            .unwrap_or_else(|| "ou_simulated".to_owned()),
        action_tag: command.value.clone(),
        action_value_json: command
            .action_json
            .clone()
            .unwrap_or_else(|| "{}".to_owned()),
        reply_context: None,
        session_busy: true,
    }
}
