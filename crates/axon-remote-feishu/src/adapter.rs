//! Feishu 与统一 Interaction/事件投递的传输适配器。

use axon_crypto::sha256_hex;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord,
    InteractionSource, ReplyRoute, ScopeRef,
};
use axon_session::SessionEventRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const FEISHU_INTERACTION_ID_PREFIX: &str = "interaction-feishu-";
const FEISHU_INTERACTION_CHANNEL: &str = "feishu";

/// 将治理层已接受的普通 Agent 输入投影为统一 durable Interaction。
///
/// 控制卡片与被治理拒绝的消息没有可提交的自然语言 Interaction。
pub fn interaction_from_gateway_report(
    report: &crate::RemoteFeishuGatewayGovernanceEventReport,
    workspace_id: &str,
    received_at_ms: u64,
) -> Result<Option<InteractionRecord>, String> {
    if report.decision.status != crate::RemoteFeishuGovernanceDecisionStatus::Accepted {
        return Ok(None);
    }
    let inline_control = report
        .decision
        .dispatch_report
        .as_ref()
        .and_then(|dispatch| match &dispatch.protocol_response.payload {
            axon_remote_runtime::RemoteProtocolResponsePayload::MessagePlan(plan) => Some(plan),
            _ => None,
        })
        .is_some_and(|plan| {
            plan.action == axon_remote_runtime::RemoteControlDispatchAction::InlineControl
        });
    if inline_control {
        return Ok(None);
    }
    let Some(prompt) = report
        .agent_prompt
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if received_at_ms == 0 {
        return Err("Feishu Interaction 接收时间无效".to_owned());
    }
    let source_message_id = report
        .meta
        .message_id
        .as_deref()
        .unwrap_or(&report.meta.request_id);
    let source_key = format!("{}:{}", report.meta.chat_id, source_message_id);
    let interaction_id = InteractionId::new(format!(
        "{FEISHU_INTERACTION_ID_PREFIX}{}",
        sha256_hex(&source_key)
    ))
    .map_err(|error| error.to_string())?;
    let source = InteractionSource::new(FEISHU_INTERACTION_CHANNEL, source_key)
        .map_err(|error| error.to_string())?;
    let scope = ScopeRef::workspace(workspace_id).map_err(|error| error.to_string())?;
    let (audience_kind, disclosure) = match report.meta.chat_type {
        Some(crate::RemoteFeishuChatType::P2p) => {
            (AudienceKind::Private, DisclosurePolicy::PrivateOnly)
        }
        Some(crate::RemoteFeishuChatType::Group) => {
            (AudienceKind::Group, DisclosurePolicy::Workspace)
        }
        Some(crate::RemoteFeishuChatType::Unknown) | None => {
            return Err("Feishu accepted message 缺少有效 chat_type".to_owned());
        }
    };
    let reply_route = ReplyRoute::new(FEISHU_INTERACTION_CHANNEL, &report.meta.chat_id)
        .map_err(|error| error.to_string())?;
    InteractionRecord::new(
        interaction_id,
        source,
        scope,
        AudienceContext::new(audience_kind, disclosure),
        reply_route,
        prompt,
        received_at_ms,
    )
    .map(|mut interaction| {
        // 入站阶段下载到本地的附件随 Interaction 一起 durable，Provider turn 据此把
        // 图片作为 vision 输入、文件作为可读附件，而不是只留文本里的路径。
        interaction.attachments = report
            .agent_files
            .iter()
            .filter(|path| path.is_absolute())
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        interaction
    })
    .map(Some)
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_text_maps_private_and_group_audiences() {
        let p2p = interaction_report(
            crate::RemoteFeishuChatType::P2p,
            crate::RemoteFeishuIncomingMessageKind::Text,
            crate::RemoteFeishuGovernanceDecisionStatus::Accepted,
            Some("你好"),
        );
        let group = interaction_report(
            crate::RemoteFeishuChatType::Group,
            crate::RemoteFeishuIncomingMessageKind::Text,
            crate::RemoteFeishuGovernanceDecisionStatus::Accepted,
            Some("群消息"),
        );

        let p2p_interaction = interaction_from_gateway_report(&p2p, "workspace-p2p", 42)
            .expect("单聊应可投影")
            .expect("单聊应生成 Interaction");
        let group_interaction = interaction_from_gateway_report(&group, "workspace-group", 42)
            .expect("群聊应可投影")
            .expect("群聊应生成 Interaction");

        assert_eq!(p2p_interaction.audience.kind(), AudienceKind::Private);
        assert_eq!(
            p2p_interaction.audience.disclosure(),
            DisclosurePolicy::PrivateOnly
        );
        assert_eq!(group_interaction.audience.kind(), AudienceKind::Group);
        assert_eq!(
            group_interaction.audience.disclosure(),
            DisclosurePolicy::Workspace
        );
    }

    #[test]
    fn rejected_and_empty_prompt_do_not_create_interaction() {
        let rejected = interaction_report(
            crate::RemoteFeishuChatType::P2p,
            crate::RemoteFeishuIncomingMessageKind::Text,
            crate::RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist,
            Some("被拒绝"),
        );
        let empty = interaction_report(
            crate::RemoteFeishuChatType::P2p,
            crate::RemoteFeishuIncomingMessageKind::Text,
            crate::RemoteFeishuGovernanceDecisionStatus::Accepted,
            Some("   "),
        );

        assert!(
            interaction_from_gateway_report(&rejected, "workspace", 42)
                .expect("拒绝事件应可判定")
                .is_none()
        );
        assert!(
            interaction_from_gateway_report(&empty, "workspace", 42)
                .expect("空 prompt 应可判定")
                .is_none()
        );
    }

    #[test]
    fn unknown_chat_type_is_rejected_for_accepted_text() {
        let report = interaction_report(
            crate::RemoteFeishuChatType::Unknown,
            crate::RemoteFeishuIncomingMessageKind::Text,
            crate::RemoteFeishuGovernanceDecisionStatus::Accepted,
            Some("消息"),
        );

        let error = interaction_from_gateway_report(&report, "workspace", 42)
            .expect_err("未知 chat_type 不应进入 Interaction");

        assert!(error.contains("chat_type"));
    }

    fn interaction_report(
        chat_type: crate::RemoteFeishuChatType,
        message_kind: crate::RemoteFeishuIncomingMessageKind,
        status: crate::RemoteFeishuGovernanceDecisionStatus,
        prompt: Option<&str>,
    ) -> crate::RemoteFeishuGatewayGovernanceEventReport {
        let request_id = "request-1";
        let chat_id = "chat-1";
        let record = crate::RemoteFeishuGovernanceRecord {
            schema_version:
                crate::remote_feishu_governance::REMOTE_FEISHU_GOVERNANCE_SCHEMA_VERSION,
            session_id: 7,
            chat_id_sha256: sha256_hex(chat_id),
            request_id_sha256: sha256_hex(request_id),
            sender_open_id_sha256: sha256_hex("sender-1"),
            text_preview: prompt.unwrap_or_default().to_owned(),
            message_kind: Some(message_kind.as_str().to_owned()),
            mentioned: true,
            queued: false,
            session_busy: false,
            decision_status: status,
            thread_state: crate::RemoteFeishuThreadState::Processing,
            reason: "test".to_owned(),
            recorded_at_ms: 42,
        };
        crate::RemoteFeishuGatewayGovernanceEventReport {
            meta: crate::RemoteFeishuGatewayInboundMeta {
                request_id: request_id.to_owned(),
                message_id: Some("message-1".to_owned()),
                chat_id: chat_id.to_owned(),
                chat_id_sha256: sha256_hex(chat_id),
                operator_open_id: Some("sender-1".to_owned()),
                chat_type: Some(chat_type),
                reply_context: None,
                message_kind: Some(message_kind),
            },
            decision: crate::RemoteFeishuGovernanceDecision {
                status,
                thread_state: crate::RemoteFeishuThreadState::Processing,
                reason: "test".to_owned(),
                dispatch_report: None,
                record,
            },
            agent_prompt: prompt.map(str::to_owned),
            quoted_message: None,
            agent_files: Vec::new(),
        }
    }
}

/// Session Event 的 Feishu outbox 投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuOutboundEvent {
    /// Session ID。
    pub session_id: String,
    /// Session cursor 序号。
    pub sequence: u64,
    /// Host generation。
    pub generation: String,
    /// 事件类型。
    pub event_type: String,
    /// 事件 payload。
    pub payload: Value,
}

/// Feishu egress adapter；仅接收已提交 Session Event。
#[derive(Debug, Default, Clone, Copy)]
pub struct FeishuEventAdapter;

impl FeishuEventAdapter {
    /// 将 Session Event 转换为可持久化 outbox payload。
    pub fn project(event: &SessionEventRecord) -> Result<FeishuOutboundEvent, serde_json::Error> {
        Ok(FeishuOutboundEvent {
            session_id: event.session_id.to_string(),
            sequence: event.sequence,
            generation: event.generation.clone(),
            event_type: event.event_type.clone(),
            payload: serde_json::from_str(&event.payload_json)?,
        })
    }
}
