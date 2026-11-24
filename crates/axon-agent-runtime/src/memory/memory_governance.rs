//! Memory governance 动作与门禁。

use axon_domain::{
    CellId, ExecutionRequestId, MemoryId, MemoryRecord, MemoryState, MemoryTombstone, SessionId,
};

use crate::MemoryFooterEvent;

/// Memory governance 动作来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryGovernanceOrigin {
    /// 本机 Memory Inspector。
    LocalMemoryInspector,
    /// 本机 Agent Pane。
    LocalAgentPane,
    /// 远控侧车。
    RemoteSidecar,
}

/// Memory governance 动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryGovernanceAction {
    /// 本机 review，不修改可召回集合。
    Review {
        /// 被 review 的记忆 ID。
        memory_id: MemoryId,
    },
    /// 遗忘记忆并生成 tombstone。
    Forget {
        /// 被遗忘的记忆 ID。
        memory_id: MemoryId,
        /// 遗忘原因，不包含原文。
        reason: String,
    },
    /// 合并冲突记忆。
    Merge {
        /// 待合并的来源记忆 ID。
        source: MemoryId,
        /// 合并目标记忆 ID。
        target: MemoryId,
    },
    /// 将 weak memory 提升为 strong memory。
    PromoteToStrong {
        /// 被提升的记忆 ID。
        memory_id: MemoryId,
    },
    /// 隔离记忆。
    Quarantine {
        /// 被隔离的记忆 ID。
        memory_id: MemoryId,
        /// 隔离原因，不包含原文。
        reason: String,
    },
}

/// Memory governance 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryGovernanceRequest {
    /// 执行关联请求 ID。
    pub execution_request_id: ExecutionRequestId,
    /// 所属 session。
    pub session_id: SessionId,
    /// 所属 RuntimeCell。
    pub cell_id: CellId,
    /// 动作来源。
    pub origin: MemoryGovernanceOrigin,
    /// 治理动作。
    pub action: MemoryGovernanceAction,
    /// 评估时间。
    pub now_ms: u64,
}

/// Memory governance 决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryGovernanceDecision {
    /// 动作已允许。
    Allow {
        /// 更新后的记忆记录；review/merge 只产生事件时为空。
        updated_record: Option<Box<MemoryRecord>>,
        /// Runtime 事件。
        event: MemoryRuntimeEvent,
        /// Footer 刷新事件。
        footer_event: MemoryFooterEvent,
    },
    /// 动作被阻断。
    Block {
        /// 阻断原因。
        reason: MemoryGovernanceBlockReason,
        /// Runtime 事件。
        event: MemoryRuntimeEvent,
    },
}

/// Memory governance 阻断原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryGovernanceBlockReason {
    /// 远控不得执行治理写动作。
    RemoteActionDenied,
    /// 目标记忆不匹配。
    MemoryIdMismatch,
    /// 当前状态不允许该动作。
    InvalidState,
    /// 缺少可阻断引用。
    MissingBlockedRef,
}

/// Memory runtime 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryRuntimeEvent {
    /// Governance 动作已应用。
    GovernanceActionApplied {
        /// 应用的动作。
        action: MemoryGovernanceAction,
        /// 发生时间。
        occurred_at_ms: u64,
    },
    /// Governance 动作被拒绝。
    GovernanceActionDenied {
        /// 被拒绝的动作。
        action: MemoryGovernanceAction,
        /// 阻断原因。
        reason: MemoryGovernanceBlockReason,
    },
}

/// Memory governance runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryGovernanceRuntime;

impl MemoryGovernanceRuntime {
    /// 评估并应用治理动作；真实持久化由外层存储执行。
    #[must_use]
    pub fn evaluate(
        &self,
        record: &MemoryRecord,
        request: &MemoryGovernanceRequest,
    ) -> MemoryGovernanceDecision {
        if request.origin == MemoryGovernanceOrigin::RemoteSidecar {
            return block(request, MemoryGovernanceBlockReason::RemoteActionDenied);
        }
        if request.action.primary_memory_id() != record.id {
            return block(request, MemoryGovernanceBlockReason::MemoryIdMismatch);
        }

        let updated_record = match apply_action(record, &request.action) {
            Ok(record) => record,
            Err(reason) => return block(request, reason),
        };
        let updated_record = updated_record.map(Box::new);
        let updated_state = updated_record.as_ref().map(|item| item.state);

        MemoryGovernanceDecision::Allow {
            updated_record,
            event: MemoryRuntimeEvent::GovernanceActionApplied {
                action: request.action.clone(),
                occurred_at_ms: request.now_ms,
            },
            footer_event: MemoryFooterEvent::MemoryHealthChanged {
                strong_count: usize::from(matches!(updated_state, Some(MemoryState::Strong))),
                weak_count: usize::from(matches!(updated_state, Some(MemoryState::Weak))),
                quarantined_count: usize::from(matches!(
                    updated_state,
                    Some(MemoryState::Quarantined)
                )),
                rejected_count: 0,
            },
        }
    }
}

impl MemoryGovernanceAction {
    fn primary_memory_id(&self) -> MemoryId {
        match self {
            Self::Review { memory_id }
            | Self::Forget { memory_id, .. }
            | Self::PromoteToStrong { memory_id }
            | Self::Quarantine { memory_id, .. } => *memory_id,
            Self::Merge { source, .. } => *source,
        }
    }
}

fn apply_action(
    record: &MemoryRecord,
    action: &MemoryGovernanceAction,
) -> Result<Option<MemoryRecord>, MemoryGovernanceBlockReason> {
    match action {
        MemoryGovernanceAction::Review { .. } | MemoryGovernanceAction::Merge { .. } => Ok(None),
        MemoryGovernanceAction::Forget { reason, .. } => forget_record(record, reason),
        MemoryGovernanceAction::PromoteToStrong { .. } => promote_record(record),
        MemoryGovernanceAction::Quarantine { reason, .. } => quarantine_record(record, reason),
    }
}

fn forget_record(
    record: &MemoryRecord,
    reason: &str,
) -> Result<Option<MemoryRecord>, MemoryGovernanceBlockReason> {
    let blocked_ref = record
        .content_ref
        .clone()
        .or_else(|| {
            record
                .tombstone
                .as_ref()
                .map(|item| item.blocked_ref.clone())
        })
        .ok_or(MemoryGovernanceBlockReason::MissingBlockedRef)?;
    let mut updated = record.clone();
    updated.state = MemoryState::Tombstone;
    updated.content_ref = None;
    updated.tombstone = Some(MemoryTombstone {
        blocked_ref,
        reason: reason.to_owned(),
    });
    Ok(Some(updated))
}

fn promote_record(
    record: &MemoryRecord,
) -> Result<Option<MemoryRecord>, MemoryGovernanceBlockReason> {
    if record.state != MemoryState::Weak || record.content_ref.is_none() {
        return Err(MemoryGovernanceBlockReason::InvalidState);
    }
    let mut updated = record.clone();
    updated.state = MemoryState::Strong;
    Ok(Some(updated))
}

fn quarantine_record(
    record: &MemoryRecord,
    reason: &str,
) -> Result<Option<MemoryRecord>, MemoryGovernanceBlockReason> {
    let blocked_ref = record
        .content_ref
        .clone()
        .ok_or(MemoryGovernanceBlockReason::MissingBlockedRef)?;
    let mut updated = record.clone();
    updated.state = MemoryState::Quarantined;
    updated.content_ref = None;
    updated.tombstone = Some(MemoryTombstone {
        blocked_ref,
        reason: reason.to_owned(),
    });
    Ok(Some(updated))
}

fn block(
    request: &MemoryGovernanceRequest,
    reason: MemoryGovernanceBlockReason,
) -> MemoryGovernanceDecision {
    MemoryGovernanceDecision::Block {
        reason,
        event: MemoryRuntimeEvent::GovernanceActionDenied {
            action: request.action.clone(),
            reason,
        },
    }
}
