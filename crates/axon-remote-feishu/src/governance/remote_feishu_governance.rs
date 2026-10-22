//! Feishu 远控治理状态与入站决策。
//!
//! 该模块补齐 Axon 的日用治理能力：
//! allowlist、持久化去重、线程处理态折叠与本地审计。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult, RemoteCapabilityPolicy};
use axon_remote_runtime::{RemoteStateStoreIndex, RemoteStateStoreRuntime};
use serde::{Deserialize, Serialize};

use crate::{
    RemoteFeishuCardActionEvent, RemoteFeishuIncomingMessage, RemoteFeishuWebSocketSessionRuntime,
    RemoteTransportDispatchReport,
};

/// Feishu 远控治理状态持久化文件名。
pub const REMOTE_FEISHU_GOVERNANCE_FILE: &str = "remote_feishu_governance.jsonl";
pub const REMOTE_FEISHU_GOVERNANCE_SCHEMA_VERSION: u32 = 1;
pub const REMOTE_FEISHU_GOVERNANCE_PREVIEW_MAX_CHARS: usize = 160;
/// Feishu 入站事件通过治理校验。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED: &str = "remote_feishu_governance:accepted";
/// Feishu 入站事件命中持久化去重。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_DUPLICATE: &str = "remote_feishu_governance:duplicate";
/// Feishu 入站事件被 allowlist 拒绝。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_ALLOWLIST_REJECTED: &str =
    "remote_feishu_governance:allowlist_rejected";
/// Feishu 群聊消息未满足 @ 机器人门禁。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_MENTION_REQUIRED: &str =
    "remote_feishu_governance:mention_required";
/// Feishu 无法确认 bot 身份，无法执行 mention gate。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_SELF_IDS_UNKNOWN: &str =
    "remote_feishu_governance:self_ids_unknown";
/// Feishu 文本消息未显式 @ 当前 bot。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_BOT_NOT_MENTIONED: &str =
    "remote_feishu_governance:bot_not_mentioned";
/// Feishu 事件来自 bot 自身，直接忽略。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_SELF_EVENT_DROPPED: &str =
    "remote_feishu_governance:self_event_dropped";
/// Feishu 入站事件命中 blacklist。
pub const REMOTE_FEISHU_GOVERNANCE_REASON_BLACKLIST_REJECTED: &str =
    "remote_feishu_governance:blacklist_rejected";

/// Feishu 远控治理配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGovernanceConfig {
    /// 允许访问远控的 sender open_id allowlist。
    pub allowed_sender_open_ids: Vec<String>,
    /// 禁止访问远控的 sender open_id blacklist。
    pub blocked_sender_open_ids: Vec<String>,
    /// 群聊消息是否要求显式 @ 机器人。
    pub require_mention: bool,
    /// 当前 bot open_id；用于 mention/self 过滤。
    pub bot_open_id: Option<String>,
    /// 按 chat_id 生效的群聊治理策略。
    pub chat_policies: Vec<RemoteFeishuChatGovernancePolicy>,
}

/// Feishu 单群治理策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuChatGovernancePolicy {
    /// Feishu chat_id。
    pub chat_id: String,
    /// 当前 chat 的 sender allowlist；非空时覆盖全局 allowlist。
    pub allowed_sender_open_ids: Vec<String>,
    /// 当前 chat 的 sender blacklist；与全局 blacklist 叠加生效。
    pub blocked_sender_open_ids: Vec<String>,
    /// 当前 chat 是否要求 @ 机器人；为空时沿用全局配置。
    pub require_mention: Option<bool>,
}

/// Feishu 入站治理决策状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteFeishuGovernanceDecisionStatus {
    /// 通过治理校验并进入远控主链。
    Accepted,
    /// 已命中持久化去重。
    DuplicateDropped,
    /// 因 sender 不在 allowlist 中被拒绝。
    RejectedByAllowlist,
    /// 因 sender 命中 blacklist 被拒绝。
    RejectedByBlacklist,
    /// 因群聊未显式 @ 机器人被拒绝。
    RejectedByMentionGate,
    /// 事件来自 bot 自身，直接忽略。
    IgnoredSelfEvent,
}

/// Feishu 线程当前处理态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteFeishuThreadState {
    /// 正在处理。
    Processing,
    /// 当前会话忙碌，已进入排队态。
    Queued,
    /// 因重复事件被丢弃。
    DuplicateDropped,
    /// 因治理策略被拒绝。
    Rejected,
}

/// 可持久化的 Feishu 远控治理记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFeishuGovernanceRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// 目标会话 ID。
    pub session_id: u64,
    /// chat_id 的 SHA-256。
    pub chat_id_sha256: String,
    /// request_id 的 SHA-256。
    pub request_id_sha256: String,
    /// sender open_id 的 SHA-256。
    pub sender_open_id_sha256: String,
    /// 有界文本预览。
    pub text_preview: String,
    /// Axon 入站文本分类；卡片动作或旧记录为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_kind: Option<String>,
    /// 是否满足 mention gate。
    pub mentioned: bool,
    /// 是否处于排队态。
    pub queued: bool,
    /// 当前会话是否忙碌。
    pub session_busy: bool,
    /// 治理决策状态。
    pub decision_status: RemoteFeishuGovernanceDecisionStatus,
    /// 线程处理态。
    pub thread_state: RemoteFeishuThreadState,
    /// 审计原因。
    pub reason: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// 单次 Feishu 入站治理决策结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGovernanceDecision {
    /// 治理决策状态。
    pub status: RemoteFeishuGovernanceDecisionStatus,
    /// 线程处理态。
    pub thread_state: RemoteFeishuThreadState,
    /// 审计原因。
    pub reason: String,
    /// 若通过治理校验，则返回远控主链 dispatch 报告。
    pub dispatch_report: Option<RemoteTransportDispatchReport>,
    /// 需要持久化的治理记录。
    pub record: RemoteFeishuGovernanceRecord,
}

/// Feishu 治理状态存储 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuGovernanceStoreRuntime;

/// Feishu 治理决策 runtime。
#[derive(Debug, Clone)]
pub struct RemoteFeishuGovernanceRuntime {
    session: RemoteFeishuWebSocketSessionRuntime,
}

impl RemoteFeishuGovernanceStoreRuntime {
    /// 解析治理状态存储路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_GOVERNANCE_FILE)
    }

    /// 追加治理记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &RemoteFeishuGovernanceRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        let key = record.request_id_sha256.clone();
        RemoteStateStoreRuntime::upsert(
            path,
            &key,
            RemoteStateStoreIndex::new(
                GOVERNANCE_STATE_NAMESPACE,
                record.session_id.to_string(),
                governance_status(record.decision_status),
                record.recorded_at_ms,
            ),
            record,
        )
    }

    /// 按请求 ID 读取单条治理记录，不扫描其他会话或历史记录。
    pub fn read_record(
        path: impl AsRef<Path>,
        request_id: &str,
    ) -> AxonResult<Option<RemoteFeishuGovernanceRecord>> {
        let key = sha256_hex(request_id);
        RemoteStateStoreRuntime::read(path, &key)
    }

    /// 读取当前治理记录，不读取历史版本。
    pub fn read_records(path: impl AsRef<Path>) -> AxonResult<Vec<RemoteFeishuGovernanceRecord>> {
        RemoteStateStoreRuntime::read_all(path)?
            .into_iter()
            .map(|record| {
                validate_record(&record)?;
                Ok(record)
            })
            .collect()
    }

    /// 按会话与 chat 折叠为每条线程的最新治理态。
    #[must_use]
    pub fn latest_threads(
        records: &[RemoteFeishuGovernanceRecord],
    ) -> Vec<RemoteFeishuGovernanceRecord> {
        let mut latest = BTreeMap::<(u64, String), RemoteFeishuGovernanceRecord>::new();
        for record in records {
            let key = (record.session_id, record.chat_id_sha256.clone());
            latest
                .entry(key)
                .and_modify(|current| {
                    if record.recorded_at_ms >= current.recorded_at_ms {
                        *current = record.clone();
                    }
                })
                .or_insert_with(|| record.clone());
        }
        let mut values = latest.into_values().collect::<Vec<_>>();
        values.sort_by_key(|record| std::cmp::Reverse(record.recorded_at_ms));
        values
    }

    /// 判断 request_id 是否已出现在持久化治理记录中。
    #[must_use]
    pub fn contains_request_id(records: &[RemoteFeishuGovernanceRecord], request_id: &str) -> bool {
        let hashed = sha256_hex(request_id);
        records
            .iter()
            .any(|record| record.request_id_sha256 == hashed)
    }
}

impl RemoteFeishuGovernanceRuntime {
    /// 创建 Feishu 治理 runtime。
    #[must_use]
    pub fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self {
            session: RemoteFeishuWebSocketSessionRuntime::new(policy),
        }
    }

    /// 处理文本消息的治理决策。
    pub fn handle_message(
        &self,
        config: &RemoteFeishuGovernanceConfig,
        event: &RemoteFeishuIncomingMessage,
        pairing_id: &str,
        received_at_ms: u64,
        existing_records: &[RemoteFeishuGovernanceRecord],
    ) -> AxonResult<RemoteFeishuGovernanceDecision> {
        let mentioned = is_message_mentioned(config, event);
        if RemoteFeishuGovernanceStoreRuntime::contains_request_id(
            existing_records,
            &event.request_id,
        ) {
            return Ok(rejected_decision(
                event,
                mentioned,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::DuplicateDropped,
                RemoteFeishuThreadState::DuplicateDropped,
                REMOTE_FEISHU_GOVERNANCE_REASON_DUPLICATE,
            ));
        }
        if is_self_event(config, event) {
            return Ok(rejected_decision(
                event,
                mentioned,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::IgnoredSelfEvent,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_SELF_EVENT_DROPPED,
            ));
        }
        if is_sender_blocked(config, &event.chat_id, &event.sender_open_id) {
            return Ok(rejected_decision(
                event,
                mentioned,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::RejectedByBlacklist,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_BLACKLIST_REJECTED,
            ));
        }
        if !is_sender_allowed(config, &event.chat_id, &event.sender_open_id) {
            return Ok(rejected_decision(
                event,
                mentioned,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_ALLOWLIST_REJECTED,
            ));
        }
        if event.chat_type.requires_group_mention()
            && require_mention_for_chat(config, &event.chat_id)
            && !mentioned
        {
            return Ok(rejected_decision(
                event,
                mentioned,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_BOT_NOT_MENTIONED,
            ));
        }
        let dispatch_report = self
            .session
            .handle_message(event, pairing_id, received_at_ms)?;
        let thread_state = if event.session_busy {
            RemoteFeishuThreadState::Queued
        } else {
            RemoteFeishuThreadState::Processing
        };
        Ok(RemoteFeishuGovernanceDecision {
            status: RemoteFeishuGovernanceDecisionStatus::Accepted,
            thread_state,
            reason: REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED.to_owned(),
            dispatch_report: Some(dispatch_report),
            record: message_record(
                event,
                mentioned,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::Accepted,
                thread_state,
                REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED,
            ),
        })
    }

    /// 处理卡片动作的治理决策。
    pub fn handle_card_action(
        &self,
        config: &RemoteFeishuGovernanceConfig,
        event: &RemoteFeishuCardActionEvent,
        pairing_id: &str,
        received_at_ms: u64,
        existing_records: &[RemoteFeishuGovernanceRecord],
    ) -> AxonResult<RemoteFeishuGovernanceDecision> {
        if RemoteFeishuGovernanceStoreRuntime::contains_request_id(
            existing_records,
            &event.request_id,
        ) {
            return Ok(RemoteFeishuGovernanceDecision {
                status: RemoteFeishuGovernanceDecisionStatus::DuplicateDropped,
                thread_state: RemoteFeishuThreadState::DuplicateDropped,
                reason: REMOTE_FEISHU_GOVERNANCE_REASON_DUPLICATE.to_owned(),
                dispatch_report: None,
                record: card_action_record(
                    event,
                    received_at_ms,
                    RemoteFeishuGovernanceDecisionStatus::DuplicateDropped,
                    RemoteFeishuThreadState::DuplicateDropped,
                    REMOTE_FEISHU_GOVERNANCE_REASON_DUPLICATE,
                ),
            });
        }
        if is_self_card_action(config, event) {
            return Ok(rejected_card_action_decision(
                event,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::IgnoredSelfEvent,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_SELF_EVENT_DROPPED,
            ));
        }
        if is_sender_blocked(config, &event.chat_id, &event.operator_open_id) {
            return Ok(rejected_card_action_decision(
                event,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::RejectedByBlacklist,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_BLACKLIST_REJECTED,
            ));
        }
        if !is_sender_allowed(config, &event.chat_id, &event.operator_open_id) {
            return Ok(rejected_card_action_decision(
                event,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist,
                RemoteFeishuThreadState::Rejected,
                REMOTE_FEISHU_GOVERNANCE_REASON_ALLOWLIST_REJECTED,
            ));
        }
        let dispatch_report = self
            .session
            .handle_card_action(event, pairing_id, received_at_ms)?;
        let thread_state = if event.session_busy {
            RemoteFeishuThreadState::Queued
        } else {
            RemoteFeishuThreadState::Processing
        };
        Ok(RemoteFeishuGovernanceDecision {
            status: RemoteFeishuGovernanceDecisionStatus::Accepted,
            thread_state,
            reason: REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED.to_owned(),
            dispatch_report: Some(dispatch_report),
            record: card_action_record(
                event,
                received_at_ms,
                RemoteFeishuGovernanceDecisionStatus::Accepted,
                thread_state,
                REMOTE_FEISHU_GOVERNANCE_REASON_ACCEPTED,
            ),
        })
    }
}

fn rejected_decision(
    event: &RemoteFeishuIncomingMessage,
    mentioned: bool,
    recorded_at_ms: u64,
    status: RemoteFeishuGovernanceDecisionStatus,
    thread_state: RemoteFeishuThreadState,
    reason: &'static str,
) -> RemoteFeishuGovernanceDecision {
    RemoteFeishuGovernanceDecision {
        status,
        thread_state,
        reason: reason.to_owned(),
        dispatch_report: None,
        record: message_record(
            event,
            mentioned,
            recorded_at_ms,
            status,
            thread_state,
            reason,
        ),
    }
}

fn rejected_card_action_decision(
    event: &RemoteFeishuCardActionEvent,
    recorded_at_ms: u64,
    status: RemoteFeishuGovernanceDecisionStatus,
    thread_state: RemoteFeishuThreadState,
    reason: &'static str,
) -> RemoteFeishuGovernanceDecision {
    RemoteFeishuGovernanceDecision {
        status,
        thread_state,
        reason: reason.to_owned(),
        dispatch_report: None,
        record: card_action_record(event, recorded_at_ms, status, thread_state, reason),
    }
}

fn message_record(
    event: &RemoteFeishuIncomingMessage,
    mentioned: bool,
    recorded_at_ms: u64,
    status: RemoteFeishuGovernanceDecisionStatus,
    thread_state: RemoteFeishuThreadState,
    reason: &'static str,
) -> RemoteFeishuGovernanceRecord {
    RemoteFeishuGovernanceRecord {
        schema_version: REMOTE_FEISHU_GOVERNANCE_SCHEMA_VERSION,
        session_id: event.session_id.0,
        chat_id_sha256: sha256_hex(&event.chat_id),
        request_id_sha256: sha256_hex(&event.request_id),
        sender_open_id_sha256: sha256_hex(&event.sender_open_id),
        text_preview: bounded_preview(&event.text),
        message_kind: Some(event.message_kind.as_str().to_owned()),
        mentioned,
        queued: event.session_busy,
        session_busy: event.session_busy,
        decision_status: status,
        thread_state,
        reason: reason.to_owned(),
        recorded_at_ms,
    }
}

fn card_action_record(
    event: &RemoteFeishuCardActionEvent,
    recorded_at_ms: u64,
    status: RemoteFeishuGovernanceDecisionStatus,
    thread_state: RemoteFeishuThreadState,
    reason: &'static str,
) -> RemoteFeishuGovernanceRecord {
    RemoteFeishuGovernanceRecord {
        schema_version: REMOTE_FEISHU_GOVERNANCE_SCHEMA_VERSION,
        session_id: event.session_id.0,
        chat_id_sha256: sha256_hex(&event.chat_id),
        request_id_sha256: sha256_hex(&event.request_id),
        sender_open_id_sha256: sha256_hex(&event.operator_open_id),
        text_preview: bounded_preview(&event.action_tag),
        message_kind: None,
        mentioned: true,
        queued: event.session_busy,
        session_busy: event.session_busy,
        decision_status: status,
        thread_state,
        reason: reason.to_owned(),
        recorded_at_ms,
    }
}

fn bounded_preview(text: &str) -> String {
    text.chars()
        .take(REMOTE_FEISHU_GOVERNANCE_PREVIEW_MAX_CHARS)
        .collect()
}

fn is_message_mentioned(
    config: &RemoteFeishuGovernanceConfig,
    event: &RemoteFeishuIncomingMessage,
) -> bool {
    if let Some(bot_open_id) = config.bot_open_id.as_deref() {
        return crate::RemoteFeishuWebSocketIngressRuntime::message_mentions_bot(
            event,
            bot_open_id,
        );
    }
    !event.mentions.is_empty()
}

fn is_self_event(
    config: &RemoteFeishuGovernanceConfig,
    event: &RemoteFeishuIncomingMessage,
) -> bool {
    config
        .bot_open_id
        .as_deref()
        .is_some_and(|bot_open_id| bot_open_id == event.sender_open_id)
}

fn is_self_card_action(
    config: &RemoteFeishuGovernanceConfig,
    event: &RemoteFeishuCardActionEvent,
) -> bool {
    config
        .bot_open_id
        .as_deref()
        .is_some_and(|bot_open_id| bot_open_id == event.operator_open_id)
}

fn is_sender_blocked(config: &RemoteFeishuGovernanceConfig, chat_id: &str, sender: &str) -> bool {
    config
        .blocked_sender_open_ids
        .iter()
        .any(|blocked| blocked == sender)
        || chat_policy(config, chat_id).is_some_and(|policy| {
            policy
                .blocked_sender_open_ids
                .iter()
                .any(|blocked| blocked == sender)
        })
}

fn is_sender_allowed(config: &RemoteFeishuGovernanceConfig, chat_id: &str, sender: &str) -> bool {
    let chat_allowed = chat_policy(config, chat_id)
        .map(|policy| policy.allowed_sender_open_ids.as_slice())
        .unwrap_or_default();
    let allowed = if chat_allowed.is_empty() {
        config.allowed_sender_open_ids.as_slice()
    } else {
        chat_allowed
    };
    allowed.is_empty() || allowed.iter().any(|open_id| open_id == sender)
}

fn require_mention_for_chat(config: &RemoteFeishuGovernanceConfig, chat_id: &str) -> bool {
    chat_policy(config, chat_id)
        .and_then(|policy| policy.require_mention)
        .unwrap_or(config.require_mention)
}

fn chat_policy<'a>(
    config: &'a RemoteFeishuGovernanceConfig,
    chat_id: &str,
) -> Option<&'a RemoteFeishuChatGovernancePolicy> {
    config
        .chat_policies
        .iter()
        .find(|policy| policy.chat_id == chat_id)
}

fn validate_record(record: &RemoteFeishuGovernanceRecord) -> AxonResult<()> {
    if record.schema_version != REMOTE_FEISHU_GOVERNANCE_SCHEMA_VERSION
        || record.session_id == 0
        || record.chat_id_sha256.trim().is_empty()
        || record.request_id_sha256.trim().is_empty()
        || record.sender_open_id_sha256.trim().is_empty()
        || record.text_preview.trim().is_empty()
        || record.reason.trim().is_empty()
        || record.recorded_at_ms == 0
    {
        return Err(AxonError::runtime_failure("Feishu 治理记录无效"));
    }
    Ok(())
}

const GOVERNANCE_STATE_NAMESPACE: &str = "feishu_governance";
const GOVERNANCE_STATUS_ACCEPTED: &str = "accepted";
const GOVERNANCE_STATUS_DUPLICATE: &str = "duplicate_dropped";
const GOVERNANCE_STATUS_ALLOWLIST: &str = "rejected_by_allowlist";
const GOVERNANCE_STATUS_BLACKLIST: &str = "rejected_by_blacklist";
const GOVERNANCE_STATUS_MENTION: &str = "rejected_by_mention_gate";
const GOVERNANCE_STATUS_SELF: &str = "ignored_self_event";

fn governance_status(status: RemoteFeishuGovernanceDecisionStatus) -> &'static str {
    match status {
        RemoteFeishuGovernanceDecisionStatus::Accepted => GOVERNANCE_STATUS_ACCEPTED,
        RemoteFeishuGovernanceDecisionStatus::DuplicateDropped => GOVERNANCE_STATUS_DUPLICATE,
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist => GOVERNANCE_STATUS_ALLOWLIST,
        RemoteFeishuGovernanceDecisionStatus::RejectedByBlacklist => GOVERNANCE_STATUS_BLACKLIST,
        RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate => GOVERNANCE_STATUS_MENTION,
        RemoteFeishuGovernanceDecisionStatus::IgnoredSelfEvent => GOVERNANCE_STATUS_SELF,
    }
}
