//! 远控通知 adapter envelope 投影。
//!
//! 该模块把本地 dispatch batch 投影为各传输 adapter 可消费的有界 envelope；不发送网络。

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    RemoteNotificationAdapterDispatchBatch, RemoteNotificationAdapterDispatchItem,
    RemoteNotificationKind, RemoteTransportKind,
};

/// 远控通知 adapter envelope 标题最大字符数。
pub const REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_TITLE_MAX_CHARS: usize = 80;
/// 远控通知 adapter envelope 摘要最大字符数。
pub const REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_SUMMARY_MAX_CHARS: usize = 240;
/// 远控通知 adapter envelope 已投影原因。
pub const REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_PROJECTED: &str =
    "remote_notification_adapter_envelope:projected";
/// 远控通知 adapter envelope 空批次原因。
pub const REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_EMPTY_BATCH: &str =
    "remote_notification_adapter_envelope:empty_batch";
/// 远控通知 adapter envelope 网络关闭原因。
pub const REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_NETWORK_CLOSED: &str =
    "remote_notification_adapter_envelope:network_closed";
/// 远控通知 adapter envelope raw payload 关闭原因。
pub const REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_RAW_PAYLOAD_CLOSED: &str =
    "remote_notification_adapter_envelope:raw_payload_closed";
/// 远控通知 adapter envelope 目标表面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteNotificationAdapterEnvelopeSurface {
    /// HTTP 轮询响应。
    HttpPoll,
    /// WebSocket server event。
    WebSocketEvent,
    /// 飞书卡片。
    FeishuCard,
    /// 移动端 push。
    MobilePush,
    /// Web 控制台 toast。
    WebToast,
}

/// 远控通知 adapter envelope。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterEnvelope {
    /// dispatch id。
    pub dispatch_id: String,
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 目标表面。
    pub surface: RemoteNotificationAdapterEnvelopeSurface,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 标题。
    pub title: String,
    /// 有界摘要。
    pub summary: String,
    /// 投递数量。
    pub delivery_count: usize,
    /// 投递类别。
    pub delivery_kinds: Vec<RemoteNotificationKind>,
    /// 尝试次数。
    pub attempt_count: u32,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许 runtime 主动发送网络。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
}

/// 远控通知 adapter envelope 投影报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterEnvelopeReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 本批 envelope。
    pub envelopes: Vec<RemoteNotificationAdapterEnvelope>,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许 runtime 主动发送网络。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控通知 adapter envelope runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationAdapterEnvelopeRuntime;

impl RemoteNotificationAdapterEnvelopeRuntime {
    /// 从 dispatch batch 生成传输 adapter envelope。
    ///
    /// # Errors
    ///
    /// 当 batch 安全边界、item 元数据或生成的标题/摘要无效时返回错误。
    pub fn project_batch(
        batch: &RemoteNotificationAdapterDispatchBatch,
    ) -> AxonResult<RemoteNotificationAdapterEnvelopeReport> {
        validate_batch(batch)?;
        let envelopes = batch
            .items
            .iter()
            .map(|item| envelope(batch, item))
            .collect::<AxonResult<Vec<_>>>()?;
        Ok(RemoteNotificationAdapterEnvelopeReport {
            session_id: batch.session_id,
            transport: batch.transport,
            envelopes,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: report_reasons(batch.items.is_empty()),
        })
    }
}

fn validate_batch(batch: &RemoteNotificationAdapterDispatchBatch) -> AxonResult<()> {
    if batch.session_id.0 == 0
        || !batch.local_only
        || batch.network_send_allowed
        || batch.raw_payload_forward_allowed
    {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter envelope batch 安全边界无效",
        ));
    }
    Ok(())
}

fn envelope(
    batch: &RemoteNotificationAdapterDispatchBatch,
    item: &RemoteNotificationAdapterDispatchItem,
) -> AxonResult<RemoteNotificationAdapterEnvelope> {
    validate_item(item)?;
    let title = format!("Axon {}", surface_label(batch.transport));
    let summary = format!(
        "有 {} 条远控通知：{}",
        item.delivery_count,
        kind_summary(&item.delivery_kinds)
    );
    validate_bounded(
        &title,
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_TITLE_MAX_CHARS,
        "远控通知 adapter envelope 标题无效",
    )?;
    validate_bounded(
        &summary,
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_SUMMARY_MAX_CHARS,
        "远控通知 adapter envelope 摘要无效",
    )?;
    Ok(RemoteNotificationAdapterEnvelope {
        dispatch_id: item.dispatch_id.clone(),
        session_id: batch.session_id,
        transport: batch.transport,
        surface: surface(batch.transport),
        subscriber_id: item.subscriber_id.clone(),
        connection_id: item.connection_id.clone(),
        pairing_id: item.pairing_id.clone(),
        title,
        summary,
        delivery_count: item.delivery_count,
        delivery_kinds: item.delivery_kinds.clone(),
        attempt_count: item.attempt_count,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
    })
}

fn validate_item(item: &RemoteNotificationAdapterDispatchItem) -> AxonResult<()> {
    if item.dispatch_id.trim().is_empty()
        || item.subscriber_id.trim().is_empty()
        || item.connection_id.trim().is_empty()
        || item.pairing_id.trim().is_empty()
        || item.delivery_count == 0
        || item.delivery_kinds.len() != item.delivery_count
    {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter envelope item 无效",
        ));
    }
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

fn surface(transport: RemoteTransportKind) -> RemoteNotificationAdapterEnvelopeSurface {
    match transport {
        RemoteTransportKind::Http => RemoteNotificationAdapterEnvelopeSurface::HttpPoll,
        RemoteTransportKind::WebSocket => RemoteNotificationAdapterEnvelopeSurface::WebSocketEvent,
        RemoteTransportKind::Feishu => RemoteNotificationAdapterEnvelopeSurface::FeishuCard,
        RemoteTransportKind::Mobile => RemoteNotificationAdapterEnvelopeSurface::MobilePush,
        RemoteTransportKind::Web => RemoteNotificationAdapterEnvelopeSurface::WebToast,
    }
}

fn surface_label(transport: RemoteTransportKind) -> &'static str {
    match transport {
        RemoteTransportKind::Http => "HTTP 远控通知",
        RemoteTransportKind::WebSocket => "WebSocket 远控通知",
        RemoteTransportKind::Feishu => "飞书远控通知",
        RemoteTransportKind::Mobile => "移动端远控通知",
        RemoteTransportKind::Web => "Web 控制台远控通知",
    }
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

fn report_reasons(empty: bool) -> Vec<String> {
    let mut reasons = vec![
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_PROJECTED.to_owned(),
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_NETWORK_CLOSED.to_owned(),
        REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
    ];
    if empty {
        reasons.push(REMOTE_NOTIFICATION_ADAPTER_ENVELOPE_REASON_EMPTY_BATCH.to_owned());
    }
    reasons
}

#[cfg(test)]
mod tests {
    use axon_domain::SessionId;

    use crate::{
        RemoteNotificationAdapterDispatchBatch, RemoteNotificationAdapterDispatchItem,
        RemoteNotificationKind, RemoteTransportKind,
    };

    use super::*;

    #[test]
    fn adapter_envelope_projects_websocket_event_without_network() {
        let report = RemoteNotificationAdapterEnvelopeRuntime::project_batch(&batch())
            .expect("project envelope");

        assert_eq!(report.envelopes.len(), 1);
        assert_eq!(
            report.envelopes[0].surface,
            RemoteNotificationAdapterEnvelopeSurface::WebSocketEvent
        );
        assert!(report.envelopes[0].summary.contains("任务开始"));
        assert!(report.local_only);
        assert!(!report.network_send_allowed);
        assert!(!report.raw_payload_forward_allowed);
    }

    #[test]
    fn adapter_envelope_rejects_unsafe_batch() {
        let mut batch = batch();
        batch.network_send_allowed = true;

        assert!(RemoteNotificationAdapterEnvelopeRuntime::project_batch(&batch).is_err());
    }

    fn batch() -> RemoteNotificationAdapterDispatchBatch {
        RemoteNotificationAdapterDispatchBatch {
            session_id: SessionId(61),
            transport: RemoteTransportKind::WebSocket,
            items: vec![RemoteNotificationAdapterDispatchItem {
                dispatch_id: "dispatch-envelope".to_owned(),
                subscriber_id: "web-client".to_owned(),
                connection_id: "ws-conn".to_owned(),
                pairing_id: "pairing-local".to_owned(),
                delivery_count: 1,
                delivery_kinds: vec![RemoteNotificationKind::TaskStarted],
                attempt_count: 0,
            }],
            ack_records: Vec::new(),
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: Vec::new(),
        }
    }
}
