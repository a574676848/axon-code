//! Feishu gateway 入站事件与治理决策的纯投影。
//!
//! 本模块只做「入站结构 → 域模型」的无副作用映射：不持有运行态，也不发起网络
//! 或磁盘 I/O。gateway worker、事件执行与报告层共用同一份归一化规则，避免同一
//! 条入站语义在多处漂移。

use axon_domain::SessionId;
use axon_remote_feishu::{
    RemoteFeishuCardActionEvent, RemoteFeishuChatType, RemoteFeishuGatewayGovernanceEventReport,
    RemoteFeishuGatewayInboundEvent, RemoteFeishuGovernanceDecisionStatus,
    RemoteFeishuIncomingMessage, RemoteFeishuIncomingMessageKind, RemoteFeishuMediaKind,
    RemoteFeishuMediaRef, RemoteFeishuReplyContext,
};

use super::super::RemoteFeishuGatewayCliEventReport;
use super::super::remote_feishu_gateway_input::{
    FeishuGatewayInputEvent, RemoteFeishuGatewayReplyContextInput,
};
use super::remote_feishu_gateway_action::GatewayAgentExecutionProjection;

const REMOTE_FEISHU_GATEWAY_CLIENT_ID: &str = "feishu-gateway";

pub(super) fn to_gateway_event(
    session_id: u64,
    input: &FeishuGatewayInputEvent,
) -> Result<RemoteFeishuGatewayInboundEvent, String> {
    match input.kind.as_str() {
        "message" => Ok(RemoteFeishuGatewayInboundEvent::Message(
            RemoteFeishuIncomingMessage {
                session_id: SessionId(session_id),
                client_id: REMOTE_FEISHU_GATEWAY_CLIENT_ID.to_owned(),
                request_id: input.request_id.clone(),
                message_id: input
                    .message_id
                    .clone()
                    .unwrap_or_else(|| format!("om_{}", input.request_id)),
                chat_id: input.chat_id.clone(),
                chat_type: chat_type(input)?,
                sender_open_id: input
                    .sender_open_id
                    .clone()
                    .ok_or_else(|| "message 事件缺少 senderOpenId".to_owned())?,
                mentions: input.mentions.clone(),
                text: input
                    .text
                    .clone()
                    .ok_or_else(|| "message 事件缺少 text".to_owned())?,
                media_refs: media_refs(input)?,
                message_kind: message_kind(input)?,
                reply_context: reply_context(input.reply_context.as_ref()),
                session_busy: input.session_busy,
            },
        )),
        "cardAction" | "card-action" => Ok(RemoteFeishuGatewayInboundEvent::CardAction(
            RemoteFeishuCardActionEvent {
                session_id: SessionId(session_id),
                client_id: REMOTE_FEISHU_GATEWAY_CLIENT_ID.to_owned(),
                request_id: input.request_id.clone(),
                message_id: input.message_id.clone(),
                chat_id: input.chat_id.clone(),
                operator_open_id: input
                    .operator_open_id
                    .clone()
                    .or_else(|| input.sender_open_id.clone())
                    .ok_or_else(|| "cardAction 事件缺少 operatorOpenId".to_owned())?,
                action_tag: input
                    .action_tag
                    .clone()
                    .ok_or_else(|| "cardAction 事件缺少 actionTag".to_owned())?,
                action_value_json: input
                    .action_value_json
                    .clone()
                    .unwrap_or_else(|| "{}".to_owned()),
                reply_context: reply_context(input.reply_context.as_ref()),
                session_busy: input.session_busy,
            },
        )),
        _ => Err("Feishu gateway event.kind 仅支持 message 或 cardAction".to_owned()),
    }
}

fn media_refs(input: &FeishuGatewayInputEvent) -> Result<Vec<RemoteFeishuMediaRef>, String> {
    input
        .media_refs
        .iter()
        .map(|media| {
            let kind = match media.kind.as_str() {
                "image" => RemoteFeishuMediaKind::Image,
                "file" => RemoteFeishuMediaKind::File,
                "audio" => RemoteFeishuMediaKind::Audio,
                "media" => RemoteFeishuMediaKind::Media,
                "sticker" => RemoteFeishuMediaKind::Sticker,
                _ => {
                    return Err(
                        "mediaRefs.kind 仅支持 image、file、audio、media 或 sticker".to_owned()
                    );
                }
            };
            if media.key.trim().is_empty() {
                return Err("mediaRefs.key 不能为空".to_owned());
            }
            Ok(RemoteFeishuMediaRef {
                kind,
                key: media.key.clone(),
                file_name: media.file_name.clone(),
            })
        })
        .collect()
}

pub(super) fn reply_context(
    input: Option<&RemoteFeishuGatewayReplyContextInput>,
) -> Option<RemoteFeishuReplyContext> {
    input.map(|context| RemoteFeishuReplyContext {
        reply_to_message_id: context.reply_to_message_id.clone(),
        reply_to_text: context.reply_to_text.clone(),
        thread_id: context.thread_id.clone(),
    })
}

pub(super) fn message_kind(
    input: &FeishuGatewayInputEvent,
) -> Result<RemoteFeishuIncomingMessageKind, String> {
    match input.message_kind.as_deref() {
        Some("text") => Ok(RemoteFeishuIncomingMessageKind::Text),
        Some("command") => Ok(RemoteFeishuIncomingMessageKind::Command),
        Some(_) => Err("messageKind 仅支持 text 或 command".to_owned()),
        None => Ok(RemoteFeishuIncomingMessageKind::from_text(
            input.text.as_deref().unwrap_or_default(),
        )),
    }
}

fn chat_type(input: &FeishuGatewayInputEvent) -> Result<RemoteFeishuChatType, String> {
    RemoteFeishuChatType::from_stable_value(input.chat_type.as_deref())
        .ok_or_else(|| "chatType 仅支持 p2p、group 或 unknown".to_owned())
}

pub(super) fn gateway_event_report(
    event_report: RemoteFeishuGatewayGovernanceEventReport,
    execution: GatewayAgentExecutionProjection,
    turn_released: bool,
) -> RemoteFeishuGatewayCliEventReport {
    RemoteFeishuGatewayCliEventReport {
        request_id: event_report.meta.request_id,
        chat_id_sha256: event_report.meta.chat_id_sha256,
        governance_status: governance_status_code(event_report.decision.status),
        governance_reason: event_report.decision.reason,
        message_kind: event_report
            .meta
            .message_kind
            .map(RemoteFeishuIncomingMessageKind::as_str),
        dispatch_planned: event_report.decision.dispatch_report.is_some(),
        agent_execution_required: execution.required,
        agent_executed: execution.executed,
        processing_started: execution.processing_started,
        processing_failed: execution.processing_failed,
        followup_queued: execution.followup_queued,
        followup_drained: false,
        session_reset: execution.session_reset,
        reply_enqueued: execution.reply_enqueued,
        agent_failed: execution.failed,
        agent_run_session_id: execution.run_session_id,
        run_session_key: execution.run_session_key,
        reply_dispatch_id: execution.reply_dispatch_id,
        agent_failure: execution.failure,
        agent_error_summary: execution.error_summary,
        turn_released,
    }
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

/// 治理报告里的回复线程 ID；控制回执需要沿同一线程回复。
pub(super) fn reply_thread_id(report: &RemoteFeishuGatewayGovernanceEventReport) -> Option<String> {
    report
        .meta
        .reply_context
        .as_ref()
        .and_then(|context| context.thread_id.clone())
}

/// 治理报告里的被回复消息 ID；控制回执需要挂在同一条消息上。
pub(super) fn reply_message_id(
    report: &RemoteFeishuGatewayGovernanceEventReport,
) -> Option<String> {
    report
        .meta
        .reply_context
        .as_ref()
        .and_then(|context| context.reply_to_message_id.clone())
}
