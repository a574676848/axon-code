//! Feishu WebSocket 远控会话 runtime。
//!
//! 该模块把 Feishu WebSocket 入站归一化与 Axon 本地远控传输主链串起来，
//! 形成可测试的最小闭环：入站事件 -> RemoteProtocolRequest -> RemoteTransportIngressRuntime。

use axon_domain::{AxonResult, RemoteCapabilityPolicy};

use crate::{
    RemoteFeishuCardActionEvent, RemoteFeishuIncomingMessage, RemoteFeishuWebSocketIngressRuntime,
    RemoteTransportDispatchReport, RemoteTransportEnvelope, RemoteTransportIngressRuntime,
    RemoteTransportKind,
};

/// Feishu WebSocket 会话 runtime。
#[derive(Debug, Clone)]
pub struct RemoteFeishuWebSocketSessionRuntime {
    transport: RemoteTransportIngressRuntime,
}

impl RemoteFeishuWebSocketSessionRuntime {
    /// 创建 Feishu WebSocket 会话 runtime。
    #[must_use]
    pub fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self {
            transport: RemoteTransportIngressRuntime::new(policy),
        }
    }

    /// 处理文本消息事件。
    ///
    /// # Errors
    ///
    /// 当入站事件或远控传输 envelope 无效时返回错误。
    pub fn handle_message(
        &self,
        event: &RemoteFeishuIncomingMessage,
        pairing_id: &str,
        received_at_ms: u64,
    ) -> AxonResult<RemoteTransportDispatchReport> {
        let protocol_request = RemoteFeishuWebSocketIngressRuntime::normalize_message(event)?;
        self.transport.handle(&RemoteTransportEnvelope {
            transport: RemoteTransportKind::Feishu,
            connection_id: event.client_id.clone(),
            pairing_id: pairing_id.to_owned(),
            session_id: event.session_id,
            received_at_ms,
            raw_body_forward_requested: false,
            protocol_request,
        })
    }

    /// 处理卡片动作事件。
    ///
    /// # Errors
    ///
    /// 当卡片动作、传输 envelope 或本地协议校验失败时返回错误。
    pub fn handle_card_action(
        &self,
        event: &RemoteFeishuCardActionEvent,
        pairing_id: &str,
        received_at_ms: u64,
    ) -> AxonResult<RemoteTransportDispatchReport> {
        let protocol_request = RemoteFeishuWebSocketIngressRuntime::normalize_card_action(event)?;
        self.transport.handle(&RemoteTransportEnvelope {
            transport: RemoteTransportKind::Feishu,
            connection_id: event.client_id.clone(),
            pairing_id: pairing_id.to_owned(),
            session_id: event.session_id,
            received_at_ms,
            raw_body_forward_requested: false,
            protocol_request,
        })
    }
}

#[cfg(test)]
mod tests {
    use axon_domain::{RemoteCapabilityPolicy, SessionId};

    use super::*;

    #[test]
    fn feishu_websocket_session_handles_text_message_into_message_plan() {
        let runtime =
            RemoteFeishuWebSocketSessionRuntime::new(RemoteCapabilityPolicy::default_sidecar());
        let report = runtime
            .handle_message(
                &RemoteFeishuIncomingMessage {
                    session_id: SessionId(21),
                    client_id: "feishu-client".to_owned(),
                    request_id: "req-msg-1".to_owned(),
                    message_id: "om_msg_21".to_owned(),
                    chat_id: "oc_xxx".to_owned(),
                    chat_type: crate::RemoteFeishuChatType::Group,
                    sender_open_id: "ou_xxx".to_owned(),
                    mentions: vec!["ou_bot".to_owned()],
                    text: "请汇总当前线程状态".to_owned(),
                    media_refs: Vec::new(),
                    message_kind: crate::RemoteFeishuIncomingMessageKind::Text,
                    reply_context: None,
                    session_busy: false,
                },
                "pairing-feishu",
                1_000,
            )
            .expect("handle message");

        assert_eq!(report.transport, RemoteTransportKind::Feishu);
        assert_eq!(report.request_id, "req-msg-1");
        assert!(!report.adapter_dispatch.network_send_allowed);
    }

    #[test]
    fn feishu_websocket_session_handles_card_action_into_message_plan() {
        let runtime =
            RemoteFeishuWebSocketSessionRuntime::new(RemoteCapabilityPolicy::default_sidecar());
        let report = runtime
            .handle_card_action(
                &RemoteFeishuCardActionEvent {
                    session_id: SessionId(22),
                    client_id: "feishu-client".to_owned(),
                    request_id: "token-22".to_owned(),
                    message_id: Some("om_card_22".to_owned()),
                    chat_id: "oc_card_22".to_owned(),
                    operator_open_id: "ou_operator".to_owned(),
                    action_tag: "button".to_owned(),
                    action_value_json: r#"{"axon_action":"clarify"}"#.to_owned(),
                    reply_context: None,
                    session_busy: true,
                },
                "pairing-feishu",
                2_000,
            )
            .expect("handle card action");

        assert_eq!(report.transport, RemoteTransportKind::Feishu);
        assert_eq!(report.request_id, "token-22");
        match &report.protocol_response.payload {
            crate::RemoteProtocolResponsePayload::MessagePlan(plan) => {
                assert_eq!(plan.command, None);
                assert_eq!(plan.command_argument, None);
                assert_eq!(
                    plan.action,
                    crate::RemoteControlDispatchAction::QueueUserMessage
                );
                assert!(plan.bounded_preview.starts_with("/card button "));
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_session_rejects_empty_text_message() {
        let runtime =
            RemoteFeishuWebSocketSessionRuntime::new(RemoteCapabilityPolicy::default_sidecar());
        let result = runtime.handle_message(
            &RemoteFeishuIncomingMessage {
                session_id: SessionId(23),
                client_id: "feishu-client".to_owned(),
                request_id: "req-empty".to_owned(),
                message_id: "om_empty".to_owned(),
                chat_id: "oc_empty".to_owned(),
                chat_type: crate::RemoteFeishuChatType::Group,
                sender_open_id: "ou_xxx".to_owned(),
                mentions: vec!["ou_bot".to_owned()],
                text: "   ".to_owned(),
                media_refs: Vec::new(),
                message_kind: crate::RemoteFeishuIncomingMessageKind::Text,
                reply_context: None,
                session_busy: false,
            },
            "pairing-feishu",
            3_000,
        );

        assert!(result.is_err());
    }
}
