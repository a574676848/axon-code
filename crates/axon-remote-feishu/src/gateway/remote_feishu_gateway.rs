//! Feishu 远控独立 gateway 前置运行态。
//!
//! 本模块不直接连接 Feishu SDK，而是先固化真实 WebSocket 常驻网关需要的
//! pending inbound queue、per-chat 入站串行锁与 shutdown drain 语义。

use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult, QuotedMessageContext};
use serde_json::Value;

use crate::{
    RemoteFeishuCardActionEvent, RemoteFeishuChatType, RemoteFeishuGovernanceConfig,
    RemoteFeishuGovernanceDecision, RemoteFeishuGovernanceDecisionStatus,
    RemoteFeishuGovernanceRecord, RemoteFeishuGovernanceRuntime,
    RemoteFeishuGovernanceStoreRuntime, RemoteFeishuIncomingMessage,
    RemoteFeishuIncomingMessageKind, RemoteFeishuReplyContext,
};

/// Feishu gateway 默认 pending inbound 深度。
pub const REMOTE_FEISHU_GATEWAY_DEFAULT_PENDING_DEPTH: usize = 256;
/// Feishu gateway 默认文本批量最大消息数。
pub const REMOTE_FEISHU_GATEWAY_TEXT_BATCH_MAX_MESSAGES: usize = 8;
/// Feishu gateway 默认文本批量最大字符数。
pub const REMOTE_FEISHU_GATEWAY_TEXT_BATCH_MAX_CHARS: usize = 4000;
/// 小文本附件注入 Agent prompt 的最大字节数。
pub const REMOTE_FEISHU_GATEWAY_TEXT_ATTACHMENT_INLINE_MAX_BYTES: u64 = 64 * 1024;
/// Feishu gateway 入站事件已入队。
pub const REMOTE_FEISHU_GATEWAY_REASON_ENQUEUED: &str = "remote_feishu_gateway:enqueued";
/// Feishu gateway 入站事件等待同 chat 前序 turn 完成。
pub const REMOTE_FEISHU_GATEWAY_REASON_DEFERRED_BY_CHAT: &str =
    "remote_feishu_gateway:deferred_by_chat";
/// Feishu gateway 入站事件已进入本轮 drain。
pub const REMOTE_FEISHU_GATEWAY_REASON_DRAINED: &str = "remote_feishu_gateway:drained";
/// Feishu gateway 正在 shutdown，拒绝新入站。
pub const REMOTE_FEISHU_GATEWAY_REASON_SHUTTING_DOWN: &str = "remote_feishu_gateway:shutting_down";
const CARD_ACTION_CONTROL_APPROVE: &str = "approve";
const CARD_ACTION_CONTROL_DENY: &str = "deny";
const CARD_ACTION_CONTROL_CLARIFY: &str = "clarify";
const CARD_ACTION_AXON_ACTION_KEY: &str = "axon_action";
const CARD_ACTION_ACTION_KEY: &str = "action";
const CARD_ACTION_COMMAND_KEY: &str = "command";
const CARD_ACTION_LABEL_KEY: &str = "label";
/// 问答作答按钮的 `axon_action` 取值；由飞书问答卡片构造器写入。
const CARD_ACTION_QUESTION_ANSWER: &str = "answer_question";
const CARD_ACTION_VALUE_PREVIEW_MAX_CHARS: usize = 240;

/// Feishu gateway 连接状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFeishuGatewayConnectionState {
    /// 尚未连接。
    Disconnected,
    /// 已进入常驻接收态。
    Connected,
    /// 正在停止。
    ShuttingDown,
}

/// Feishu gateway 入站事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFeishuGatewayInboundEvent {
    /// 文本消息。
    Message(RemoteFeishuIncomingMessage),
    /// 卡片动作。
    CardAction(RemoteFeishuCardActionEvent),
}

/// Feishu gateway 入站事件元信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayInboundMeta {
    /// 请求 ID。
    pub request_id: String,
    /// Feishu message_id；卡片动作为空。
    pub message_id: Option<String>,
    /// Feishu chat 标识。
    pub chat_id: String,
    /// chat_id 的 SHA-256。
    pub chat_id_sha256: String,
    /// Feishu 会话类型；卡片动作为空。
    pub chat_type: Option<RemoteFeishuChatType>,
    /// 触发本次事件的操作人 open_id；消息事件为发送者，卡片动作为点击者。
    pub operator_open_id: Option<String>,
    /// 可选 reply/thread 上下文。
    pub reply_context: Option<RemoteFeishuReplyContext>,
    /// Axon 入站文本分类；卡片动作事件为空。
    pub message_kind: Option<RemoteFeishuIncomingMessageKind>,
}

/// Feishu gateway 入站队列快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayQueuedEvent {
    /// 入站事件。
    pub event: RemoteFeishuGatewayInboundEvent,
    /// 入站元信息。
    pub meta: RemoteFeishuGatewayInboundMeta,
    /// 入队时间。
    pub enqueued_at_ms: u64,
    /// 已合并的 Feishu 文本消息数。
    pub merged_message_count: usize,
}

/// Feishu gateway drain 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayDrainReport {
    /// 本轮允许进入 Agent turn 的事件。
    pub ready_events: Vec<RemoteFeishuGatewayQueuedEvent>,
    /// 仍在等待的事件数。
    pub pending_events: usize,
    /// 当前活跃 chat turn 锁数量。
    pub active_chat_turns: usize,
    /// 因同 chat 活跃锁被延后的事件数。
    pub deferred_by_chat: usize,
    /// 审计原因。
    pub reasons: Vec<String>,
}

/// Feishu gateway 接入治理链的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayGovernanceInput {
    /// 治理配置。
    pub governance_config: RemoteFeishuGovernanceConfig,
    /// 远控 pairing ID。
    pub pairing_id: String,
    /// 已持久化治理记录。
    pub existing_records: Vec<RemoteFeishuGovernanceRecord>,
}

/// Feishu gateway 接入治理链后的单条结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayGovernanceEventReport {
    /// 入站事件元信息。
    pub meta: RemoteFeishuGatewayInboundMeta,
    /// 治理决策。
    pub decision: RemoteFeishuGovernanceDecision,
    /// 交给本地 Agent 的完整归一化 prompt；仅内存传递，不进入审计 JSON。
    pub agent_prompt: Option<String>,
    /// 被回复消息的可见文本上下文；仅内存传递，不进入审计 JSON。
    pub quoted_message: Option<QuotedMessageContext>,
    /// 交给本地 Agent 的本地附件文件路径；仅内存传递，不进入审计 JSON。
    pub agent_files: Vec<PathBuf>,
}

/// Feishu gateway drain -> governance -> session -> remote transport 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayGovernanceReport {
    /// drain 报告。
    pub drain: RemoteFeishuGatewayDrainReport,
    /// 已进入治理链的事件结果。
    pub event_reports: Vec<RemoteFeishuGatewayGovernanceEventReport>,
}

/// Feishu gateway 单轮 tick 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayTickInput {
    /// 治理链输入。
    pub governance: RemoteFeishuGatewayGovernanceInput,
    /// 治理记录落盘路径。
    pub governance_store_path: PathBuf,
}

/// Feishu gateway 单轮 tick 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayTickReport {
    /// gateway 到治理链报告。
    pub governance: RemoteFeishuGatewayGovernanceReport,
    /// 已追加写入的治理记录数量。
    pub persisted_records: usize,
    /// 当前活跃 chat turn 锁数量。
    pub active_chat_turns: usize,
}

/// Feishu gateway chat turn 释放报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayTurnCompleteReport {
    /// chat_id 的 SHA-256。
    pub chat_id_sha256: String,
    /// 本次是否释放了既有锁。
    pub released: bool,
    /// 释放后的活跃 chat turn 锁数量。
    pub active_chat_turns: usize,
    /// 释放后的 pending inbound 数量。
    pub pending_events: usize,
}

/// Feishu gateway 运行态。
#[derive(Debug, Clone)]
pub struct RemoteFeishuGatewayRuntime {
    state: RemoteFeishuGatewayConnectionState,
    pending: VecDeque<RemoteFeishuGatewayQueuedEvent>,
    active_chat_hashes: HashSet<String>,
    max_pending_depth: usize,
}

impl RemoteFeishuGatewayRuntime {
    /// 创建 Feishu gateway 运行态。
    #[must_use]
    pub fn new(max_pending_depth: usize) -> Self {
        Self {
            state: RemoteFeishuGatewayConnectionState::Disconnected,
            pending: VecDeque::new(),
            active_chat_hashes: HashSet::new(),
            max_pending_depth: max_pending_depth.max(1),
        }
    }

    /// 标记 gateway 进入连接态。
    pub fn connect(&mut self) {
        self.state = RemoteFeishuGatewayConnectionState::Connected;
    }

    /// 当前连接状态。
    #[must_use]
    pub const fn state(&self) -> RemoteFeishuGatewayConnectionState {
        self.state
    }

    /// 当前 pending inbound 数量。
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 当前活跃 chat turn 锁数量。
    #[must_use]
    pub fn active_chat_turn_count(&self) -> usize {
        self.active_chat_hashes.len()
    }

    /// 入队真实 WebSocket 接收到的事件。
    ///
    /// # Errors
    ///
    /// 当 gateway 未连接、正在停止、事件无效或 pending 队列已满时返回错误。
    pub fn enqueue(
        &mut self,
        event: RemoteFeishuGatewayInboundEvent,
        enqueued_at_ms: u64,
    ) -> AxonResult<RemoteFeishuGatewayInboundMeta> {
        if self.state != RemoteFeishuGatewayConnectionState::Connected {
            return Err(AxonError::runtime_failure(
                "Feishu gateway 未处于连接态，无法接收入站事件",
            ));
        }
        if enqueued_at_ms == 0 {
            return Err(AxonError::runtime_failure("Feishu gateway 入站时间无效"));
        }
        let meta = event_meta(&event)?;
        if self.try_merge_text_event(&event, &meta, enqueued_at_ms)? {
            return Ok(meta);
        }
        if self.pending.len() >= self.max_pending_depth {
            return Err(AxonError::runtime_failure(
                "Feishu gateway pending inbound 已达上限",
            ));
        }
        self.pending.push_back(RemoteFeishuGatewayQueuedEvent {
            event,
            meta: meta.clone(),
            enqueued_at_ms,
            merged_message_count: 1,
        });
        Ok(meta)
    }

    /// 按 chat 维度执行入站限流后入队。
    pub fn enqueue_with_rate_limit(
        &mut self,
        event: RemoteFeishuGatewayInboundEvent,
        enqueued_at_ms: u64,
        limiter: &mut crate::RemoteFeishuRateLimiter,
    ) -> AxonResult<RemoteFeishuGatewayInboundMeta> {
        let meta = event_meta(&event)?;
        let decision = limiter.check(&meta.chat_id, enqueued_at_ms)?;
        if !decision.allowed {
            return Err(AxonError::runtime_failure("Feishu gateway 入站触发限流"));
        }
        self.enqueue(event, enqueued_at_ms)
    }

    fn try_merge_text_event(
        &mut self,
        event: &RemoteFeishuGatewayInboundEvent,
        incoming_meta: &RemoteFeishuGatewayInboundMeta,
        enqueued_at_ms: u64,
    ) -> AxonResult<bool> {
        let Some(existing) = self.pending.back_mut() else {
            return Ok(false);
        };
        let (
            RemoteFeishuGatewayInboundEvent::Message(existing_message),
            RemoteFeishuGatewayInboundEvent::Message(incoming_message),
        ) = (&mut existing.event, event)
        else {
            return Ok(false);
        };
        if !can_merge_text_messages(existing_message, incoming_message) {
            return Ok(false);
        }
        let next_count = existing.merged_message_count + 1;
        let next_text = merged_text(&existing_message.text, &incoming_message.text);
        if next_count > REMOTE_FEISHU_GATEWAY_TEXT_BATCH_MAX_MESSAGES
            || next_text.chars().count() > REMOTE_FEISHU_GATEWAY_TEXT_BATCH_MAX_CHARS
        {
            return Ok(false);
        }
        existing_message.text = next_text;
        existing_message
            .media_refs
            .extend(incoming_message.media_refs.iter().cloned());
        existing_message
            .message_id
            .clone_from(&incoming_message.message_id);
        existing_message
            .request_id
            .clone_from(&incoming_message.request_id);
        existing.meta = incoming_meta.clone();
        existing.enqueued_at_ms = enqueued_at_ms;
        existing.merged_message_count = next_count;
        Ok(true)
    }

    /// drain pending inbound，并按 chat 维度只放行一个未活跃事件。
    pub fn drain_ready(&mut self) -> RemoteFeishuGatewayDrainReport {
        let mut ready_events = Vec::new();
        let mut deferred_events = VecDeque::new();
        let mut seen_in_batch = HashSet::new();
        let mut deferred_by_chat = 0usize;
        while let Some(queued) = self.pending.pop_front() {
            let chat_hash = queued.meta.chat_id_sha256.clone();
            if self.active_chat_hashes.contains(&chat_hash) || !seen_in_batch.insert(chat_hash) {
                deferred_by_chat += 1;
                deferred_events.push_back(queued);
                continue;
            }
            self.active_chat_hashes
                .insert(queued.meta.chat_id_sha256.clone());
            ready_events.push(queued);
        }
        self.pending = deferred_events;
        let mut reasons = vec![REMOTE_FEISHU_GATEWAY_REASON_DRAINED.to_owned()];
        if deferred_by_chat > 0 {
            reasons.push(REMOTE_FEISHU_GATEWAY_REASON_DEFERRED_BY_CHAT.to_owned());
        }
        RemoteFeishuGatewayDrainReport {
            ready_events,
            pending_events: self.pending.len(),
            active_chat_turns: self.active_chat_hashes.len(),
            deferred_by_chat,
            reasons,
        }
    }

    /// drain pending inbound 并进入治理链。
    ///
    /// Accepted 事件会继续进入 session 与 remote transport；rejected/duplicate
    /// 事件保留治理决策，供上层持久化审计。
    pub fn drain_into_governance(
        &mut self,
        governance: &RemoteFeishuGovernanceRuntime,
        input: &RemoteFeishuGatewayGovernanceInput,
    ) -> AxonResult<RemoteFeishuGatewayGovernanceReport> {
        if input.pairing_id.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu gateway 治理输入缺少 pairing_id",
            ));
        }
        let drain = self.drain_ready();
        let mut event_reports = Vec::with_capacity(drain.ready_events.len());
        let mut tick_records = input.existing_records.clone();
        for queued in &drain.ready_events {
            let decision = match &queued.event {
                RemoteFeishuGatewayInboundEvent::Message(message) => governance.handle_message(
                    &input.governance_config,
                    message,
                    &input.pairing_id,
                    queued.enqueued_at_ms,
                    &tick_records,
                )?,
                RemoteFeishuGatewayInboundEvent::CardAction(card_action) => governance
                    .handle_card_action(
                        &input.governance_config,
                        card_action,
                        &input.pairing_id,
                        queued.enqueued_at_ms,
                        &tick_records,
                    )?,
            };
            tick_records.push(decision.record.clone());
            event_reports.push(RemoteFeishuGatewayGovernanceEventReport {
                meta: queued.meta.clone(),
                decision,
                agent_prompt: agent_prompt(&queued.event),
                quoted_message: quoted_message(&queued.event),
                agent_files: agent_files(&queued.event),
            });
        }
        Ok(RemoteFeishuGatewayGovernanceReport {
            drain,
            event_reports,
        })
    }

    /// 执行单轮 gateway tick，并把治理决策记录追加到 JSONL。
    ///
    /// 只有终结性决策（rejected / duplicate）在此落盘。`Accepted` 记录的语义是
    /// **已交付**：它必须由调用方在 dispatch 提交成功之后落盘。若在此抢先落盘，
    /// 「治理记录落盘 → 提交失败 → 重投」的重试会把自己的记录判成重复而静默丢弃，
    /// 用户既收不到回复也不会有任何错误上报。
    pub fn tick_and_persist_governance(
        &mut self,
        governance: &RemoteFeishuGovernanceRuntime,
        input: &RemoteFeishuGatewayTickInput,
    ) -> AxonResult<RemoteFeishuGatewayTickReport> {
        let report = self.drain_into_governance(governance, &input.governance)?;
        let mut persisted_records = 0usize;
        for event_report in &report.event_reports {
            if event_report.decision.status == RemoteFeishuGovernanceDecisionStatus::Accepted {
                continue;
            }
            RemoteFeishuGovernanceStoreRuntime::append_record(
                &input.governance_store_path,
                &event_report.decision.record,
            )?;
            persisted_records += 1;
        }
        Ok(RemoteFeishuGatewayTickReport {
            governance: report,
            persisted_records,
            active_chat_turns: self.active_chat_hashes.len(),
        })
    }

    /// 释放一个 chat 的入站 turn 锁。
    pub fn complete_chat_turn(
        &mut self,
        chat_id: &str,
    ) -> AxonResult<RemoteFeishuGatewayTurnCompleteReport> {
        if chat_id.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu gateway chat turn release 缺少 chat_id",
            ));
        }
        let chat_id_sha256 = sha256_hex(chat_id);
        let released = self.active_chat_hashes.remove(&chat_id_sha256);
        Ok(RemoteFeishuGatewayTurnCompleteReport {
            chat_id_sha256,
            released,
            active_chat_turns: self.active_chat_hashes.len(),
            pending_events: self.pending.len(),
        })
    }

    /// 标记 shutdown，可选择保留 pending 供重启 replay。
    pub fn shutdown(&mut self, drop_pending: bool) {
        self.state = RemoteFeishuGatewayConnectionState::ShuttingDown;
        self.active_chat_hashes.clear();
        if drop_pending {
            self.pending.clear();
        }
    }
}

impl Default for RemoteFeishuGatewayRuntime {
    fn default() -> Self {
        Self::new(REMOTE_FEISHU_GATEWAY_DEFAULT_PENDING_DEPTH)
    }
}

fn event_meta(
    event: &RemoteFeishuGatewayInboundEvent,
) -> AxonResult<RemoteFeishuGatewayInboundMeta> {
    let (request_id, message_id, chat_id, reply_context, message_kind, chat_type) = match event {
        RemoteFeishuGatewayInboundEvent::Message(message) => (
            message.request_id.as_str(),
            Some(message.message_id.as_str()),
            message.chat_id.as_str(),
            message.reply_context.clone(),
            Some(message.message_kind),
            Some(message.chat_type),
        ),
        RemoteFeishuGatewayInboundEvent::CardAction(card_action) => (
            card_action.request_id.as_str(),
            card_action.message_id.as_deref(),
            card_action.chat_id.as_str(),
            card_action.reply_context.clone(),
            Some(RemoteFeishuIncomingMessageKind::Text),
            Some(RemoteFeishuChatType::Group),
        ),
    };
    if request_id.trim().is_empty() || chat_id.trim().is_empty() {
        return Err(AxonError::runtime_failure(
            "Feishu gateway 入站事件缺少 request_id 或 chat_id",
        ));
    }
    let operator_open_id = match event {
        RemoteFeishuGatewayInboundEvent::Message(message) => Some(message.sender_open_id.clone()),
        RemoteFeishuGatewayInboundEvent::CardAction(card_action) => {
            Some(card_action.operator_open_id.clone())
        }
    };
    Ok(RemoteFeishuGatewayInboundMeta {
        request_id: request_id.to_owned(),
        message_id: message_id.map(str::to_owned),
        chat_id: chat_id.to_owned(),
        chat_id_sha256: sha256_hex(chat_id),
        operator_open_id,
        reply_context,
        message_kind,
        chat_type,
    })
}

fn agent_prompt(event: &RemoteFeishuGatewayInboundEvent) -> Option<String> {
    match event {
        RemoteFeishuGatewayInboundEvent::Message(message) => {
            Some(agent_visible_message_text(message))
        }
        RemoteFeishuGatewayInboundEvent::CardAction(card_action) => {
            agent_visible_card_action_text(card_action)
        }
    }
}

fn agent_visible_card_action_text(event: &RemoteFeishuCardActionEvent) -> Option<String> {
    if card_action_handled_locally(event) {
        return None;
    }
    let action_label = card_action_label(event);
    let value_preview = bounded_card_action_value(&event.action_value_json);
    Some(format!(
        "用户点击了飞书卡片动作。\n动作标签：{}\n动作值摘要：{}",
        action_label, value_preview
    ))
}

/// 该卡片动作是否已在本地处理完毕，因而**不得**再生成摘要派发一次 agent turn。
///
/// 控制命令（approve/deny/clarify）与问答作答（answer_question）都由各自的入口结算：
/// 前者走内联控制事务，后者走问答结算并把答案作为一条用户消息注入。若仍按普通卡片
/// 动作生成摘要派发，同一次点击会重复触发一轮 turn。
fn card_action_handled_locally(event: &RemoteFeishuCardActionEvent) -> bool {
    card_action_control_command(event).is_some() || card_action_is_question_answer(event)
}

/// 是否为问答作答按钮；与 `remote_feishu_question_card` 的载荷契约保持一致。
fn card_action_is_question_answer(event: &RemoteFeishuCardActionEvent) -> bool {
    matches!(
        card_action_json_field(&event.action_value_json, CARD_ACTION_AXON_ACTION_KEY).as_deref(),
        Some(CARD_ACTION_QUESTION_ANSWER)
    )
}

fn card_action_control_command(event: &RemoteFeishuCardActionEvent) -> Option<&'static str> {
    let candidates = [
        event.action_tag.trim().to_ascii_lowercase(),
        card_action_json_field(&event.action_value_json, CARD_ACTION_AXON_ACTION_KEY)
            .unwrap_or_default()
            .to_ascii_lowercase(),
        card_action_json_field(&event.action_value_json, CARD_ACTION_ACTION_KEY)
            .unwrap_or_default()
            .to_ascii_lowercase(),
        card_action_json_field(&event.action_value_json, CARD_ACTION_COMMAND_KEY)
            .unwrap_or_default()
            .trim_start_matches('/')
            .to_ascii_lowercase(),
    ];
    candidates.iter().find_map(|value| match value.as_str() {
        CARD_ACTION_CONTROL_APPROVE | "approve_once" | "approve_session" | "approve_always" => {
            Some(CARD_ACTION_CONTROL_APPROVE)
        }
        CARD_ACTION_CONTROL_DENY => Some(CARD_ACTION_CONTROL_DENY),
        CARD_ACTION_CONTROL_CLARIFY => Some(CARD_ACTION_CONTROL_CLARIFY),
        _ => None,
    })
}

fn card_action_label(event: &RemoteFeishuCardActionEvent) -> String {
    card_action_json_field(&event.action_value_json, CARD_ACTION_LABEL_KEY)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| event.action_tag.trim().to_owned())
}

fn card_action_json_field(payload: &str, key: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(payload).ok()?;
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn bounded_card_action_value(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "{}".to_owned();
    }
    let preview: String = trimmed
        .chars()
        .take(CARD_ACTION_VALUE_PREVIEW_MAX_CHARS)
        .collect();
    if preview.chars().count() == trimmed.chars().count() {
        preview
    } else {
        format!("{preview}...")
    }
}

fn agent_visible_message_text(message: &RemoteFeishuIncomingMessage) -> String {
    message_text_with_inline_attachments(&message.text)
}

fn quoted_message(event: &RemoteFeishuGatewayInboundEvent) -> Option<QuotedMessageContext> {
    let RemoteFeishuGatewayInboundEvent::Message(message) = event else {
        return None;
    };
    if message.message_kind == RemoteFeishuIncomingMessageKind::Command {
        return None;
    }
    let context = message.reply_context.as_ref()?;
    QuotedMessageContext::new(
        context.reply_to_message_id.as_deref()?,
        context.reply_to_text.as_deref()?,
    )
    .ok()
}

fn message_text_with_inline_attachments(text: &str) -> String {
    let mut inline_blocks = Vec::new();
    for path in text.lines().filter_map(local_path_from_attachment_line) {
        if let Some(block) = inline_text_attachment(&path) {
            inline_blocks.push(block);
        }
    }
    if inline_blocks.is_empty() {
        return text.to_owned();
    }
    format!("{text}\n\n{}", inline_blocks.join("\n\n"))
}

fn inline_text_attachment(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > REMOTE_FEISHU_GATEWAY_TEXT_ATTACHMENT_INLINE_MAX_BYTES {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment");
    Some(format!("[Content of {name}]:\n{content}"))
}

fn agent_files(event: &RemoteFeishuGatewayInboundEvent) -> Vec<PathBuf> {
    let RemoteFeishuGatewayInboundEvent::Message(message) = event else {
        return Vec::new();
    };
    message
        .text
        .lines()
        .filter_map(local_path_from_attachment_line)
        .filter(|path| path.is_file())
        .collect()
}

fn local_path_from_attachment_line(line: &str) -> Option<PathBuf> {
    let trimmed = line.trim();
    (!trimmed.is_empty() && !trimmed.starts_with("[Attachment:") && Path::new(trimmed).is_file())
        .then(|| PathBuf::from(trimmed))
}

fn can_merge_text_messages(
    existing: &RemoteFeishuIncomingMessage,
    incoming: &RemoteFeishuIncomingMessage,
) -> bool {
    existing.message_kind == RemoteFeishuIncomingMessageKind::Text
        && incoming.message_kind == RemoteFeishuIncomingMessageKind::Text
        && existing.chat_id == incoming.chat_id
        && existing.reply_context == incoming.reply_context
}

fn merged_text(existing: &str, incoming: &str) -> String {
    match (existing.is_empty(), incoming.is_empty()) {
        (true, _) => incoming.to_owned(),
        (_, true) => existing.to_owned(),
        _ => format!("{existing}\n{incoming}"),
    }
}
