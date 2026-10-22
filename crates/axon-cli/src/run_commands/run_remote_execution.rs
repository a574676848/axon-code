//! Daemon 组合根的 Provider runner 与本地 Feishu 通知写入。

use std::path::{Path, PathBuf};

use axon_crypto::sha256_hex_prefix;
use axon_domain::SessionId;
use axon_remote_feishu::{
    RemoteFeishuAgentReplyArtifactInput, RemoteFeishuAgentReplyArtifactRuntime,
    RemoteFeishuFeedbackTaskStoreRuntime, RemoteFeishuMessageTarget,
    RemoteFeishuProcessingCleanupTaskInput, RemoteFeishuProcessingOutcome,
};
use axon_remote_runtime::{
    RemoteNotificationDelivery, RemoteNotificationDispatchStatus, RemoteNotificationFanoutPlan,
    RemoteNotificationKind, RemoteNotificationOutboxInput, RemoteNotificationOutboxRecord,
    RemoteNotificationOutboxRuntime, RemoteNotificationSeverity, RemoteNotificationTimingPolicy,
    RemoteTransportKind, WireLifecycleStatus, remote_notification_wire_lifecycle_event,
};
use axon_session_runtime::ProviderTurnRequest;

use crate::remote_commands::remote_feishu_transport_context::parse as parse_feishu_transport_context;

#[path = "run_remote_provider_turn.rs"]
mod run_remote_provider_turn;

pub(crate) use run_remote_provider_turn::CliProviderTurnRunner;

const FEISHU_NOTIFICATION_SUBSCRIBER: &str = "feishu-gateway";
const FEISHU_AGENT_SOURCE_TRACE: &str = "remote_feishu:local_notification";
const FEISHU_AGENT_OUTBOX_REASON: &str = "remote_feishu:local_notification_planned";
const FEISHU_AGENT_DISPATCH_PREFIX: &str = "feishu-agent-";
const FEISHU_AGENT_REPLY_REFERENCE_PREFIX: &str = "feishu-agent-reply";
const FEISHU_LOCAL_ACTION_HINT: &str = "Feishu 回复当前会话";
const FEISHU_REPLY_TITLE: &str = "远控线程完成";
const FEISHU_DISPATCH_HASH_CHARS: usize = 16;
const FEISHU_REPLY_PREVIEW_LIMIT: usize = 240;
const FEISHU_PROGRESS_DISPATCH_PREFIX: &str = "feishu-progress-";
const FEISHU_PROGRESS_HASH_CHARS: usize = 24;
/// 历史 Feishu 会话 ID 前缀，形如 `feishu-session-<n>`。
const FEISHU_SESSION_ID_PREFIX: &str = "feishu-session-";
/// 会话 ID 映射为本地 `SessionId` 时取用的十六进制字符数（16 字符 = 64 位）。
const FEISHU_SESSION_HASH_CHARS: usize = 16;

#[derive(Clone)]
pub(crate) struct FeishuTurnContext {
    app_id: String,
    api_base_url: Option<String>,
    dispatch_id: String,
    feedback_store_path: PathBuf,
    app_secret_env: String,
    session_id: SessionId,
    source_message_id: Option<String>,
}

fn resolve_feishu_turn_context(
    config_path: &Path,
    request: &ProviderTurnRequest,
) -> Result<Option<FeishuTurnContext>, String> {
    let Some(transport) = request.transport_context.as_ref() else {
        return Ok(None);
    };
    let Some(context) = parse_feishu_transport_context(transport)? else {
        return Ok(None);
    };
    // 投递项派生的主 turn 正文由持久收件箱统一出站，实时投影必须让位以避免重复投递。
    if context.is_delivery_derived() {
        return Ok(None);
    }
    crate::run_commands::load_run_env_file(&crate::setup_commands::env_path_for_config(
        config_path,
    ))?;
    let loaded = axon_config::ConfigLoader::load_from_path(config_path)
        .map_err(|error| format!("读取 Feishu 实时进度配置失败：{error:?}"))?;
    let config = loaded.config.remote.feishu_websocket;
    if !config.enabled || !config.enable_notification_sending {
        return Ok(None);
    }
    let source_message_id = context.source_message_id;
    let axon_root = crate::runtime_paths::runtime_axon_root_path(config_path)?;
    let progress_root = axon_root.clone();
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(axon_root);
    let dispatch_id = progress_dispatch_id(transport, &request.run_session_id);
    // 实时投影按 session 归属回合：已提交的普通流式事件不携带 transportContext，
    // egress 侧只能靠这份登记把增量路由到正确的会话与卡片。
    crate::remote_commands::remote_feishu_progress_service::register_turn(
        progress_root,
        config_path.to_path_buf(),
        &request.session_id,
        crate::remote_commands::remote_feishu_progress_service::ProgressTurn {
            dispatch_id: dispatch_id.clone(),
            chat_id: context.chat_id,
            reply_to_message_id: context.reply_to_message_id,
        },
    );
    Ok(Some(FeishuTurnContext {
        feedback_store_path: RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(
            &runtime_root,
        ),
        app_secret_env: config.app_secret_env,
        session_id: daemon_session_id(&request.session_id)?,
        source_message_id,
        app_id: config.app_id,
        api_base_url: (!config.api_base_url.trim().is_empty()).then_some(config.api_base_url),
        dispatch_id,
    }))
}

fn transport_files(transport: Option<&serde_json::Value>) -> Vec<PathBuf> {
    transport
        .and_then(|value| value.get("files"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(PathBuf::from)
        // 附件缓存可能已过期回收：此时只丢附件，绝不因单个附件缺失而拒绝整轮 turn。
        .filter(|path| path.is_file())
        .collect()
}

fn progress_dispatch_id(transport: &serde_json::Value, run_session_id: &str) -> String {
    let source = transport
        .get("cursor")
        .and_then(|cursor| cursor.get("eventId"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            transport
                .get("sourceMessageId")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or(run_session_id);
    format!(
        "{FEISHU_PROGRESS_DISPATCH_PREFIX}{}",
        sha256_hex_prefix(source.as_bytes(), FEISHU_PROGRESS_HASH_CHARS)
    )
}

fn complete_feishu_processing(context: &FeishuTurnContext, outcome: RemoteFeishuProcessingOutcome) {
    let Some(message_id) = context.source_message_id.as_deref() else {
        return;
    };
    let recorded_at_ms = crate::run_commands::run_execution::now_ms();
    let task = RemoteFeishuFeedbackTaskStoreRuntime::task_from_cleanup(
        RemoteFeishuProcessingCleanupTaskInput {
            app_id: &context.app_id,
            app_secret_env: &context.app_secret_env,
            session_id: context.session_id,
            dispatch_id: &context.dispatch_id,
            message_id,
            outcome,
            api_base_url: context.api_base_url.as_deref(),
            recorded_at_ms,
        },
    );
    if let Err(error) =
        RemoteFeishuFeedbackTaskStoreRuntime::append_record(&context.feedback_store_path, &task)
    {
        axon_logger::warning(&format!(
            "写入 Feishu Typing feedback task 失败，不阻塞 Provider Worker: {error:?}"
        ));
    }
}

/// 仅用于不经 Agent 的本地 Feishu 回复或服务告警。
#[derive(Debug, Clone)]
pub(crate) struct RemoteAgentExecutionInput<'a> {
    pub(crate) axon_root: &'a Path,
    pub(crate) axon_session_id: SessionId,
    pub(crate) run_session_key: &'a str,
    pub(crate) feishu_chat_id: &'a str,
    pub(crate) processing_message_id: Option<&'a str>,
    pub(crate) reply_to_message_id: Option<&'a str>,
    pub(crate) thread_id: Option<&'a str>,
    pub(crate) pairing_id: &'a str,
    pub(crate) delivery_id: &'a str,
    pub(crate) recorded_at_ms: u64,
}

/// 将无需 Agent 的文本写入 Feishu artifact 与 outbox。
pub(crate) fn enqueue_remote_agent_reply(
    input: &RemoteAgentExecutionInput<'_>,
    body: &str,
) -> Result<String, String> {
    persist_notification(input, body, RemoteNotificationSeverity::DirectReply)
}

/// 将 Gateway 告警写入 Feishu artifact 与 outbox。
pub(crate) fn enqueue_remote_gateway_alert(
    input: &RemoteAgentExecutionInput<'_>,
    body: &str,
) -> Result<String, String> {
    persist_notification(input, body, RemoteNotificationSeverity::Warning)
}

fn persist_notification(
    input: &RemoteAgentExecutionInput<'_>,
    body: &str,
    severity: RemoteNotificationSeverity,
) -> Result<String, String> {
    let dispatch_id = dispatch_id(input);
    let artifact_path = RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(input.axon_root);
    let artifact = RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
        RemoteFeishuAgentReplyArtifactInput {
            dispatch_id: dispatch_id.clone(),
            run_session_id: input.run_session_key.to_owned(),
            processing_message_id: input.processing_message_id.map(ToOwned::to_owned),
            target: RemoteFeishuMessageTarget {
                receive_id: input.feishu_chat_id.to_owned(),
                receive_id_type: receive_id_type(input.feishu_chat_id).to_owned(),
                reply_to_message_id: input.reply_to_message_id.map(ToOwned::to_owned),
            },
            thread_id: input.thread_id.map(ToOwned::to_owned),
            body: body.to_owned(),
            recorded_at_ms: input.recorded_at_ms,
        },
    )
    .map_err(|error| format!("构造 Feishu 回复 artifact 失败：{error:?}"))?;
    RemoteFeishuAgentReplyArtifactRuntime::store_record(&artifact_path, &artifact)
        .map_err(|error| format!("写入 Feishu 回复 artifact 失败：{error:?}"))?;

    let record = outbox_record(input, body, &dispatch_id, severity)?;
    append_outbox_record(input, &artifact_path, &record, dispatch_id, severity)
}

fn append_outbox_record(
    input: &RemoteAgentExecutionInput<'_>,
    artifact_path: &Path,
    record: &RemoteNotificationOutboxRecord,
    dispatch_id: String,
    severity: RemoteNotificationSeverity,
) -> Result<String, String> {
    let append_result = RemoteNotificationOutboxRuntime::append_timed_record(
        RemoteNotificationOutboxRuntime::resolve_store_path(input.axon_root),
        record,
        severity,
        RemoteNotificationTimingPolicy::default(),
        RemoteNotificationTimingPolicy::local_minute_of_day(),
    );
    match append_result {
        Ok(report) => {
            if let Some(coalesced_dispatch_id) = report.coalesced_dispatch_id.as_deref() {
                RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(
                    artifact_path,
                    std::slice::from_ref(&dispatch_id),
                )
                .map_err(|error| format!("清理 Feishu 合并通知 artifact 失败：{error:?}"))?;
                return Ok(coalesced_dispatch_id.to_owned());
            }
            if !report.evicted_dispatch_ids.is_empty() {
                RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(
                    artifact_path,
                    &report.evicted_dispatch_ids,
                )
                .map_err(|error| format!("清理 Feishu LRU artifact 失败：{error:?}"))?;
            }
            Ok(dispatch_id)
        }
        Err(error) => {
            let _ = RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(
                artifact_path,
                std::slice::from_ref(&dispatch_id),
            );
            Err(format!("写入 Feishu outbox 失败：{error:?}"))
        }
    }
}

fn outbox_record(
    input: &RemoteAgentExecutionInput<'_>,
    body: &str,
    dispatch_id: &str,
    severity: RemoteNotificationSeverity,
) -> Result<RemoteNotificationOutboxRecord, String> {
    let wire_time_ms = i64::try_from(input.recorded_at_ms)
        .map_err(|_| "Feishu 回复时间超出 wire 协议范围".to_owned())?;
    RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan: RemoteNotificationFanoutPlan {
            session_id: input.axon_session_id,
            subscriber_id: FEISHU_NOTIFICATION_SUBSCRIBER.to_owned(),
            connection_id: input.feishu_chat_id.to_owned(),
            pairing_id: input.pairing_id.to_owned(),
            transport: RemoteTransportKind::Feishu,
            subscribed_kinds: vec![RemoteNotificationKind::ThreadCompleted],
            delivered: vec![notification_delivery(
                input,
                body,
                dispatch_id,
                severity,
                wire_time_ms,
            )],
            dropped_count: 0,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
        },
        status: RemoteNotificationDispatchStatus::Pending,
        recorded_at_ms: input.recorded_at_ms,
        updated_at_ms: input.recorded_at_ms,
        attempt_count: 0,
        next_retry_after_ms: None,
        last_error: None,
        adapter_message_id: None,
        source_trace: FEISHU_AGENT_SOURCE_TRACE.to_owned(),
        reason: if matches!(
            severity,
            RemoteNotificationSeverity::Info | RemoteNotificationSeverity::DirectReply
        ) {
            format!(
                "{FEISHU_AGENT_OUTBOX_REASON}:{}",
                sha256_hex_prefix(body.as_bytes(), FEISHU_DISPATCH_HASH_CHARS)
            )
        } else {
            FEISHU_AGENT_OUTBOX_REASON.to_owned()
        },
    })
    .map_err(|error| format!("构造 Feishu outbox 失败：{error:?}"))
}

fn notification_delivery(
    input: &RemoteAgentExecutionInput<'_>,
    body: &str,
    dispatch_id: &str,
    severity: RemoteNotificationSeverity,
    wire_time_ms: i64,
) -> RemoteNotificationDelivery {
    RemoteNotificationDelivery {
        kind: RemoteNotificationKind::ThreadCompleted,
        wire_event: remote_notification_wire_lifecycle_event(
            input.axon_session_id,
            dispatch_id,
            WireLifecycleStatus::Completed,
            FEISHU_REPLY_TITLE,
            Some(bounded_preview(body)),
            wire_time_ms,
        ),
        bounded_body: bounded_preview(body),
        reference_ids: vec![
            format!("remote-delivery:{}", input.delivery_id),
            format!("{FEISHU_AGENT_REPLY_REFERENCE_PREFIX}:{dispatch_id}"),
        ],
        local_action_hint: FEISHU_LOCAL_ACTION_HINT.to_owned(),
        severity,
    }
}

fn dispatch_id(input: &RemoteAgentExecutionInput<'_>) -> String {
    let source = format!("{}:{}", input.delivery_id, input.recorded_at_ms);
    format!(
        "{FEISHU_AGENT_DISPATCH_PREFIX}{}",
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

fn bounded_preview(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= FEISHU_REPLY_PREVIEW_LIMIT {
        trimmed.to_owned()
    } else {
        let mut preview = trimmed
            .chars()
            .take(FEISHU_REPLY_PREVIEW_LIMIT)
            .collect::<String>();
        preview.push_str("...");
        preview
    }
}

fn daemon_session_id(session_id: &str) -> Result<SessionId, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("Feishu session ID 为空".to_owned());
    }
    // 历史形态保留数值原义，既有反馈任务分区不受影响。
    if let Some(value) = session_id.strip_prefix(FEISHU_SESSION_ID_PREFIX) {
        let value = value
            .parse::<u64>()
            .map_err(|_| "Feishu run session ID 数值无效".to_owned())?;
        if value == 0 {
            return Err("Feishu run session ID 不能为零".to_owned());
        }
        return Ok(SessionId(value));
    }
    // 现行 Daemon 会话 ID 已演进为 `interaction-feishu-<sha256>`；该映射只承担
    // 反馈任务分区与投递路由，按哈希稳定派生即可，不得再以旧前缀拒绝整轮 turn。
    let digest = sha256_hex_prefix(session_id.as_bytes(), FEISHU_SESSION_HASH_CHARS);
    let value =
        u64::from_str_radix(&digest, 16).map_err(|_| "Feishu session ID 哈希无效".to_owned())?;
    if value == 0 {
        return Err("Feishu session ID 哈希不能为零".to_owned());
    }
    Ok(SessionId(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_notification_reason_is_content_addressed_for_safe_coalescing() {
        let input = RemoteAgentExecutionInput {
            axon_root: Path::new("."),
            axon_session_id: SessionId(1),
            run_session_key: "feishu-session-1",
            feishu_chat_id: "oc_chat",
            processing_message_id: None,
            reply_to_message_id: None,
            thread_id: None,
            pairing_id: "pairing-1",
            delivery_id: "delivery-1",
            recorded_at_ms: 1,
        };
        let first = outbox_record(
            &input,
            "first",
            "dispatch-1",
            RemoteNotificationSeverity::Info,
        )
        .expect("first");
        let duplicate = outbox_record(
            &input,
            "first",
            "dispatch-2",
            RemoteNotificationSeverity::Info,
        )
        .expect("duplicate");
        let distinct = outbox_record(
            &input,
            "second",
            "dispatch-3",
            RemoteNotificationSeverity::Info,
        )
        .expect("distinct");
        assert_eq!(first.reason, duplicate.reason);
        assert_ne!(first.reason, distinct.reason);
    }

    #[test]
    fn session_id_mapping_accepts_legacy_and_interaction_forms() {
        // 历史形态保持数值原义。
        assert_eq!(daemon_session_id("feishu-session-9").unwrap(), SessionId(9));
        // 现行 Daemon 形态必须稳定映射到同一个非零 SessionId。
        let interaction =
            "interaction-feishu-e31658763db2884407e66133115f1fde05996d5b78f0847d981e7146634dff2e";
        let mapped = daemon_session_id(interaction).expect("现行会话 ID 必须可映射");
        assert_ne!(mapped, SessionId(0));
        assert_eq!(mapped, daemon_session_id(interaction).unwrap());
        assert_ne!(
            mapped,
            daemon_session_id("interaction-feishu-a303f0c6f6a6a49d1405701d56378f5a").unwrap()
        );
        assert!(daemon_session_id("").is_err());
        assert!(daemon_session_id("feishu-session-0").is_err());
        assert!(daemon_session_id("feishu-session-abc").is_err());
    }
}
