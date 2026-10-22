use axon_agent_runtime::AgentReplyAttachmentRuntime;
use axon_domain::SessionId;
use axon_remote_feishu::{
    RemoteFeishuAgentReplyArtifactRecord, RemoteFeishuAgentReplyArtifactRuntime,
    RemoteFeishuDeliveryRuntime, RemoteFeishuFeedbackTaskStoreRuntime,
    RemoteFeishuWebSocketAdapterConfig, RemoteFeishuWebSocketAdapterRuntime,
};
use axon_remote_runtime::{
    RemoteNotificationAdapterDispatchBatch, RemoteNotificationAdapterDispatchItem,
    RemoteNotificationAdapterEnvelopeRuntime, RemoteNotificationCardRuntime,
    RemoteNotificationOutboxRecord, RemoteNotificationOutboxRuntime, RemoteTransportKind,
};

use crate::{CliRunResult, OutputFormat, management_commands::cli_error};

use super::{
    RemoteCliContext, RemoteRenderCommand, bounded_limit, now_ms,
    remote_feishu::{RemoteFeishuDeliveryReport, feishu_feedback_status},
};

pub(super) fn run_remote_deliver_feishu(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &RemoteRenderCommand,
) -> CliRunResult {
    let (record, dispatch_plan, agent_reply, delivery) =
        match build_feishu_delivery_context(context, command) {
            Ok(context) => context,
            Err(error) => return cli_error(error),
        };
    let mut report = RemoteFeishuDeliveryReport {
        status: "remote_feishu_delivery_ready",
        session_id: command.session_id,
        token_url: delivery.token_request.url,
        message_url: delivery.message_request.url,
        message_method: delivery.message_request.method,
        network_send_enabled: delivery.network_send_enabled,
        executed: false,
        agent_reply_local_media: agent_reply
            .as_ref()
            .is_some_and(agent_reply_contains_local_file),
        delivered_message_count: 0,
        feedback_status: None,
        adapter_message_id: None,
    };
    if command.execute
        && let Err(error) = execute_feishu_delivery(
            context,
            command,
            &record,
            &dispatch_plan,
            agent_reply.as_ref(),
            &mut report,
        )
    {
        return cli_error(error);
    }
    super::remote_render::render_remote_feishu_delivery(format, &report)
}

fn build_feishu_delivery_context(
    context: &RemoteCliContext,
    command: &RemoteRenderCommand,
) -> Result<
    (
        axon_remote_runtime::RemoteNotificationOutboxRecord,
        axon_remote_feishu::RemoteFeishuWebSocketDispatchPlan,
        Option<RemoteFeishuAgentReplyArtifactRecord>,
        axon_remote_feishu::RemoteFeishuDeliveryPlan,
    ),
    String,
> {
    let limit = bounded_limit(command.limit)?;
    let now_ms = now_ms();
    let batch = RemoteNotificationOutboxRuntime::pending_for_transport(
        &context.outbox_store_path,
        SessionId(command.session_id),
        RemoteTransportKind::Feishu,
        now_ms,
    )
    .map_err(|error| format!("{error:?}"))?;
    let record = batch
        .records
        .into_iter()
        .take(limit)
        .next()
        .ok_or_else(|| "未找到可投递的 Feishu pending outbox 记录".to_owned())?;
    let dispatch_plan = project_feishu_dispatch_plan(context, &record)?;
    let agent_reply = resolve_agent_reply_artifact(context, &record)?;
    let delivery = if let Some(reply) = agent_reply.as_ref() {
        RemoteFeishuDeliveryRuntime::plan_agent_reply_requests(
            &dispatch_plan,
            feishu_api_base_url(context),
            &record,
            reply,
        )
        .map_err(|error| format!("{error:?}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "Feishu Agent 回复分段计划为空".to_owned())?
    } else {
        RemoteFeishuDeliveryRuntime::plan_requests(
            &dispatch_plan,
            feishu_api_base_url(context),
            &record,
        )
        .map_err(|error| format!("{error:?}"))?
    };
    Ok((record, dispatch_plan, agent_reply, delivery))
}

pub(super) fn project_feishu_dispatch_plan(
    context: &RemoteCliContext,
    record: &RemoteNotificationOutboxRecord,
) -> Result<axon_remote_feishu::RemoteFeishuWebSocketDispatchPlan, String> {
    let envelope = RemoteNotificationAdapterEnvelopeRuntime::project_batch(
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
    )
    .map_err(|error| format!("{error:?}"))?
    .envelopes
    .into_iter()
    .next()
    .ok_or_else(|| "无法生成 Feishu delivery envelope".to_owned())?;
    let card =
        RemoteNotificationCardRuntime::project(&envelope).map_err(|error| format!("{error:?}"))?;
    RemoteFeishuWebSocketAdapterRuntime::plan_dispatch(
        &RemoteFeishuWebSocketAdapterConfig {
            enabled: context.feishu_websocket.enabled,
            app_id: context.feishu_websocket.app_id.clone(),
            app_secret_env: context.feishu_websocket.app_secret_env.clone(),
            websocket_url: (!context.feishu_websocket.websocket_url.trim().is_empty())
                .then_some(context.feishu_websocket.websocket_url.clone()),
            enable_notification_sending: context.feishu_websocket.enable_notification_sending,
        },
        &card,
    )
    .map_err(|error| format!("{error:?}"))
}

fn execute_feishu_delivery(
    context: &RemoteCliContext,
    command: &RemoteRenderCommand,
    record: &RemoteNotificationOutboxRecord,
    dispatch_plan: &axon_remote_feishu::RemoteFeishuWebSocketDispatchPlan,
    agent_reply: Option<&RemoteFeishuAgentReplyArtifactRecord>,
    report: &mut RemoteFeishuDeliveryReport,
) -> Result<(), String> {
    let sent_at_ms = now_ms();
    // 正文与附件在同一契约下出站：可见正文始终投递，附件按真实存在的文件追加。
    let execution = match (agent_reply, command.app_secret.as_deref()) {
        (Some(reply), Some(secret)) => {
            RemoteFeishuDeliveryRuntime::execute_agent_reply_delivery_with_secret(
                dispatch_plan,
                secret,
                feishu_api_base_url(context),
                record,
                reply,
                sent_at_ms,
            )
        }
        (Some(reply), None) => RemoteFeishuDeliveryRuntime::execute_agent_reply_delivery(
            dispatch_plan,
            feishu_api_base_url(context),
            record,
            reply,
            sent_at_ms,
        ),
        (None, Some(secret)) => RemoteFeishuDeliveryRuntime::execute_with_secret(
            dispatch_plan,
            secret,
            feishu_api_base_url(context),
            record,
            sent_at_ms,
        ),
        (None, None) => RemoteFeishuDeliveryRuntime::execute(
            dispatch_plan,
            feishu_api_base_url(context),
            record,
            sent_at_ms,
        ),
    }
    .map_err(|error| format!("{error:?}"))?;
    for feedback_record in &execution.feedback.records {
        RemoteNotificationOutboxRuntime::append_record(&context.outbox_store_path, feedback_record)
            .map_err(|error| format!("{error:?}"))?;
    }
    if let Some(task) = RemoteFeishuFeedbackTaskStoreRuntime::task_from_delivery(
        dispatch_plan,
        &execution,
        agent_reply.and_then(|reply| reply.processing_message_id.as_deref()),
        feishu_api_base_url(context),
        sent_at_ms,
    ) {
        RemoteFeishuFeedbackTaskStoreRuntime::append_record(
            RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&context.root),
            &task,
        )
        .map_err(|error| format!("{error:?}"))?;
    }
    report.executed = true;
    report.feedback_status = Some(feishu_feedback_status(execution.send_status).to_owned());
    report.adapter_message_id = execution.adapter_message_id;
    report.delivered_message_count = execution.feedback.delivered_count;
    Ok(())
}

/// 回复是否包含**确实存在**的可投递本地文件。
///
/// 判定与执行必须共用同一存在性契约：只看 `MEDIA:` 行首会导致路径非法时整条回复
/// 在附件分支失败，用户连正文都收不到。
pub(super) fn agent_reply_contains_local_file(
    reply: &RemoteFeishuAgentReplyArtifactRecord,
) -> bool {
    !AgentReplyAttachmentRuntime::existing_files(&reply.body).is_empty()
}

pub(super) fn feishu_api_base_url(context: &RemoteCliContext) -> Option<&str> {
    (!context.feishu_websocket.api_base_url.trim().is_empty())
        .then_some(context.feishu_websocket.api_base_url.as_str())
}

pub(super) fn resolve_agent_reply_artifact(
    context: &RemoteCliContext,
    record: &RemoteNotificationOutboxRecord,
) -> Result<Option<RemoteFeishuAgentReplyArtifactRecord>, String> {
    RemoteFeishuAgentReplyArtifactRuntime::latest_for_dispatch(
        RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&context.root),
        &record.dispatch_id,
    )
    .map_err(|error| format!("{error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(body: &str) -> RemoteFeishuAgentReplyArtifactRecord {
        RemoteFeishuAgentReplyArtifactRecord {
            schema_version: 1,
            dispatch_id: "dispatch".to_owned(),
            run_session_id: "session".to_owned(),
            processing_message_id: None,
            receive_id: "chat".to_owned(),
            receive_id_type: "chat_id".to_owned(),
            reply_to_message_id: None,
            thread_id: None,
            body: body.to_owned(),
            recorded_at_ms: 1,
        }
    }

    #[test]
    fn attachment_delivery_requires_an_existing_local_file() {
        let root = axon_test_support::test_temp_dir();
        let file = root.path().join("report.txt");
        std::fs::write(&file, "report").expect("测试文件应可写入");

        // 路径不存在：不得进入附件分支，必须退化为纯正文投递。
        assert!(!agent_reply_contains_local_file(&artifact(
            "说明\nMEDIA:C:\\missing\\report.txt"
        )));
        // 路径存在：进入附件分支，同时正文按可见文本投递。
        assert!(agent_reply_contains_local_file(&artifact(&format!(
            "说明\nMEDIA:{}",
            file.display()
        ))));
        assert!(!agent_reply_contains_local_file(&artifact("普通回复")));
    }
}
