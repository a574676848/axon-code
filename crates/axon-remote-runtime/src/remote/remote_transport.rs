//! 远控传输入口计划。
//!
//! 该模块冻结 HTTP/WS/Feishu/mobile/Web adapter 进入本地协议 runtime 前的边界：
//! 只接受结构化 envelope，不读取 raw body、不启动监听器、不发送网络响应。

use axon_domain::{AxonError, AxonResult, RemoteCapabilityPolicy, SessionId};
use serde::{Deserialize, Serialize};

use crate::{
    RemoteProtocolPayload, RemoteProtocolRequest, RemoteProtocolResponse,
    RemoteProtocolResponsePayload, RemoteProtocolSimulatorRuntime,
};

/// 远控传输 connection id 最大字符数。
pub const REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS: usize = 128;
/// 远控传输 pairing id 最大字符数。
pub const REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS: usize = 128;
/// 远控传输入口已校验原因。
pub const REMOTE_TRANSPORT_REASON_INGRESS_VALIDATED: &str = "remote_transport:ingress_validated";
/// 远控协议已处理原因。
pub const REMOTE_TRANSPORT_REASON_PROTOCOL_HANDLED: &str = "remote_transport:protocol_handled";
/// 远控 adapter 响应计划已生成原因。
pub const REMOTE_TRANSPORT_REASON_ADAPTER_DISPATCH_PLANNED: &str =
    "remote_transport:adapter_dispatch_planned";
/// 远控网络发送关闭原因。
pub const REMOTE_TRANSPORT_REASON_NETWORK_CLOSED: &str = "remote_transport:network_closed";
/// 远控 raw body 转发关闭原因。
pub const REMOTE_TRANSPORT_REASON_RAW_BODY_CLOSED: &str = "remote_transport:raw_body_closed";

/// 远控传输类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteTransportKind {
    /// HTTP 请求/响应。
    Http,
    /// WebSocket 消息。
    WebSocket,
    /// 飞书 adapter。
    Feishu,
    /// 移动端 adapter。
    Mobile,
    /// Web 控制台 adapter。
    Web,
}

/// 远控传输入口 envelope。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTransportEnvelope {
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 目标会话。
    pub session_id: SessionId,
    /// 接收时间。
    pub received_at_ms: u64,
    /// 是否请求转发 raw body。
    pub raw_body_forward_requested: bool,
    /// 已解析的本地协议请求。
    pub protocol_request: RemoteProtocolRequest,
}

/// 远控传输处理报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTransportDispatchReport {
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 目标会话。
    pub session_id: SessionId,
    /// 远控 client id。
    pub client_id: String,
    /// 请求 ID。
    pub request_id: String,
    /// 本地协议响应。
    pub protocol_response: RemoteProtocolResponse,
    /// adapter-neutral 响应计划。
    pub adapter_dispatch: RemoteAdapterDispatchPlan,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控 adapter 响应计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAdapterDispatchPlan {
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 目标会话。
    pub session_id: SessionId,
    /// 响应载荷类别。
    pub payload_kind: RemoteAdapterPayloadKind,
    /// 是否只要求本机 adapter 回包。
    pub response_only: bool,
    /// 是否允许 runtime 主动发送网络消息。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw body。
    pub raw_body_forward_allowed: bool,
}

/// 远控 adapter 响应载荷类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAdapterPayloadKind {
    /// bounded summary。
    Summary,
    /// 后续指令规划。
    MessagePlan,
    /// 单条通知或审批卡片投递报告。
    DeliveryReport,
    /// 通知订阅报告。
    NotificationSubscription,
    /// 通知批量报告。
    NotificationBatch,
}

/// 远控传输入口 runtime。
#[derive(Debug, Clone)]
pub struct RemoteTransportIngressRuntime {
    protocol: RemoteProtocolSimulatorRuntime,
}

impl RemoteTransportIngressRuntime {
    /// 创建远控传输入口 runtime。
    #[must_use]
    pub fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self {
            protocol: RemoteProtocolSimulatorRuntime::new(policy),
        }
    }

    /// 处理远控传输入口 envelope。
    ///
    /// # Errors
    ///
    /// 当 envelope 元数据无效、请求转发 raw body、client/request/session 绑定不一致，
    /// 或内部协议 runtime 拒绝请求时返回错误。
    pub fn handle(
        &self,
        envelope: &RemoteTransportEnvelope,
    ) -> AxonResult<RemoteTransportDispatchReport> {
        validate_envelope(envelope)?;
        let response = self.protocol.handle(&envelope.protocol_request)?;
        if response.outbound_network_allowed {
            return Err(AxonError::runtime_failure("远控协议响应越过传输安全边界"));
        }

        let adapter_dispatch = adapter_dispatch(envelope.transport, envelope.session_id, &response);
        Ok(RemoteTransportDispatchReport {
            transport: envelope.transport,
            connection_id: envelope.connection_id.clone(),
            pairing_id: envelope.pairing_id.clone(),
            session_id: envelope.session_id,
            client_id: response.client_id.clone(),
            request_id: response.request_id.clone(),
            protocol_response: response,
            adapter_dispatch,
            reasons: vec![
                REMOTE_TRANSPORT_REASON_INGRESS_VALIDATED.to_owned(),
                REMOTE_TRANSPORT_REASON_PROTOCOL_HANDLED.to_owned(),
                REMOTE_TRANSPORT_REASON_ADAPTER_DISPATCH_PLANNED.to_owned(),
                REMOTE_TRANSPORT_REASON_NETWORK_CLOSED.to_owned(),
                REMOTE_TRANSPORT_REASON_RAW_BODY_CLOSED.to_owned(),
            ],
        })
    }
}

fn validate_envelope(envelope: &RemoteTransportEnvelope) -> AxonResult<()> {
    validate_id(
        &envelope.connection_id,
        REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS,
    )?;
    validate_id(&envelope.pairing_id, REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS)?;
    if envelope.received_at_ms == 0 {
        return Err(AxonError::runtime_failure("远控接收时间无效"));
    }
    if envelope.raw_body_forward_requested {
        return Err(AxonError::runtime_failure("远控 raw body 转发未开放"));
    }
    if envelope.protocol_request.client_id != envelope.connection_id {
        return Err(AxonError::runtime_failure("远控 client id 与连接不匹配"));
    }
    if payload_session_id(&envelope.protocol_request.payload) != envelope.session_id {
        return Err(AxonError::runtime_failure(
            "远控请求 session 与 envelope 不匹配",
        ));
    }
    Ok(())
}

fn validate_id(value: &str, limit: usize) -> AxonResult<()> {
    let actual = value.chars().count();
    if value.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if actual > limit {
        return Err(AxonError::InputTooLong { limit, actual });
    }
    Ok(())
}

fn payload_session_id(payload: &RemoteProtocolPayload) -> SessionId {
    match payload {
        RemoteProtocolPayload::ReadSummary { session_id } => *session_id,
        RemoteProtocolPayload::DeliverMessage { message } => message.session_id,
        RemoteProtocolPayload::SendNotification { notification } => notification.session_id,
        RemoteProtocolPayload::SubscribeNotifications { subscription }
        | RemoteProtocolPayload::DeliverNotifications { subscription, .. } => {
            subscription.session_id
        }
    }
}

fn adapter_dispatch(
    transport: RemoteTransportKind,
    session_id: SessionId,
    response: &RemoteProtocolResponse,
) -> RemoteAdapterDispatchPlan {
    RemoteAdapterDispatchPlan {
        transport,
        session_id,
        payload_kind: payload_kind(&response.payload),
        response_only: true,
        network_send_allowed: false,
        raw_body_forward_allowed: false,
    }
}

fn payload_kind(payload: &RemoteProtocolResponsePayload) -> RemoteAdapterPayloadKind {
    match payload {
        RemoteProtocolResponsePayload::Summary(_) => RemoteAdapterPayloadKind::Summary,
        RemoteProtocolResponsePayload::MessagePlan(_) => RemoteAdapterPayloadKind::MessagePlan,
        RemoteProtocolResponsePayload::DeliveryReport(_) => {
            RemoteAdapterPayloadKind::DeliveryReport
        }
        RemoteProtocolResponsePayload::NotificationSubscription(_) => {
            RemoteAdapterPayloadKind::NotificationSubscription
        }
        RemoteProtocolResponsePayload::NotificationBatch(_) => {
            RemoteAdapterPayloadKind::NotificationBatch
        }
    }
}

#[cfg(test)]
mod tests {
    use axon_domain::{AxonError, RemoteCapabilityPolicy, SessionId};

    use super::*;
    use crate::REMOTE_PROTOCOL_VERSION;

    #[test]
    fn remote_transport_ingress_binds_envelope_to_protocol_without_network() {
        let report = runtime().handle(&summary_envelope()).unwrap();

        assert_eq!(report.transport, RemoteTransportKind::WebSocket);
        assert_eq!(report.connection_id, "mobile-local-sim");
        assert_eq!(report.pairing_id, "pairing-local");
        assert_eq!(report.session_id, SessionId(11));
        assert_eq!(
            report.adapter_dispatch.payload_kind,
            RemoteAdapterPayloadKind::Summary
        );
        assert!(report.adapter_dispatch.response_only);
        assert!(!report.adapter_dispatch.network_send_allowed);
        assert!(!report.adapter_dispatch.raw_body_forward_allowed);
        assert!(!report.protocol_response.outbound_network_allowed);
        assert!(
            report
                .reasons
                .contains(&REMOTE_TRANSPORT_REASON_ADAPTER_DISPATCH_PLANNED.to_owned())
        );
        assert!(
            report
                .reasons
                .contains(&REMOTE_TRANSPORT_REASON_NETWORK_CLOSED.to_owned())
        );
        assert!(
            report
                .reasons
                .contains(&REMOTE_TRANSPORT_REASON_RAW_BODY_CLOSED.to_owned())
        );
    }

    #[test]
    fn remote_transport_ingress_rejects_raw_body_and_session_mismatch() {
        let mut raw_body = summary_envelope();
        raw_body.raw_body_forward_requested = true;
        assert_eq!(
            runtime().handle(&raw_body),
            Err(AxonError::runtime_failure("远控 raw body 转发未开放"))
        );

        let mut session_mismatch = summary_envelope();
        session_mismatch.session_id = SessionId(12);
        assert_eq!(
            runtime().handle(&session_mismatch),
            Err(AxonError::runtime_failure(
                "远控请求 session 与 envelope 不匹配"
            ))
        );
    }

    #[test]
    fn remote_transport_ingress_rejects_connection_client_mismatch() {
        let mut envelope = summary_envelope();
        envelope.connection_id = "another-client".to_owned();

        assert_eq!(
            runtime().handle(&envelope),
            Err(AxonError::runtime_failure("远控 client id 与连接不匹配"))
        );
    }

    fn runtime() -> RemoteTransportIngressRuntime {
        RemoteTransportIngressRuntime::new(RemoteCapabilityPolicy::default_sidecar())
    }

    fn summary_envelope() -> RemoteTransportEnvelope {
        RemoteTransportEnvelope {
            transport: RemoteTransportKind::WebSocket,
            connection_id: "mobile-local-sim".to_owned(),
            pairing_id: "pairing-local".to_owned(),
            session_id: SessionId(11),
            received_at_ms: 1_000,
            raw_body_forward_requested: false,
            protocol_request: RemoteProtocolRequest {
                protocol_version: REMOTE_PROTOCOL_VERSION.to_owned(),
                client_id: "mobile-local-sim".to_owned(),
                request_id: "request-summary".to_owned(),
                payload: RemoteProtocolPayload::ReadSummary {
                    session_id: SessionId(11),
                },
            },
        }
    }
}
