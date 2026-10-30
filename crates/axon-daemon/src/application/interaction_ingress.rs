//! JARVIS Interaction durable ingress 与 active steer 分流。

use axon_domain::{
    AudienceKind, DisclosurePolicy, InteractionId, InteractionRecord, InteractionStatus, ScopeRef,
};
use axon_session::{
    InteractionAppendReceipt, InteractionProcessingLease, InteractionStore, InteractionStoreError,
};
use std::sync::Arc;

/// Interaction 入口策略错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionIngressPolicyError {
    /// 外部入口只能提交 Received 初态。
    InvalidInitialStatus,
    /// 自然语言 Interaction 必须绑定 Workspace。
    WorkspaceRequired,
    /// 受众类型与披露边界组合不允许。
    AudienceDisclosureMismatch,
    /// 回复路由必须沿用入口来源渠道。
    ReplyRouteChannelMismatch,
    /// 回复路由目标不能为空。
    EmptyReplyRouteDestination,
}

impl std::fmt::Display for InteractionIngressPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInitialStatus => {
                formatter.write_str("Interaction 入站状态必须为 received")
            }
            Self::WorkspaceRequired => formatter.write_str("Interaction 必须绑定 Workspace"),
            Self::AudienceDisclosureMismatch => {
                formatter.write_str("audience 与 disclosure policy 不匹配")
            }
            Self::ReplyRouteChannelMismatch => {
                formatter.write_str("reply route 渠道必须与来源渠道一致")
            }
            Self::EmptyReplyRouteDestination => formatter.write_str("reply route 目标不能为空"),
        }
    }
}

impl std::error::Error for InteractionIngressPolicyError {}

/// 统一自然语言入口策略。
#[derive(Debug, Clone, Copy, Default)]
pub struct InteractionIngressPolicy;

impl InteractionIngressPolicy {
    /// 校验 Interaction 的场景、受众和回复路由边界。
    pub fn validate(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<(), InteractionIngressPolicyError> {
        if interaction.status != InteractionStatus::Received {
            return Err(InteractionIngressPolicyError::InvalidInitialStatus);
        }
        if matches!(interaction.scope, ScopeRef::Global) {
            return Err(InteractionIngressPolicyError::WorkspaceRequired);
        }
        let audience_valid = matches!(
            (
                interaction.audience.kind(),
                interaction.audience.disclosure()
            ),
            (AudienceKind::Private, DisclosurePolicy::PrivateOnly)
                | (AudienceKind::Group, DisclosurePolicy::Workspace)
        );
        if !audience_valid {
            return Err(InteractionIngressPolicyError::AudienceDisclosureMismatch);
        }
        if interaction.reply_route.channel() != interaction.source.channel() {
            return Err(InteractionIngressPolicyError::ReplyRouteChannelMismatch);
        }
        if interaction.reply_route.destination().is_empty() {
            return Err(InteractionIngressPolicyError::EmptyReplyRouteDestination);
        }
        Ok(())
    }
}

/// 当前 Interaction 的临时投递结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionDelivery {
    /// 已写入 durable inbox，等待下一次主 turn claim。
    Queued,
    /// 已写入 durable inbox，并成功送入活跃主 turn 的安全边界。
    Steered,
}

/// Interaction ingress 的稳定回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionIngressReceipt {
    /// Interaction 标识。
    pub interaction_id: InteractionId,
    /// 本次是否新写入 durable inbox。
    pub inserted: bool,
    /// 临时投递结果。
    pub delivery: InteractionDelivery,
    /// active steer 失败原因；不影响 durable queued 恢复。
    pub steer_error: Option<String>,
}

/// 活跃主 turn 的安全边界注入端口。
pub trait ActiveTurnSteerSink: Send + Sync {
    /// 尝试注入一条已经 durable append 的 Interaction。
    ///
    /// 返回 `Ok(false)` 表示当前没有可用的活跃 turn，调用方应等待下一轮 claim。
    fn try_steer(&self, interaction: &InteractionRecord) -> Result<bool, String>;

    /// 尝试把输入追加到同一会话的活动 Primary Turn。
    ///
    /// 返回活动 Primary Interaction ID 表示消息已复用原回合；返回 `None` 表示
    /// 当前没有匹配的活动回合，调用方应创建新的 Interaction。
    fn append_turn_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, String> {
        let _ = interaction;
        Ok(None)
    }
}

impl<T> ActiveTurnSteerSink for Arc<T>
where
    T: ActiveTurnSteerSink + ?Sized,
{
    fn try_steer(&self, interaction: &InteractionRecord) -> Result<bool, String> {
        self.as_ref().try_steer(interaction)
    }

    fn append_turn_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, String> {
        self.as_ref().append_turn_input(interaction)
    }
}

/// 未配置活跃主 turn 时，明确保留 durable queued 语义。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoActiveTurnSteerSink;

impl ActiveTurnSteerSink for NoActiveTurnSteerSink {
    fn try_steer(&self, _interaction: &InteractionRecord) -> Result<bool, String> {
        Ok(false)
    }
}

/// 统一自然语言入口；先持久化，再尝试 active steer。
#[derive(Debug, Clone)]
pub struct PrimaryTurnIngress<S, K> {
    store: S,
    steer_sink: K,
    policy: InteractionIngressPolicy,
    now_ms: u64,
    lease_duration_ms: u64,
}

impl<S, K> PrimaryTurnIngress<S, K>
where
    S: InteractionStore,
    K: ActiveTurnSteerSink,
{
    /// 创建 Interaction ingress。
    #[must_use]
    pub fn new(store: S, steer_sink: K, now_ms: u64, lease_duration_ms: u64) -> Self {
        Self {
            store,
            steer_sink,
            policy: InteractionIngressPolicy,
            now_ms,
            lease_duration_ms,
        }
    }

    /// 按会话亲和键追加一条输入；无匹配活动回合时返回 None。
    pub fn append_turn_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<Option<InteractionId>, InteractionStoreError> {
        self.policy.validate(interaction).map_err(|error| {
            InteractionStoreError::InvalidState(format!("入口策略拒绝：{error}"))
        })?;
        if is_system_generated(interaction) {
            return Ok(None);
        }
        self.steer_sink
            .append_turn_input(interaction)
            .map_err(InteractionStoreError::InvalidState)
    }

    /// 路由一条输入：优先追加到活动回合，否则创建新的 queued Interaction。
    pub fn route_input(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionIngressReceipt, InteractionStoreError> {
        if let Some(primary_interaction_id) = self.append_turn_input(interaction)? {
            return Ok(InteractionIngressReceipt {
                interaction_id: primary_interaction_id,
                inserted: false,
                delivery: InteractionDelivery::Steered,
                steer_error: None,
            });
        }
        self.store.append(interaction).map(queued_receipt)
    }

    /// 创建新的 durable Interaction，并在新记录上尝试一次 active steer。
    pub fn submit_new_interaction(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionIngressReceipt, InteractionStoreError> {
        self.policy.validate(interaction).map_err(|error| {
            InteractionStoreError::InvalidState(format!("入口策略拒绝：{error}"))
        })?;
        self.submit_validated(interaction)
    }

    /// 兼容旧调用方；新入口应明确选择 route_input 或 submit_new_interaction。
    pub fn submit(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionIngressReceipt, InteractionStoreError> {
        self.submit_new_interaction(interaction)
    }

    fn submit_validated(
        &self,
        interaction: &InteractionRecord,
    ) -> Result<InteractionIngressReceipt, InteractionStoreError> {
        let append = self.store.append(interaction)?;
        self.receipt_from_append(interaction, append)
    }

    fn receipt_from_append(
        &self,
        interaction: &InteractionRecord,
        append: InteractionAppendReceipt,
    ) -> Result<InteractionIngressReceipt, InteractionStoreError> {
        if !append.inserted {
            return Ok(InteractionIngressReceipt {
                interaction_id: append.interaction_id,
                inserted: false,
                delivery: InteractionDelivery::Queued,
                steer_error: None,
            });
        }
        let Some(lease) = self.store.claim_processing_by_id(
            &append.interaction_id,
            self.now_ms,
            self.lease_duration_ms,
        )?
        else {
            return Ok(queued_receipt(append));
        };
        match self.steer_sink.try_steer(interaction) {
            Ok(true) => {
                self.store.mark_understood(
                    &lease.interaction.interaction_id,
                    &lease.lease_token,
                    self.now_ms,
                )?;
                Ok(InteractionIngressReceipt {
                    interaction_id: append.interaction_id,
                    inserted: true,
                    delivery: InteractionDelivery::Steered,
                    steer_error: None,
                })
            }
            Ok(false) => {
                self.release_to_queue(&lease)?;
                Ok(queued_receipt(append))
            }
            Err(error) => {
                self.release_to_queue(&lease)?;
                Ok(InteractionIngressReceipt {
                    steer_error: Some(error),
                    ..queued_receipt(append)
                })
            }
        }
    }

    fn release_to_queue(
        &self,
        lease: &InteractionProcessingLease,
    ) -> Result<(), InteractionStoreError> {
        self.store.release_processing(
            &lease.interaction.interaction_id,
            &lease.lease_token,
            self.now_ms,
        )
    }
}

fn queued_receipt(append: InteractionAppendReceipt) -> InteractionIngressReceipt {
    InteractionIngressReceipt {
        interaction_id: append.interaction_id,
        inserted: append.inserted,
        delivery: InteractionDelivery::Queued,
        steer_error: None,
    }
}

fn is_system_generated(interaction: &InteractionRecord) -> bool {
    let source_message_id = interaction.source.source_message_id();
    interaction.source.channel() == axon_domain::SYSTEM_SOURCE_CHANNEL
        || source_message_id.starts_with(axon_domain::COMPLETION_STEER_SOURCE_PREFIX)
        || source_message_id.starts_with(axon_domain::CRON_WAKE_SOURCE_PREFIX)
}

#[cfg(test)]
#[path = "interaction_ingress_tests.rs"]
mod tests;
