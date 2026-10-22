use axon_domain::SessionId;
use axon_remote_feishu::{
    REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL, RemoteFeishuWebSocketAdapterConfig,
    RemoteFeishuWebSocketAdapterRuntime,
};
use axon_remote_runtime::{
    RemoteNotificationAdapterDispatchBatch, RemoteNotificationAdapterDispatchItem,
    RemoteNotificationAdapterEnvelopeRuntime, RemoteNotificationAdapterSendStatus,
    RemoteNotificationCardRuntime, RemoteNotificationOutboxRuntime, RemoteTransportDispatchReport,
    RemoteTransportKind,
};
use serde::Serialize;

use crate::{CliRunResult, OutputFormat, management_commands::cli_error};

use super::{RemoteCliContext, RemoteRenderCommand, bounded_limit, now_ms};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteRenderReport {
    pub status: &'static str,
    pub session_id: u64,
    pub transport: &'static str,
    pub limit: usize,
    pub pending_records: usize,
    pub adapter: &'static str,
    pub content_type: String,
    pub payload: String,
    pub network_send_allowed: bool,
    pub raw_payload_forward_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuStatusReport {
    pub status: &'static str,
    pub enabled: bool,
    pub app_id_configured: bool,
    pub app_secret_env_configured: bool,
    pub allowed_sender_count: usize,
    pub require_mention: bool,
    pub bot_identity_hydrated: bool,
    pub bot_open_id: Option<String>,
    pub bot_name: Option<String>,
    pub websocket_url: String,
    pub using_default_websocket_url: bool,
    pub notification_sending_enabled: bool,
    pub network_send_allowed: bool,
    pub raw_payload_forward_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuSimulationReport {
    pub status: &'static str,
    pub mode: String,
    pub session_id: u64,
    pub transport: &'static str,
    pub request_id: String,
    pub governance_status: &'static str,
    pub governance_reason: String,
    pub payload_kind: &'static str,
    pub response_only: bool,
    pub network_send_allowed: bool,
    pub raw_body_forward_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuDeliveryReport {
    pub status: &'static str,
    pub session_id: u64,
    pub token_url: String,
    pub message_url: String,
    pub message_method: String,
    pub network_send_enabled: bool,
    pub executed: bool,
    pub agent_reply_local_media: bool,
    pub delivered_message_count: usize,
    pub feedback_status: Option<String>,
    pub adapter_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuWorklistItem {
    pub chat_id_sha256: String,
    pub dispatch_id: String,
    pub session_id: u64,
    pub status: &'static str,
    pub governance_status: Option<&'static str>,
    pub governance_reason: Option<String>,
    pub text_preview: Option<String>,
    pub mentioned: Option<bool>,
    pub retryable: bool,
    pub next_retry_after_ms: Option<u64>,
    pub has_adapter_message_id: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuWorklistReport {
    pub status: &'static str,
    pub total_records: usize,
    pub pending_count: usize,
    pub retryable_count: usize,
    pub governance_threads: usize,
    pub processing_threads: usize,
    pub queued_threads: usize,
    pub rejected_threads: usize,
    pub recent_dispatches: Vec<RemoteFeishuWorklistItem>,
}

pub(super) enum RemoteFeishuSimulationOutcome {
    Accepted {
        report: Box<RemoteTransportDispatchReport>,
        governance_status: &'static str,
        governance_reason: String,
    },
    Rejected(RemoteFeishuSimulationReport),
}

pub(super) fn run_remote_feishu_status(
    format: OutputFormat,
    context: &RemoteCliContext,
) -> CliRunResult {
    let websocket_url = if context.feishu_websocket.websocket_url.trim().is_empty() {
        REMOTE_FEISHU_WEBSOCKET_DEFAULT_URL.to_owned()
    } else {
        context.feishu_websocket.websocket_url.clone()
    };
    let identity = super::remote_feishu_identity::resolve_feishu_bot_identity(context);
    let report = RemoteFeishuStatusReport {
        status: if context.feishu_websocket.enabled {
            "remote_feishu_websocket_ready"
        } else {
            "remote_feishu_websocket_disabled"
        },
        enabled: context.feishu_websocket.enabled,
        app_id_configured: !context.feishu_websocket.app_id.trim().is_empty(),
        app_secret_env_configured: !context.feishu_websocket.app_secret_env.trim().is_empty(),
        allowed_sender_count: context.feishu_websocket.allowed_sender_open_ids.len(),
        require_mention: context.feishu_websocket.require_mention,
        bot_identity_hydrated: identity.is_some(),
        bot_open_id: identity
            .as_ref()
            .map(|identity| identity.bot_open_id.clone()),
        bot_name: identity.as_ref().and_then(|identity| {
            (!identity.bot_name.trim().is_empty()).then(|| identity.bot_name.clone())
        }),
        websocket_url,
        using_default_websocket_url: context.feishu_websocket.websocket_url.trim().is_empty(),
        notification_sending_enabled: context.feishu_websocket.enable_notification_sending,
        network_send_allowed: context.feishu_websocket.enable_notification_sending,
        raw_payload_forward_allowed: false,
    };
    super::remote_render::render_remote_feishu_status(format, &report)
}

pub(super) fn run_remote_render_feishu(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteRenderCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let now_ms = now_ms();
    let batch = match RemoteNotificationOutboxRuntime::pending_for_transport(
        &context.outbox_store_path,
        SessionId(command.session_id),
        RemoteTransportKind::Feishu,
        now_ms,
    ) {
        Ok(batch) => batch,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let Some(record) = batch.records.into_iter().take(limit).next() else {
        return cli_error("未找到可渲染的 Feishu pending outbox 记录");
    };
    let envelope = match RemoteNotificationAdapterEnvelopeRuntime::project_batch(
        &RemoteNotificationAdapterDispatchBatch {
            session_id: SessionId(record.session_id),
            transport: record.transport,
            items: vec![RemoteNotificationAdapterDispatchItem {
                dispatch_id: record.dispatch_id.clone(),
                subscriber_id: record.subscriber_id.clone(),
                connection_id: record.connection_id.clone(),
                pairing_id: record.pairing_id.clone(),
                delivery_count: record.delivery_count,
                delivery_kinds: record.delivery_kinds.clone(),
                attempt_count: record.attempt_count,
            }],
            ack_records: Vec::new(),
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: Vec::new(),
        },
    ) {
        Ok(report) => report.envelopes.into_iter().next(),
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let Some(envelope) = envelope else {
        return cli_error("无法生成 Feishu 渲染 envelope");
    };
    let card = match RemoteNotificationCardRuntime::project(&envelope) {
        Ok(card) => card,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let dispatch_plan = match RemoteFeishuWebSocketAdapterRuntime::plan_dispatch(
        &RemoteFeishuWebSocketAdapterConfig {
            enabled: context.feishu_websocket.enabled,
            app_id: context.feishu_websocket.app_id.clone(),
            app_secret_env: context.feishu_websocket.app_secret_env.clone(),
            websocket_url: (!context.feishu_websocket.websocket_url.trim().is_empty())
                .then_some(context.feishu_websocket.websocket_url.clone()),
            enable_notification_sending: context.feishu_websocket.enable_notification_sending,
        },
        &card,
    ) {
        Ok(plan) => plan,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let report = RemoteRenderReport {
        status: "remote_render_ready",
        session_id: command.session_id,
        transport: "feishu",
        limit,
        pending_records: 1,
        adapter: "feishuWebSocket",
        content_type: dispatch_plan.artifact.content_type,
        payload: dispatch_plan.artifact.payload,
        network_send_allowed: dispatch_plan.notification_sending_enabled,
        raw_payload_forward_allowed: false,
    };
    super::remote_render::render_remote_render(format, &report)
}

pub(super) const fn feishu_feedback_status(
    status: RemoteNotificationAdapterSendStatus,
) -> &'static str {
    match status {
        RemoteNotificationAdapterSendStatus::Delivered => "delivered",
        RemoteNotificationAdapterSendStatus::RetryableFailure => "failedRetryable",
        RemoteNotificationAdapterSendStatus::PermanentFailure => "failedPermanent",
    }
}
