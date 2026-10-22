use axon_domain::{InteractionId, InteractionRecord};
use serde::{Deserialize, Serialize};

/// 提交 Interaction 的内部请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitInteractionRequest {
    /// 已完成场景解析的 Interaction。
    pub interaction: InteractionRecord,
}

/// 登记「正文已就绪」投递项的请求。
///
/// 系统发起的投递（定时任务等）没有主 turn 汇总环节：正文本身就是最终交付内容，
/// 由 Daemon 统一登记到持久收件箱并投递到当时所有可用渠道。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueDeliveryRequest {
    /// 目标 Workspace。
    pub workspace_id: String,
    /// 发起投递的来源标识，例如 Cron 任务 ID。
    pub goal_id: String,
    /// 投递项稳定身份；同一身份重复登记幂等。
    pub delivery_id: String,
    /// 已就绪的最终交付正文。
    pub content: String,
}

/// 投递项登记回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnqueueDeliveryReceipt {
    /// 投递项通知 ID。
    pub notification_id: String,
    /// 本次登记的渠道目标数量。
    pub targets: u32,
}

/// Interaction 持久化后的稳定回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitInteractionReceipt {
    /// Interaction 标识。
    pub interaction_id: InteractionId,
    /// 是否成功写入或命中已有记录。
    pub accepted: bool,
    /// 是否命中来源幂等键对应的已有记录。
    pub deduplicated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{
        AudienceContext, AudienceKind, DisclosurePolicy, InteractionSource, ReplyRoute, ScopeRef,
    };

    fn request() -> SubmitInteractionRequest {
        SubmitInteractionRequest {
            interaction: InteractionRecord::new(
                InteractionId::new("interaction-1").expect("合法 ID"),
                InteractionSource::new("cli", "message-1").expect("合法来源"),
                ScopeRef::workspace("workspace-a").expect("合法作用域"),
                AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
                ReplyRoute::new("cli", "stdout").expect("合法路由"),
                "检查状态",
                1,
            )
            .expect("Interaction 应创建"),
        }
    }

    #[test]
    fn submit_interaction_request_contains_no_runtime_identity_fields() {
        let value = serde_json::to_value(request()).expect("请求应可序列化");
        let object = value.as_object().expect("请求应为对象");
        assert!(object.get("sessionId").is_none());
        assert!(object.get("threadId").is_none());
        assert!(object.get("runId").is_none());
    }

    #[test]
    fn submit_interaction_receipt_round_trips() {
        let receipt = SubmitInteractionReceipt {
            interaction_id: InteractionId::new("interaction-1").expect("合法 ID"),
            accepted: true,
            deduplicated: true,
        };
        let encoded = serde_json::to_value(&receipt).expect("回执应可序列化");
        let decoded: SubmitInteractionReceipt =
            serde_json::from_value(encoded).expect("回执应可反序列化");
        assert_eq!(decoded, receipt);
    }
}
