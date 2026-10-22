use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use super::super::RemoteFeishuGatewayCliEventReport;
use super::*;
use axon_domain::{AxonError, RemoteCapabilityPolicy};
use axon_remote_feishu::{
    RemoteFeishuGovernanceDecisionStatus, RemoteFeishuGovernanceStoreRuntime,
    RemoteFeishuInboundMediaDownloadInput, RemoteFeishuMediaFlowRuntime,
    RemoteFeishuOapiExecutorRuntime,
};
use axon_session::WorkspaceResolver;

use super::remote_feishu_gateway_action::submit_gateway_agent_action;
use super::remote_feishu_gateway_mapping::{gateway_event_report, to_gateway_event};
use crate::remote_commands::now_ms;

/// 单个入站附件下载上限；超过即降级该附件，不允许长期占用入站主链。
const REMOTE_FEISHU_INBOUND_MEDIA_ATTACHMENT_TIMEOUT: Duration = Duration::from_secs(10);
/// 单条入站消息的附件下载总预算，避免多附件消息把阻塞时间线性放大。
const REMOTE_FEISHU_INBOUND_MEDIA_EVENT_BUDGET: Duration = Duration::from_secs(20);

pub(super) struct FeishuGatewayExecutionRuntime<'a> {
    pub(super) context: &'a RemoteCliContext,
    pub(super) command: &'a RemoteGatewayCommand,
    pub(super) tenant_access_token: &'a str,
    pub(super) api_base_url: Option<&'a str>,
    pub(super) governance: &'a RemoteFeishuGovernanceRuntime,
    pub(super) store_path: &'a std::path::Path,
}

/// 车道执行所需的 owned 上下文。
///
/// 车道 worker 不能再借用调度线程的栈，但必须共享同一份配置、治理运行态与引用缓存，
/// 因此把借用来源整体持有下来，按需物化 [`FeishuGatewayExecutionRuntime`] 借用视图。
pub(super) struct GatewayLaneScope {
    context: RemoteCliContext,
    command: RemoteGatewayCommand,
    tenant_access_token: String,
    api_base_url: Option<String>,
    governance: RemoteFeishuGovernanceRuntime,
    store_path: PathBuf,
    reply_resolver: Arc<RemoteFeishuReplyResolver>,
}

impl GatewayLaneScope {
    pub(super) fn new(
        context: RemoteCliContext,
        command: RemoteGatewayCommand,
        tenant_access_token: String,
        api_base_url: Option<String>,
        reply_resolver: Arc<RemoteFeishuReplyResolver>,
    ) -> Self {
        Self {
            store_path: RemoteFeishuGovernanceStoreRuntime::resolve_store_path(&context.root),
            governance: RemoteFeishuGovernanceRuntime::new(
                RemoteCapabilityPolicy::default_sidecar(),
            ),
            context,
            command,
            tenant_access_token,
            api_base_url,
            reply_resolver,
        }
    }

    /// 执行单条入站事件的完整处理链。
    ///
    /// 被治理跳过或不属于本 bot 的 reaction 返回空报告：它们在语义上已经处理完毕，
    /// 批次确认必须继续推进。
    pub(super) fn execute(
        &self,
        input: &FeishuGatewayInputEvent,
    ) -> Result<ExecutedGatewayInput, String> {
        let runtime = FeishuGatewayExecutionRuntime {
            context: &self.context,
            command: &self.command,
            tenant_access_token: &self.tenant_access_token,
            api_base_url: self.api_base_url.as_deref(),
            governance: &self.governance,
            store_path: &self.store_path,
        };
        let Some(input) = super::materialize_pending_reaction(&runtime, input)? else {
            return Ok(ExecutedGatewayInput::default());
        };
        let mut gateway = RemoteFeishuGatewayRuntime::new(1);
        gateway.connect();
        Ok(
            process_gateway_input(&runtime, &input, self.reply_resolver.as_ref(), &mut gateway)?
                .unwrap_or_default(),
        )
    }
}

#[derive(Debug, Default)]
pub(in crate::remote_commands::remote_feishu_gateway) struct ExecutedGatewayInput {
    pub(in crate::remote_commands::remote_feishu_gateway) persisted_records: usize,
    pub(in crate::remote_commands::remote_feishu_gateway) released_turns: usize,
    pub(in crate::remote_commands::remote_feishu_gateway) event_reports:
        Vec<RemoteFeishuGatewayCliEventReport>,
}

pub(super) fn process_gateway_input(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &FeishuGatewayInputEvent,
    reply_resolver: &RemoteFeishuReplyResolver,
    gateway: &mut RemoteFeishuGatewayRuntime,
) -> Result<Option<ExecutedGatewayInput>, String> {
    if should_skip_gateway_input_event(runtime.context, input)? {
        return Ok(None);
    }
    // 治理记录只在「已交付」或终结性拒绝后写入，因此命中即代表这条事件处理过：
    // 引用与媒体属于首次投递的增强信息，重放时重复下载/查询只会浪费入站主链时间。
    let existing_record =
        RemoteFeishuGovernanceStoreRuntime::read_record(runtime.store_path, &input.request_id)
            .map_err(|error| format!("读取 Feishu 治理索引失败：{error:?}"))?;
    let already_settled = existing_record.is_some();
    let mut enriched_input = input.clone();
    if !already_settled {
        enrich_reply_context(
            &mut enriched_input,
            runtime.tenant_access_token,
            runtime.api_base_url,
            reply_resolver,
        );
        materialize_inbound_media(runtime, &mut enriched_input)?;
    }
    gateway
        .enqueue(
            to_gateway_event(runtime.command.session_id, &enriched_input)?,
            input.enqueued_at_ms.unwrap_or_else(now_ms),
        )
        .map_err(|error| format!("{error:?}"))?;
    let tick = execute_gateway_tick(
        runtime.context,
        gateway,
        runtime.governance,
        runtime.store_path,
        existing_record,
    )?;
    let persisted_records = tick.persisted_records;
    let mut released_turns = 0usize;
    let mut event_reports = Vec::new();
    let mut settled_records = 0usize;
    for event_report in tick.governance.event_reports {
        let (report, released, settled) =
            process_governance_event(runtime, input, gateway, event_report)?;
        released_turns += usize::from(released);
        settled_records += usize::from(settled);
        event_reports.push(report);
    }
    Ok(Some(ExecutedGatewayInput {
        persisted_records: persisted_records + settled_records,
        released_turns,
        event_reports,
    }))
}

/// 下载入站媒体并把本地缓存路径注入 agent 可见文本。
///
/// 必须发生在 worker 阶段（durable append 之后）：在飞书 callback 内下载会把回调
/// ACK 拖长到秒级，触发官方超时重投并放大为重复事件。附件是消息的增强而非本体，
/// 因此单条附件下载失败只降级该附件并留下明确标记，绝不拒绝整条用户消息。
fn materialize_inbound_media(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &mut FeishuGatewayInputEvent,
) -> Result<(), String> {
    if input.kind != "message" || input.media_refs.is_empty() {
        return Ok(());
    }
    let RemoteFeishuGatewayInboundEvent::Message(message) =
        to_gateway_event(runtime.command.session_id, input)?
    else {
        return Ok(());
    };
    let cache_root = axon_remote_feishu::remote_feishu_runtime_root(&runtime.context.root);
    // 单条消息的附件下载必须有上界：入站媒体是消息增强，历史实现用全局 30s agent
    // 逐个附件串行下载，任意一个 chat 的大附件都能阻塞全部 chat 的治理与提交。
    let deadline = Instant::now() + REMOTE_FEISHU_INBOUND_MEDIA_EVENT_BUDGET;
    match RemoteFeishuMediaFlowRuntime::download_inbound_media_with_http(
        RemoteFeishuInboundMediaDownloadInput {
            message,
            tenant_access_token: runtime.tenant_access_token.to_owned(),
            api_base_url: runtime.api_base_url.map(ToOwned::to_owned),
            cache_root,
        },
        |request| {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AxonError::runtime_failure(
                    "Feishu 入站媒体下载超出单条消息预算",
                ));
            }
            RemoteFeishuOapiExecutorRuntime::execute_binary_with_timeout(
                request,
                remaining.min(REMOTE_FEISHU_INBOUND_MEDIA_ATTACHMENT_TIMEOUT),
            )
        },
    ) {
        Ok(report) => {
            input.text = Some(report.message.text);
            Ok(())
        }
        Err(error) => {
            axon_logger::warning(&format!(
                "Feishu 入站媒体下载失败，已降级为附件不可用: message_id={:?} error={error:?}",
                input.message_id
            ));
            input.text = Some(degraded_media_text(input));
            Ok(())
        }
    }
}

/// 下载失败时保留附件元数据并显式标注不可用，避免 Agent 对不存在的附件作推断。
fn degraded_media_text(input: &FeishuGatewayInputEvent) -> String {
    let mut text = input.text.clone().unwrap_or_default().trim().to_owned();
    for media in &input.media_refs {
        let name = media.file_name.as_deref().unwrap_or(media.kind.as_str());
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!(
            "[Attachment: {name}]\n（附件下载失败，内容不可用，请让用户重新发送）"
        ));
    }
    text
}

/// 处理单条治理事件，返回（事件报告, 是否释放 chat turn, 是否落盘 Accepted 记录）。
fn process_governance_event(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &FeishuGatewayInputEvent,
    gateway: &mut RemoteFeishuGatewayRuntime,
    event_report: axon_remote_feishu::RemoteFeishuGatewayGovernanceEventReport,
) -> Result<(RemoteFeishuGatewayCliEventReport, bool, bool), String> {
    let received_at_ms = input.enqueued_at_ms.unwrap_or_else(now_ms);
    if event_report.decision.status
        == axon_remote_feishu::RemoteFeishuGovernanceDecisionStatus::Accepted
        && event_report.meta.message_kind.is_some()
    {
        if input.kind == "message"
            && input
                .source_created_at_ms
                .filter(|value| *value > 0)
                .is_none()
        {
            return Err("已接受的飞书消息缺少有效 sourceCreatedAtMs".to_owned());
        }
        let workspace = WorkspaceResolver
            .resolve(&runtime.context.axon_root, received_at_ms)
            .map_err(|error| format!("解析 Feishu Interaction Workspace 失败：{error}"))?;
        if is_card_action_kind(&input.kind) {
            handle_card_action_input(runtime, input, &event_report, &workspace.workspace_id);
        } else {
            // 记录车道与会话的绑定：Daemon 侧提问落在该车道，问答卡片据此找回用户会话。
            record_gateway_chat_route(runtime, input, &workspace.workspace_id);
        }
        if let Some(interaction) = axon_remote_feishu::interaction_from_gateway_report(
            &event_report,
            &workspace.workspace_id,
            received_at_ms,
        )? {
            remote_feishu_control::submit_gateway_interaction(
                runtime.context,
                &runtime.command.daemon_endpoint,
                &interaction,
            )?;
        }
    }
    advance_accepted_gateway_input(
        &runtime.context.root,
        input,
        event_report.decision.status,
        received_at_ms,
    )?;
    let execution = submit_gateway_agent_action(
        runtime.context,
        &runtime.command.daemon_endpoint,
        runtime.command.session_id,
        &event_report,
    )?;
    // Accepted 记录的语义是「已交付」：只有 dispatch 提交成功后才落盘。
    // 提前落盘会让提交失败后的重投被自己的记录判成重复，事件被静默丢弃。
    let settled = event_report.decision.status
        == axon_remote_feishu::RemoteFeishuGovernanceDecisionStatus::Accepted;
    if settled {
        axon_remote_feishu::RemoteFeishuGovernanceStoreRuntime::append_record(
            runtime.store_path,
            &event_report.decision.record,
        )
        .map_err(|error| format!("写入 Feishu 治理记录失败：{error:?}"))?;
    }
    let release = gateway
        .complete_chat_turn(&event_report.meta.chat_id)
        .map_err(|error| format!("{error:?}"))?;
    Ok((
        gateway_event_report(event_report, execution, release.released),
        release.released,
        settled,
    ))
}

/// 卡片动作事件类型判定。
///
/// 事件源写入的是 `cardAction`（`card_action_record`），外部 adapter 也可能写成
/// `card-action`；两者语义相同，判定必须同时接受，否则卡片动作会整体走不到分发。
pub(super) fn is_card_action_kind(kind: &str) -> bool {
    matches!(kind, "cardAction" | "card-action")
}

/// 取卡片动作的问答载荷。
///
/// 载荷只在 `actionValueJson` 里：卡片动作事件的 `text` 恒为空，按 `text` 取载荷的
/// 实现永远拿不到按钮内容。
pub(super) fn question_answer_payload(input: &FeishuGatewayInputEvent) -> Option<&str> {
    if !is_card_action_kind(&input.kind) {
        return None;
    }
    input
        .action_value_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// 问答作答的入站处理：载荷取自 `actionValueJson`，操作人取自事件携带的身份。
fn handle_card_action_input(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &FeishuGatewayInputEvent,
    event_report: &axon_remote_feishu::RemoteFeishuGatewayGovernanceEventReport,
    workspace_id: &str,
) {
    let Some(action_json) = question_answer_payload(input) else {
        // 卡片动作没有载荷时按既有卡片动作链路继续推进，不在这里吞掉事件。
        return;
    };
    let operator = input
        .operator_open_id
        .as_deref()
        .or(event_report.meta.operator_open_id.as_deref())
        .unwrap_or_default();
    if operator.trim().is_empty() {
        axon_logger::warning(&format!(
            "Feishu 卡片动作缺少操作人身份，问答结算按匿名处理: request_id={}",
            input.request_id
        ));
    }
    let outcome =
        crate::remote_commands::remote_feishu_question_action::try_handle_feishu_question_action(
            runtime.context,
            &runtime.command.daemon_endpoint,
            action_json,
            operator,
            &event_report.meta.chat_id,
            input.message_id.as_deref(),
            Some(workspace_id),
        );
    match outcome {
        Some(Ok(receipt)) => axon_logger::info(&format!("Feishu 卡片动作已就地处理: {receipt}")),
        Some(Err(error)) => axon_logger::warning(&format!(
            "Feishu 卡片动作就地处理失败: request_id={} error={error}",
            input.request_id
        )),
        None => {}
    }
}

/// 记录 `车道 -> 会话` 绑定，供问答卡片投递与结算回执定位目标会话。
pub(super) fn record_gateway_chat_route(
    runtime: &FeishuGatewayExecutionRuntime<'_>,
    input: &FeishuGatewayInputEvent,
    workspace_id: &str,
) {
    let Some(chat_type) = input
        .chat_type
        .as_deref()
        .and_then(|value| axon_remote_feishu::RemoteFeishuChatType::from_stable_value(Some(value)))
    else {
        return;
    };
    let reply_to_message_id = input
        .reply_context
        .as_ref()
        .and_then(|context| context.reply_to_message_id.as_deref());
    if let Err(error) =
        crate::remote_commands::remote_feishu_question_action::record_feishu_question_route(
            &runtime.context.root,
            workspace_id,
            &input.chat_id,
            chat_type,
            reply_to_message_id,
        )
    {
        axon_logger::warning(&format!("Feishu 问答路由绑定写入失败: {error}"));
    }
}

pub(super) fn advance_accepted_gateway_input(
    root: &std::path::Path,
    input: &FeishuGatewayInputEvent,
    status: RemoteFeishuGovernanceDecisionStatus,
    recorded_at_ms: u64,
) -> Result<(), String> {
    if input.kind != "message" || status != RemoteFeishuGovernanceDecisionStatus::Accepted {
        return Ok(());
    }
    let source_created_at_ms = input
        .source_created_at_ms
        .filter(|value| *value > 0)
        .ok_or_else(|| "已接受的飞书消息缺少有效 sourceCreatedAtMs".to_owned())?;
    remote_feishu_ingress_watermark::advance(
        root,
        &input.chat_id,
        &input.request_id,
        source_created_at_ms,
        recorded_at_ms,
    )
    .map(|_| ())
}
