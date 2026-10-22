use std::{
    sync::{Arc, mpsc::SyncSender},
    time::Duration,
};

use super::super::{
    RemoteCliContext, RemoteGatewayCommand, remote_feishu_ingress_watermark,
    remote_feishu_reply_resolver::RemoteFeishuReplyResolver,
};
use super::{
    REMOTE_FEISHU_GATEWAY_PAIRING_ID,
    remote_feishu_gateway_input::{FeishuGatewayInputEvent, RemoteFeishuGatewayMediaRefInput},
};
use axon_domain::AxonResult;
use axon_remote_feishu::{
    RemoteFeishuChatGovernancePolicy, RemoteFeishuGatewayGovernanceInput,
    RemoteFeishuGatewayInboundEvent, RemoteFeishuGatewayRuntime, RemoteFeishuGatewaySdkLoopEvent,
    RemoteFeishuGatewaySdkPayloadMapperRuntime, RemoteFeishuGatewayTickInput,
    RemoteFeishuGovernanceConfig, RemoteFeishuGovernanceRecord, RemoteFeishuGovernanceRuntime,
    RemoteFeishuHttpRequestPlan, RemoteFeishuHttpResponse, RemoteFeishuIncomingMessageKind,
    RemoteFeishuMessageLookupRuntime, RemoteFeishuOapiExecutorRuntime,
};

#[path = "remote_feishu_control.rs"]
pub(crate) mod remote_feishu_control;
#[path = "remote_feishu_gateway_action.rs"]
mod remote_feishu_gateway_action;
#[path = "remote_feishu_gateway_event_execution.rs"]
mod remote_feishu_gateway_event_execution;
#[path = "remote_feishu_gateway_lanes.rs"]
pub(crate) mod remote_feishu_gateway_lanes;
#[path = "remote_feishu_gateway_mapping.rs"]
mod remote_feishu_gateway_mapping;
#[path = "remote_feishu_processing_queue.rs"]
mod remote_feishu_processing_queue;

use remote_feishu_gateway_action::enqueue_processing_start;
#[cfg(test)]
use remote_feishu_gateway_event_execution::advance_accepted_gateway_input;
use remote_feishu_gateway_event_execution::{FeishuGatewayExecutionRuntime, GatewayLaneScope};
use remote_feishu_gateway_lanes::GatewayLaneScheduler;
use remote_feishu_gateway_mapping::{message_kind, reply_context};

#[cfg(test)]
#[path = "remote_feishu_gateway_closure_liveness_tests.rs"]
mod closure_liveness_tests;

const FEISHU_DAEMON_SESSION_PREFIX: &str = "feishu-session-";
/// reaction 归属查询上限；与引用正文解析（3s）保持一致。
///
/// reaction 只是交互增强，不允许用全局 30s HTTP 超时占用入站主链的长尾。
const REMOTE_FEISHU_GATEWAY_REACTION_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn prepare_sdk_event(
    context: &RemoteCliContext,
    event: &RemoteFeishuGatewaySdkLoopEvent,
) -> Result<bool, String> {
    let RemoteFeishuGatewaySdkLoopEvent::Message(message) = event else {
        return Ok(false);
    };
    enqueue_processing_start(context, &message.message_id);
    // 返回 false，确保事件总是先写入 durable event source，再由网关提交 Interaction。
    Ok(false)
}

/// 启动按 chat 分车道的入站调度器。
///
/// 车道共享同一份配置、治理运行态与引用缓存；每条任务完成即通过 `wake` 唤醒调度
/// 线程继续结算与读取，因此不需要额外的轮询或第二套唤醒通道。
pub(crate) fn start_gateway_lane_scheduler(
    context: RemoteCliContext,
    command: RemoteGatewayCommand,
    tenant_access_token: String,
    api_base_url: Option<String>,
    wake: SyncSender<()>,
) -> GatewayLaneScheduler {
    let scope = GatewayLaneScope::new(
        context,
        command,
        tenant_access_token,
        api_base_url,
        Arc::new(RemoteFeishuReplyResolver::default()),
    );
    GatewayLaneScheduler::start(move |input| scope.execute(input), wake)
}

/// 把单个 reactionPending 事件补齐为可治理的消息事件。
///
/// 车道 worker（子模块）直接调用；不属于本 bot 的 reaction 或无法补齐归属的 reaction
/// 返回 `None`（该事件已处理完毕）。
fn materialize_pending_reaction(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &FeishuGatewayInputEvent,
) -> Result<Option<FeishuGatewayInputEvent>, String> {
    materialize_pending_reaction_with_http(runtime, input, |request| {
        RemoteFeishuOapiExecutorRuntime::execute_json_with_timeout(
            request,
            REMOTE_FEISHU_GATEWAY_REACTION_LOOKUP_TIMEOUT,
        )
    })
}

fn materialize_pending_reaction_with_http<F>(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &FeishuGatewayInputEvent,
    mut http: F,
) -> Result<Option<FeishuGatewayInputEvent>, String>
where
    F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
{
    if input.kind != "reactionPending" {
        return Ok(Some(input.clone()));
    }
    let Some(raw_payload) = input.raw_payload_json.as_deref() else {
        return Err("reactionPending 事件缺少 rawPayloadJson".to_owned());
    };
    let message_id = input
        .message_id
        .as_deref()
        .ok_or_else(|| "reactionPending 事件缺少 messageId".to_owned())?;
    let Some(info) = RemoteFeishuMessageLookupRuntime::fetch_message_info_with_http(
        runtime.api_base_url,
        runtime.tenant_access_token,
        message_id,
        &mut http,
    )
    .map_err(|error| format!("reaction 消息归属查询失败: {error:?}"))?
    else {
        return Ok(None);
    };
    if !info
        .sender_id
        .as_deref()
        .is_some_and(|sender_id| sender_id == runtime.context.feishu_websocket.app_id)
    {
        return Ok(None);
    }
    let Some(chat_id) = info
        .chat_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
    else {
        axon_logger::warning(&format!(
            "Feishu reaction 消息归属查询未返回 chat_id，已跳过该事件: message_id={message_id}"
        ));
        return Ok(None);
    };
    // reaction payload 不含 chat_type，而 Interaction 合同要求 p2p/group 明确：查不到就跳过，
    // 绝不能把群聊默认记成私聊（错误受众会污染 disclosure 与 Workspace 车道语义）。
    let resolved_chat_type = RemoteFeishuMessageLookupRuntime::fetch_chat_type_with_http(
        runtime.api_base_url,
        runtime.tenant_access_token,
        &chat_id,
        &mut http,
    )
    .map_err(|error| format!("reaction 会话类型查询失败: {error:?}"))?;
    let Some(resolved_chat_type) = resolved_chat_type else {
        axon_logger::warning(&format!(
            "Feishu reaction 无法确定会话类型，已跳过该事件: message_id={message_id}"
        ));
        return Ok(None);
    };
    let mut value = serde_json::from_str::<serde_json::Value>(raw_payload)
        .map_err(|_| "reaction 原始 payload JSON 无效".to_owned())?;
    {
        let event = value
            .get_mut("event")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| "reaction 原始 payload 缺少 event 对象".to_owned())?;
        event.insert("chat_id".to_owned(), serde_json::Value::String(chat_id));
        event.insert(
            "chat_type".to_owned(),
            serde_json::Value::String(resolved_chat_type.as_str().to_owned()),
        );
    }
    let mapped = RemoteFeishuGatewaySdkPayloadMapperRuntime::map_reaction_payload(
        &serde_json::to_string(&value).map_err(|_| "reaction payload 序列化失败".to_owned())?,
    )
    .map_err(|error| format!("reaction payload 映射失败: {error:?}"))?;
    let RemoteFeishuGatewaySdkLoopEvent::Message(message) = mapped else {
        return Err("reaction payload 映射结果类型无效".to_owned());
    };
    if message.source_created_at_ms.is_none() {
        // 缺少 action_time 时 ingress 水位会直接确认并丢弃该事件；把这种静默丢弃变成可见告警。
        axon_logger::warning(&format!(
            "Feishu reaction 事件缺少 action_time，将按无有效 sourceCreatedAtMs 确认并丢弃: message_id={}",
            message.message_id
        ));
    }
    Ok(Some(FeishuGatewayInputEvent {
        kind: "message".to_owned(),
        request_id: message.request_id,
        chat_id: message.chat_id,
        chat_type: Some(message.chat_type.as_str().to_owned()),
        message_id: Some(message.message_id),
        source_created_at_ms: message.source_created_at_ms,
        sender_open_id: Some(message.sender_open_id),
        operator_open_id: None,
        mentions: message.mentions,
        text: Some(message.text),
        media_refs: message
            .media_refs
            .into_iter()
            .map(|media| RemoteFeishuGatewayMediaRefInput {
                kind: media.kind.as_str().to_owned(),
                key: media.key,
                file_name: media.file_name,
            })
            .collect(),
        message_kind: Some(message.message_kind.as_str().to_owned()),
        action_tag: None,
        action_value_json: None,
        reply_context: None,
        session_busy: message.session_busy,
        enqueued_at_ms: input.enqueued_at_ms,
        raw_payload_json: None,
    }))
}

fn should_skip_gateway_input_event(
    context: &RemoteCliContext,
    input: &FeishuGatewayInputEvent,
) -> Result<bool, String> {
    if input.kind != "message" {
        return Ok(false);
    }
    remote_feishu_ingress_watermark::should_reject(
        &context.root,
        &input.chat_id,
        &input.request_id,
        input.source_created_at_ms,
    )
}

/// 执行单条事件的治理裁决。
///
/// `existing_record` 命中说明这条事件已经交付过：治理链必然给出 `DuplicateDropped`，
/// 因此只做内存裁决（`drain_into_governance`）而不再写回治理记录：重放既不该产生
/// 写入，也不该用 `DuplicateDropped` 覆盖原来的 `Accepted` 审计记录。
fn execute_gateway_tick(
    context: &RemoteCliContext,
    gateway: &mut RemoteFeishuGatewayRuntime,
    governance: &RemoteFeishuGovernanceRuntime,
    store_path: &std::path::Path,
    existing_record: Option<RemoteFeishuGovernanceRecord>,
) -> Result<axon_remote_feishu::RemoteFeishuGatewayTickReport, String> {
    let settled = existing_record.is_some();
    let governance_input = RemoteFeishuGatewayGovernanceInput {
        governance_config: governance_config(context),
        pairing_id: REMOTE_FEISHU_GATEWAY_PAIRING_ID.to_owned(),
        existing_records: existing_record.into_iter().collect(),
    };
    if settled {
        let report = gateway
            .drain_into_governance(governance, &governance_input)
            .map_err(|error| format!("{error:?}"))?;
        return Ok(axon_remote_feishu::RemoteFeishuGatewayTickReport {
            governance: report,
            persisted_records: 0,
            active_chat_turns: gateway.active_chat_turn_count(),
        });
    }
    gateway
        .tick_and_persist_governance(
            governance,
            &RemoteFeishuGatewayTickInput {
                governance: governance_input,
                governance_store_path: store_path.to_path_buf(),
            },
        )
        .map_err(|error| format!("{error:?}"))
}

fn governance_config(context: &RemoteCliContext) -> RemoteFeishuGovernanceConfig {
    RemoteFeishuGovernanceConfig {
        allowed_sender_open_ids: context.feishu_websocket.allowed_sender_open_ids.clone(),
        blocked_sender_open_ids: context.feishu_websocket.blocked_sender_open_ids.clone(),
        require_mention: context.feishu_websocket.require_mention,
        bot_open_id: super::super::remote_feishu_identity::resolve_feishu_bot_identity(context)
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

fn enrich_reply_context(
    input: &mut FeishuGatewayInputEvent,
    tenant_access_token: &str,
    api_base_url: Option<&str>,
    reply_resolver: &RemoteFeishuReplyResolver,
) {
    if !should_enrich_reply_context(input) {
        return;
    }
    let context = reply_context(input.reply_context.as_ref());
    let resolution = reply_resolver.resolve(context.as_ref(), tenant_access_token, api_base_url);
    let Some(quoted_message) = resolution.quoted_message else {
        return;
    };
    if let Some(reply_context) = input.reply_context.as_mut() {
        reply_context.reply_to_text = Some(quoted_message.visible_text);
    }
}

fn should_enrich_reply_context(input: &FeishuGatewayInputEvent) -> bool {
    input.kind == "message"
        && message_kind(input).ok() == Some(RemoteFeishuIncomingMessageKind::Text)
}

#[cfg(test)]
#[path = "remote_feishu_gateway_execution_tests.rs"]
mod tests;
