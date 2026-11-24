//! Session 端口。

/// 子代理终态 continuation 引用的持久化端口。
pub mod agent_continuation_store;
/// agent 间消息 mailbox 的持久化端口。
pub mod agent_mailbox_store;
/// Interaction 收件箱的持久化端口。
pub mod client_delivery_store;
/// 会话级上下文快照的只读端口。
pub mod context_snapshot_store;
pub mod decision_store;
pub mod execution_store;
pub mod goal_commit_store;
pub mod goal_store;
/// Runtime 健康聚合端口。
pub mod health_store;
pub mod ingress_cursor_store;
/// 在途轮次的判死读取与收口端口。
pub mod interaction_settlement_store;
/// 入站事实与处理状态端口。
pub mod interaction_store;
pub mod lease_store;
pub mod ledger_commit_store;
pub mod ledger_event_store;
pub mod result_store;
/// 恢复指针读写端口。
pub mod resume_pointer_store;
/// 名册状态的派生口径。
pub mod roster_status;
pub mod schedule_store;
pub mod session_action_store;
pub mod session_event_store;
/// 车道聚合查询端口。
pub mod session_query_store;
pub mod user_question_store;
pub mod workspace_store;

mod event_log_store;
mod event_store;
mod runtime_store;

pub use agent_continuation_store::{
    AgentContinuationAppendOutcome, AgentContinuationQuery, AgentContinuationRecord,
    AgentContinuationState, AgentContinuationStore, AgentContinuationStoreError,
};
pub use agent_mailbox_store::{
    AgentMailboxAppendOutcome, AgentMailboxMessageKind, AgentMailboxMessageRecord,
    AgentMailboxMessageState, AgentMailboxQuery, AgentMailboxRetryPolicy, AgentMailboxStore,
    AgentMailboxStoreError,
};
pub use client_delivery_store::{
    ClientDeliveryStore, ClientDeliveryStoreError, ClientInboxEntry, ClientPresenceRecord,
    CompletionDeliveryRecord, CompletionDeliveryTarget, CompletionNotificationRecord,
    NotificationDeliveryState,
};
pub use context_snapshot_store::ContextSnapshotStore;
pub use decision_store::{DecisionStore, DecisionStoreError};
pub use event_log_store::{EventLogStore, requires_attribution};
pub use event_store::{ThreadActivationSnapshot, ThreadEventStore};
pub use execution_store::{ExecutionStore, ExecutionStoreError, ExecutionTrigger};
pub use goal_commit_store::{GoalCommitError, GoalCommitStore};
pub use goal_store::{GoalMutationStore, GoalPage, GoalStore, GoalStoreError};
pub use health_store::{HealthStore, RuntimeHealthCounters};
pub use ingress_cursor_store::SessionIngressCursorStore;
pub use interaction_settlement_store::{
    InFlightTurnFacts, InteractionSettlementStore, QueuedTurnFacts,
};
pub use interaction_store::{
    DispatchTerminalKind, InteractionAppendReceipt, InteractionCursor, InteractionProcessingLease,
    InteractionQueryStore, InteractionStore, InteractionStoreError, InteractionTurnHead,
};
pub use lease_store::LeaseStore;
pub use ledger_commit_store::{ExecutionSettlement, LedgerCommitError, LedgerCommitStore};
pub use ledger_event_store::{LedgerEventStore, LedgerEventStoreError};
pub use result_store::{
    RESULT_ACK_SOURCE_EXPIRED, RESULT_ACK_SOURCE_READ, ResultStore, ResultStoreError,
};
pub use resume_pointer_store::{ResumePointerStore, ResumePointerStoreError};
pub use roster_status::{RosterLastTurnStatus, RosterStatus, RosterTurnStatus};
pub use runtime_store::{
    CheckpointTelemetry, GoalSessionRequest, NewSessionRequest, RecoveringSettlement, RuntimeStore,
    RuntimeStoreError, ScheduleSessionRequest, SubagentSessionRequest,
};
pub use session_query_store::{SessionQueryStore, SessionSummaryRow};
pub use user_question_store::{UserQuestionStore, UserQuestionStoreError};
pub use workspace_store::{WorkspaceStore, WorkspaceStoreError};
