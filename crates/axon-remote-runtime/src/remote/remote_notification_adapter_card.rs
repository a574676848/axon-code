//! 远控通知平台无关卡片投影。
//!
//! 该模块只负责把 adapter envelope 归一化为平台无关的通知卡片模型，
//! 供 Feishu、Mobile、Web 等不同外层 adapter 渲染。

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_SUMMARY_MAX_CHARS,
    REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_TITLE_MAX_CHARS, RemoteNotificationAdapterEnvelope,
    RemoteNotificationAdapterEnvelopeSurface, RemoteNotificationKind, RemoteTransportKind,
};

/// 通知卡片标题最大字符数。
pub const REMOTE_NOTIFICATION_CARD_TITLE_MAX_CHARS: usize = 60;
/// 通知卡片正文最大字符数。
pub const REMOTE_NOTIFICATION_CARD_BODY_MAX_CHARS: usize = 240;
/// 通知卡片类别摘要最大字符数。
pub const REMOTE_NOTIFICATION_CARD_KIND_SUMMARY_MAX_CHARS: usize = 160;

/// 平台无关的远控通知卡片风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteNotificationCardTone {
    /// 普通信息。
    Info,
    /// 需要关注。
    Warning,
    /// 失败或阻断。
    Danger,
}

/// 平台无关的远控通知卡片模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationCard {
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 目标表面。
    pub surface: RemoteNotificationAdapterEnvelopeSurface,
    /// 标题。
    pub title: String,
    /// 正文。
    pub body: String,
    /// 投递类别摘要。
    pub kind_summary: String,
    /// 投递数量。
    pub delivery_count: usize,
    /// 卡片色调。
    pub tone: RemoteNotificationCardTone,
    /// 是否仅本地消费。
    pub local_only: bool,
    /// 是否允许网络发送。
    pub network_send_allowed: bool,
    /// 是否允许 raw payload。
    pub raw_payload_forward_allowed: bool,
}

/// 平台无关通知卡片 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationCardRuntime;

impl RemoteNotificationCardRuntime {
    /// 从 adapter envelope 投影为平台无关通知卡片。
    ///
    /// # Errors
    ///
    /// 当 envelope 安全边界无效或生成文本超限时返回错误。
    pub fn project(
        envelope: &RemoteNotificationAdapterEnvelope,
    ) -> AxonResult<RemoteNotificationCard> {
        validate_card_envelope(envelope)?;
        Ok(RemoteNotificationCard {
            session_id: envelope.session_id,
            transport: envelope.transport,
            surface: envelope.surface,
            title: clamp_chars(&envelope.title, REMOTE_NOTIFICATION_CARD_TITLE_MAX_CHARS),
            body: clamp_chars(&envelope.summary, REMOTE_NOTIFICATION_CARD_BODY_MAX_CHARS),
            kind_summary: clamp_chars(
                &kind_summary(&envelope.delivery_kinds),
                REMOTE_NOTIFICATION_CARD_KIND_SUMMARY_MAX_CHARS,
            ),
            delivery_count: envelope.delivery_count,
            tone: card_tone(envelope),
            local_only: envelope.local_only,
            network_send_allowed: envelope.network_send_allowed,
            raw_payload_forward_allowed: envelope.raw_payload_forward_allowed,
        })
    }
}

pub(crate) fn validate_card_envelope(
    envelope: &RemoteNotificationAdapterEnvelope,
) -> AxonResult<()> {
    if !envelope.local_only || envelope.network_send_allowed || envelope.raw_payload_forward_allowed
    {
        return Err(AxonError::runtime_failure(
            "远控通知卡片 envelope 安全边界无效",
        ));
    }
    validate_bounded(
        &envelope.title,
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_TITLE_MAX_CHARS,
        "远控通知卡片标题无效",
    )?;
    validate_bounded(
        &envelope.summary,
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_SUMMARY_MAX_CHARS,
        "远控通知卡片摘要无效",
    )?;
    Ok(())
}

pub(crate) fn validate_bounded(
    value: &str,
    max_chars: usize,
    message: &'static str,
) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

pub(crate) fn clamp_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn kind_summary(kinds: &[RemoteNotificationKind]) -> String {
    kinds
        .iter()
        .copied()
        .map(kind_label)
        .collect::<Vec<_>>()
        .join(",")
}

fn kind_label(kind: RemoteNotificationKind) -> &'static str {
    match kind {
        RemoteNotificationKind::TaskStarted => "任务开始",
        RemoteNotificationKind::TestCompleted => "测试完成",
        RemoteNotificationKind::TaskFailed => "任务失败",
        RemoteNotificationKind::InputRequired => "需要输入",
        RemoteNotificationKind::ThreadCompleted => "线程完成",
        RemoteNotificationKind::SafetyBlocked => "安全阻断",
    }
}

fn card_tone(envelope: &RemoteNotificationAdapterEnvelope) -> RemoteNotificationCardTone {
    if envelope.delivery_kinds.iter().any(|kind| {
        matches!(
            kind,
            RemoteNotificationKind::TaskFailed | RemoteNotificationKind::SafetyBlocked
        )
    }) {
        RemoteNotificationCardTone::Danger
    } else if envelope
        .delivery_kinds
        .iter()
        .any(|kind| matches!(kind, RemoteNotificationKind::InputRequired))
    {
        RemoteNotificationCardTone::Warning
    } else {
        RemoteNotificationCardTone::Info
    }
}
