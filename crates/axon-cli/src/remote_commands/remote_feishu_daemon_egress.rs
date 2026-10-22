//! Daemon Session Event 到 Feishu 持久出站的组合根适配器。
//!
//! Agent Runtime 只写 transport-neutral Session Event；本模块识别 Feishu
//! transport context，并在网络投递前先持久化 artifact 与 outbox。

use std::path::{Path, PathBuf};

use axon_crypto::sha256_hex_prefix;
use axon_domain::SessionId;
use axon_protocol::wire::{WireEventEnvelope, WireEventKind, WireProviderStatusPhase};
use axon_remote_feishu::{
    FeishuEventAdapter, RemoteFeishuAgentReplyArtifactInput, RemoteFeishuAgentReplyArtifactRuntime,
    RemoteFeishuFeedbackTaskStoreRuntime, RemoteFeishuMessageTarget, RemoteFeishuProcessingOutcome,
};
use axon_remote_runtime::{
    RemoteNotificationDelivery, RemoteNotificationDispatchStatus, RemoteNotificationFanoutPlan,
    RemoteNotificationKind, RemoteNotificationOutboxInput, RemoteNotificationOutboxRecord,
    RemoteNotificationOutboxRuntime, RemoteNotificationSeverity, RemoteNotificationTimingPolicy,
    RemoteTransportKind, WireLifecycleStatus, remote_notification_wire_lifecycle_event,
};
use axon_session::SessionEventRecord;
use axon_session_runtime::EventPublisher;
use serde_json::Value;

use super::remote_feishu_transport_context::{is_feishu, parse as parse_feishu_transport_context};

const TURN_COMPLETED_EVENT: &str = "turn_completed";
const TURN_FAILED_EVENT: &str = "turn_failed";
const WIRE_EVENT: &str = "wire_event";
const FEISHU_DAEMON_SESSION_PREFIX: &str = "feishu-session-";
const FEISHU_EGRESS_DISPATCH_PREFIX: &str = "feishu-egress-";
const FEISHU_DAEMON_SUBSCRIBER_ID: &str = "feishu-daemon";
const FEISHU_DAEMON_PAIRING_ID: &str = "pairing-feishu";
const FEISHU_EGRESS_SOURCE_TRACE: &str = "daemon:feishu_event_publisher";
const FEISHU_EGRESS_REASON: &str = "daemon:feishu_turn_completed";
const FEISHU_REPLY_TITLE: &str = "Axon 回复";
/// 主 turn 被永久拒绝时的回执标题；失败必须与正常回复可区分。
const FEISHU_TURN_FAILURE_TITLE: &str = "Axon 处理失败";
/// 主 turn 被永久拒绝时的 outbox 原因；与正常回复分属不同原因便于审计。
const FEISHU_TURN_FAILURE_REASON: &str = "daemon:feishu_turn_rejected";
/// 问答结算降级通知的标题。
const FEISHU_QUESTION_NOTICE_TITLE: &str = "Axon 问答已结算";
/// 问答结算降级通知的 outbox 原因。
const FEISHU_QUESTION_NOTICE_REASON: &str = "daemon:feishu_question_settled";
/// 问答结算降级通知的本地动作提示。
const FEISHU_QUESTION_NOTICE_ACTION_HINT: &str = "Feishu 问答卡片状态回写失败，改以文本送达";
const FEISHU_REPLY_LOCAL_ACTION_HINT: &str = "Feishu 回复当前会话";
/// Feishu 渠道标识；与入站 Interaction 的 `source.channel()` 保持同一契约。
const FEISHU_DISPATCH_HASH_CHARS: usize = 16;
const FEISHU_REPLY_PREVIEW_LIMIT: usize = 240;
const FEISHU_PROVIDER_CONNECTING: &str = "🔌 正在连接供应商";
const FEISHU_PROVIDER_CONNECTED: &str = "✅ 供应商已连接";
const FEISHU_PROVIDER_GENERATING: &str = "💭 正在思考并生成回复";
const FEISHU_PROVIDER_RETRYING: &str = "🔁 正在重试供应商请求";
const FEISHU_PROVIDER_FAILED: &str = "⚠️ 供应商请求失败";
const FEISHU_PROVIDER_FAILOVER: &str = "↪️ 正在切换供应商";

/// 已提交 Session Event 的 Feishu 出站发布器。
#[derive(Debug, Clone)]
pub(crate) struct FeishuDaemonEventPublisher {
    axon_root: PathBuf,
    config_path: PathBuf,
}

impl FeishuDaemonEventPublisher {
    /// 使用 Axon 根目录创建发布器。
    #[must_use]
    pub(crate) const fn new(axon_root: PathBuf, config_path: PathBuf) -> Self {
        Self {
            axon_root,
            config_path,
        }
    }
}

impl EventPublisher for FeishuDaemonEventPublisher {
    fn publish(
        &self,
        event: &SessionEventRecord,
        _cursor: &axon_domain::ConsumerCursor,
    ) -> Result<(), String> {
        self.publish_with_work(event).map(|_| ())
    }
}

impl FeishuDaemonEventPublisher {
    pub(crate) fn publish_with_work(&self, event: &SessionEventRecord) -> Result<bool, String> {
        let feedback_created =
            match enqueue_control_processing_feedback(&self.axon_root, &self.config_path, event) {
                Ok(created) => created,
                Err(error) => {
                    axon_logger::warning(&format!(
                        "Feishu 控制 Action Typing feedback 入队失败，不影响事件出站: {error}"
                    ));
                    false
                }
            };
        // 实时投影：只做无阻塞入队，网络写入由后台 worker 完成，绝不阻塞本事件流。
        project_committed_assistant_delta(event);
        let Some(input) = FeishuEgressInput::from_event(event)? else {
            // 非出站事件同样要收口投影登记，避免回合结束后登记表残留。
            if matches!(
                event.event_type.as_str(),
                TURN_COMPLETED_EVENT | TURN_FAILED_EVENT
            ) {
                let _ = finish_projection(&event.session_id.to_string());
            }
            return Ok(feedback_created);
        };
        // 回合收口：正文若已完整投影到卡片，则不再写 durable outbox，避免同一正文投递两次。
        let projection = finish_projection(&event.session_id.to_string());
        if event.event_type == TURN_COMPLETED_EVENT && projection.suppresses_durable_reply() {
            return Ok(true);
        }
        persist_egress(&self.axon_root, input)?;
        Ok(true)
    }
}

/// 把已提交的 assistant 增量与工具开始/完成交给实时投影；未登记的 session 静默忽略。
///
/// 普通流式 wire 事件不携带 `transportContext`，归属由 turn 开始时的 session 登记决定。
/// 工具卡只投影 wire 事件里已脱敏的 `summary`，**绝不投影 `output_preview` 等原始输出**。
fn project_committed_assistant_delta(event: &SessionEventRecord) {
    if event.event_type != WIRE_EVENT {
        return;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&event.payload_json) else {
        return;
    };
    let Ok(envelope) = serde_json::from_value::<WireEventEnvelope>(payload) else {
        return;
    };
    let session_id = event.session_id.to_string();
    match &envelope.kind {
        WireEventKind::AssistantMessage(message)
            if message.delta && !message.content.is_empty() =>
        {
            super::remote_feishu_progress_service::project_committed_delta(
                &session_id,
                &message.content,
            );
        }
        WireEventKind::ToolCall(call) => {
            // 折叠态只给一项事实：模型自述目的 → 工具专用主参数 → 通用主参数 → 原始参数摘要。
            // 规则要求静默的工具（参数没有信息量）不追加任何目标行。
            let summary = super::remote_feishu_progress_projection::render_tool_target(
                &call.tool_name,
                call.arguments_summary.as_deref(),
                call.intent.as_deref(),
            )
            .unwrap_or_default();
            super::remote_feishu_progress_service::project_committed_tool(
                &session_id,
                &call.tool_call_id,
                super::remote_feishu_progress_projection::render_tool_body(
                    &call.tool_name,
                    &summary,
                    super::remote_feishu_progress_projection::ToolProgressStatus::Running,
                ),
            );
        }
        WireEventKind::ToolResult(result) => {
            let status = if result.status == WireLifecycleStatus::Completed {
                super::remote_feishu_progress_projection::ToolProgressStatus::Completed
            } else {
                super::remote_feishu_progress_projection::ToolProgressStatus::Failed
            };
            super::remote_feishu_progress_service::project_committed_tool(
                &session_id,
                &result.tool_call_id,
                super::remote_feishu_progress_projection::render_tool_body(
                    result.tool_name.as_deref().unwrap_or_default(),
                    &result.summary,
                    status,
                ),
            );
        }
        WireEventKind::ProviderStatus(status) => {
            super::remote_feishu_progress_service::project_committed_delta(
                &session_id,
                &provider_status_text(status),
            );
        }
        WireEventKind::ReasoningStart(_) => {
            super::remote_feishu_progress_service::project_committed_delta(
                &session_id,
                "💭 正在思考…",
            );
        }
        _ => {}
    }
}

fn provider_status_text(status: &axon_protocol::wire::WireProviderStatus) -> String {
    let phase = match status.phase {
        WireProviderStatusPhase::Connecting => FEISHU_PROVIDER_CONNECTING,
        WireProviderStatusPhase::Connected => FEISHU_PROVIDER_CONNECTED,
        WireProviderStatusPhase::Generating => FEISHU_PROVIDER_GENERATING,
        WireProviderStatusPhase::Retrying => FEISHU_PROVIDER_RETRYING,
        WireProviderStatusPhase::Failed => FEISHU_PROVIDER_FAILED,
        WireProviderStatusPhase::Failover => FEISHU_PROVIDER_FAILOVER,
    };
    if status.provider.trim().is_empty() && status.model.trim().is_empty() {
        return phase.to_owned();
    }
    format!(
        "{phase}：{}/{}",
        status.provider.trim(),
        status.model.trim()
    )
}

/// 收口该 session 的投影回合并返回结论。
fn finish_projection(session_id: &str) -> super::remote_feishu_progress_service::TurnProjection {
    super::remote_feishu_progress_service::finish_registered_turn(session_id)
}

fn enqueue_control_processing_feedback(
    axon_root: &Path,
    config_path: &Path,
    event: &SessionEventRecord,
) -> Result<bool, String> {
    if event.event_type != TURN_COMPLETED_EVENT {
        return Ok(false);
    }
    let projected = FeishuEventAdapter::project(event)
        .map_err(|error| format!("解析 Feishu 控制完成事件失败: {error}"))?;
    let payload = projected.payload;
    if payload.get("kind").and_then(Value::as_str) != Some("remote_control") {
        return Ok(false);
    }
    let Some(context) = payload.get("transportContext") else {
        return Ok(false);
    };
    if !is_feishu(context) {
        return Ok(false);
    }
    let Some(message_id) = optional_string(context, "sourceMessageId") else {
        return Ok(false);
    };
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(
        config_path,
    ))?;
    let loaded = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取 Feishu Typing 配置失败: {error:?}"))?;
    let config = loaded.config.remote.feishu_websocket;
    if !config.enabled || !config.enable_notification_sending {
        return Ok(false);
    }
    let session_id = numeric_session_id(&event.session_id.to_string());
    let action_id = payload
        .get("actionId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown-action");
    let task = RemoteFeishuFeedbackTaskStoreRuntime::task_from_cleanup(
        axon_remote_feishu::RemoteFeishuProcessingCleanupTaskInput {
            app_id: &config.app_id,
            app_secret_env: &config.app_secret_env,
            session_id,
            dispatch_id: &format!("feishu-control:{action_id}"),
            message_id: &message_id,
            outcome: RemoteFeishuProcessingOutcome::Success,
            api_base_url: (!config.api_base_url.trim().is_empty())
                .then_some(config.api_base_url.as_str()),
            recorded_at_ms: event.emitted_at_ms,
        },
    );
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(axon_root);
    RemoteFeishuFeedbackTaskStoreRuntime::append_record(
        RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&runtime_root),
        &task,
    )
    .map_err(|error| format!("写入 Feishu Typing feedback task 失败: {error:?}"))?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeishuEgressKind {
    TurnCompleted,
    /// 问答卡片回写失败时的降级文本通知。
    QuestionNotice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeishuEgressInput {
    daemon_session_id: String,
    numeric_session_id: SessionId,
    sequence: u64,
    emitted_at_ms: u64,
    chat_id: String,
    thread_id: Option<String>,
    reply_to_message_id: Option<String>,
    response: String,
    kind: FeishuEgressKind,
    /// 通知严重级别；主 turn 被拒绝的回执必须以 Warning 出站。
    severity: RemoteNotificationSeverity,
}

impl FeishuEgressInput {
    /// 由持久收件箱条目构造系统投递输入。
    ///
    /// 系统投递（Task/Execution 完成、Cron 投递）没有可回复的入站消息，因此不带
    /// reply-to：正文直接发送到目标对话。dispatch 由「投递身份 + 目标」稳定派生，
    /// 重复投递命中同一 dispatch，不会产生第二条消息。
    pub(crate) fn from_inbox_entry(entry: &axon_session::ClientInboxEntry) -> Self {
        Self {
            daemon_session_id: entry.execution_id.clone(),
            numeric_session_id: numeric_session_id(&entry.notification_id),
            sequence: 0,
            emitted_at_ms: entry.created_at_ms,
            chat_id: entry.destination.clone(),
            thread_id: None,
            reply_to_message_id: None,
            response: entry.content.clone(),
            kind: FeishuEgressKind::TurnCompleted,
            severity: RemoteNotificationSeverity::DirectReply,
        }
    }

    /// 由问答结算降级构造系统投递输入。
    ///
    /// 卡片 PATCH 失败时的兜底：结算事实已在问答记录里落定，正文改由 durable outbox
    /// 以普通消息送达，绝不因为卡片回写失败而让用户看不到结算结论。dispatch 由稳定
    /// 的结算标识派生，重复兜底命中同一 dispatch，不会刷出第二条通知。
    pub(crate) fn from_question_notice(
        chat_id: &str,
        response: &str,
        dispatch_id: &str,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            daemon_session_id: format!("feishu-question:{dispatch_id}"),
            numeric_session_id: numeric_session_id(dispatch_id),
            sequence: 0,
            emitted_at_ms: recorded_at_ms,
            chat_id: chat_id.to_owned(),
            thread_id: None,
            reply_to_message_id: None,
            response: response.to_owned(),
            kind: FeishuEgressKind::QuestionNotice,
            severity: RemoteNotificationSeverity::DirectReply,
        }
    }

    fn from_event(event: &SessionEventRecord) -> Result<Option<Self>, String> {
        match event.event_type.as_str() {
            TURN_COMPLETED_EVENT => Self::from_turn_event(event, false),
            TURN_FAILED_EVENT => Self::from_turn_event(event, true),
            _ => Ok(None),
        }
    }

    fn from_turn_event(event: &SessionEventRecord, failed: bool) -> Result<Option<Self>, String> {
        let projected = FeishuEventAdapter::project(event)
            .map_err(|error| format!("投影 Feishu Session Event 失败：{error}"))?;
        let payload = projected.payload;
        // 用户可见的完成消息只能由主 turn 的汇总回复产生：后台 Execution 的原始回复
        // 必须在这里被拦下，避免绕过主 turn 直接投递到飞书。
        if payload.get("kind").and_then(Value::as_str)
            != Some(axon_daemon::PRIMARY_TURN_ACTION_KIND)
        {
            return Ok(None);
        }

        let Some(context) = payload.get("transportContext") else {
            return Ok(None);
        };
        let Some(transport) = parse_feishu_transport_context(context)? else {
            return Ok(None);
        };
        // 由投递项派生的主 turn（Task 完成通知、Cron 投递）不在 egress 直接出站：
        // 它们的正文统一由持久收件箱投递器写 durable outbox，避免同一投递项重复出站。
        if transport.is_delivery_derived() {
            return Ok(None);
        }

        let response = payload
            .get("response")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let Some(response) = response else {
            return Ok(None);
        };
        let reply_to_message_id = transport.reply_to_message_id;
        let severity = if failed {
            RemoteNotificationSeverity::Warning
        } else {
            // 用户回合的最终回复是直接回答而非主动通知：必须立即出站，
            // 不得套用静默期延期，否则夜间提问会被推迟到静默期结束。
            RemoteNotificationSeverity::DirectReply
        };
        Ok(Some(Self {
            daemon_session_id: projected.session_id,
            numeric_session_id: numeric_session_id(&event.session_id.to_string()),
            sequence: projected.sequence,
            emitted_at_ms: event.emitted_at_ms,
            chat_id: transport.chat_id,
            thread_id: optional_string(context, "threadId"),
            reply_to_message_id,
            response: response.to_owned(),
            kind: FeishuEgressKind::TurnCompleted,
            severity,
        }))
    }
}

pub(crate) fn persist_egress(
    axon_root: &std::path::Path,
    input: FeishuEgressInput,
) -> Result<(), String> {
    let dispatch_id = dispatch_id(&input);
    let artifact_path = RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(
        axon_remote_feishu::remote_feishu_runtime_root(axon_root),
    );
    let outbox_path = RemoteNotificationOutboxRuntime::resolve_store_path(
        axon_remote_feishu::remote_feishu_runtime_root(axon_root),
    );
    let recorded_at_ms = input.emitted_at_ms;
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: dispatch_id.clone(),
            run_session_id: input.daemon_session_id.clone(),
            processing_message_id: input.reply_to_message_id.clone(),
            target: RemoteFeishuMessageTarget {
                receive_id: input.chat_id.clone(),
                receive_id_type: receive_id_type(&input.chat_id).to_owned(),
                reply_to_message_id: input.reply_to_message_id.clone(),
            },
            thread_id: input.thread_id.clone(),
            body: input.response.clone(),
            recorded_at_ms,
        },
    )
    .map_err(|error| format!("构造 Feishu 回复 artifact 失败：{error:?}"))?;
    RemoteFeishuAgentReplyArtifactRuntime::store_record(&artifact_path, &artifact)
        .map_err(|error| format!("写入 Feishu 回复 artifact 失败：{error:?}"))?;

    let record = outbox_record(&input, &dispatch_id, recorded_at_ms)?;
    match RemoteNotificationOutboxRuntime::append_timed_record(
        &outbox_path,
        &record,
        input.severity,
        RemoteNotificationTimingPolicy::default(),
        RemoteNotificationTimingPolicy::local_minute_of_day(),
    ) {
        Ok(report) => {
            if !report.evicted_dispatch_ids.is_empty() {
                RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(
                    &artifact_path,
                    &report.evicted_dispatch_ids,
                )
                .map_err(|error| format!("清理 Feishu LRU 回复 artifact 失败：{error:?}"))?;
            }
            Ok(())
        }
        Err(error) => {
            let _ = RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(
                &artifact_path,
                std::slice::from_ref(&dispatch_id),
            );
            Err(format!("写入 Feishu 回复 outbox 失败：{error:?}"))
        }
    }
}

fn outbox_record(
    input: &FeishuEgressInput,
    dispatch_id: &str,
    recorded_at_ms: u64,
) -> Result<RemoteNotificationOutboxRecord, String> {
    let wire_time_ms = i64::try_from(recorded_at_ms)
        .map_err(|_| "Feishu 回复事件时间超出 wire 协议范围".to_owned())?;
    let (notification_kind, lifecycle_status, title, reason, local_action_hint) = match input.kind {
        FeishuEgressKind::TurnCompleted
            if input.severity == RemoteNotificationSeverity::Warning =>
        {
            (
                RemoteNotificationKind::ThreadCompleted,
                WireLifecycleStatus::Completed,
                FEISHU_TURN_FAILURE_TITLE,
                FEISHU_TURN_FAILURE_REASON,
                FEISHU_REPLY_LOCAL_ACTION_HINT,
            )
        }
        FeishuEgressKind::TurnCompleted => (
            RemoteNotificationKind::ThreadCompleted,
            WireLifecycleStatus::Completed,
            FEISHU_REPLY_TITLE,
            FEISHU_EGRESS_REASON,
            FEISHU_REPLY_LOCAL_ACTION_HINT,
        ),
        FeishuEgressKind::QuestionNotice => (
            RemoteNotificationKind::InputRequired,
            WireLifecycleStatus::Completed,
            FEISHU_QUESTION_NOTICE_TITLE,
            FEISHU_QUESTION_NOTICE_REASON,
            FEISHU_QUESTION_NOTICE_ACTION_HINT,
        ),
    };
    let mut record = RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan: RemoteNotificationFanoutPlan {
            session_id: input.numeric_session_id,
            subscriber_id: FEISHU_DAEMON_SUBSCRIBER_ID.to_owned(),
            connection_id: input.chat_id.clone(),
            pairing_id: FEISHU_DAEMON_PAIRING_ID.to_owned(),
            transport: RemoteTransportKind::Feishu,
            subscribed_kinds: vec![notification_kind],
            delivered: vec![RemoteNotificationDelivery {
                kind: notification_kind,
                wire_event: remote_notification_wire_lifecycle_event(
                    input.numeric_session_id,
                    dispatch_id,
                    lifecycle_status,
                    title,
                    Some(bounded_preview(&input.response)),
                    wire_time_ms,
                ),
                bounded_body: bounded_preview(&input.response),
                reference_ids: vec![format!("session-event:{}", input.sequence)],
                local_action_hint: local_action_hint.to_owned(),
                severity: input.severity,
            }],
            dropped_count: 0,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
        },
        status: RemoteNotificationDispatchStatus::Pending,
        recorded_at_ms,
        updated_at_ms: recorded_at_ms,
        attempt_count: 0,
        next_retry_after_ms: None,
        last_error: None,
        adapter_message_id: None,
        source_trace: FEISHU_EGRESS_SOURCE_TRACE.to_owned(),
        reason: reason.to_owned(),
    })
    .map_err(|error| format!("构造 Feishu 回复 outbox 失败：{error:?}"))?;
    // 同一毫秒内的多条出站记录必须按来源 Session Event 的单调序号定序：
    // dispatch_id 是哈希值，其大小序与事件先后无关。
    record.sequence = Some(input.sequence);
    Ok(record)
}

fn numeric_session_id(value: &str) -> SessionId {
    let parsed = value
        .strip_prefix(FEISHU_DAEMON_SESSION_PREFIX)
        .unwrap_or(value)
        .parse::<u64>()
        .ok()
        .filter(|&num| num > 0);
    if let Some(num) = parsed {
        return SessionId(num);
    }
    // 目标态 interaction-xxxx 或任意非数字 session ID：哈希映射为稳定非零 u64
    let digest = axon_crypto::sha256_bytes(value.as_bytes());
    let num = u64::from_be_bytes(digest.as_bytes()[0..8].try_into().unwrap_or([1; 8]));
    SessionId(num.max(1))
}

fn dispatch_id(input: &FeishuEgressInput) -> String {
    let source = format!(
        "{}:{}:{}",
        input.daemon_session_id, input.sequence, input.chat_id
    );
    format!(
        "{FEISHU_EGRESS_DISPATCH_PREFIX}{}",
        sha256_hex_prefix(source.as_bytes(), FEISHU_DISPATCH_HASH_CHARS)
    )
}

fn receive_id_type(receive_id: &str) -> &'static str {
    if receive_id.starts_with("ou_") {
        "open_id"
    } else {
        "chat_id"
    }
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).map(str::trim)
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    string_field(value, key)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
}

fn bounded_preview(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= FEISHU_REPLY_PREVIEW_LIMIT {
        return trimmed.to_owned();
    }
    let mut preview = trimmed
        .chars()
        .take(FEISHU_REPLY_PREVIEW_LIMIT)
        .collect::<String>();
    preview.push_str("...");
    preview
}

#[cfg(test)]
#[path = "remote_feishu_daemon_egress_tests.rs"]
mod tests;
