//! Axon 统一 Session/Thread 子系统。
//!
//! 本 crate 是会话事实来源，负责 Thread、Turn、事件流、fork、resume、
//! 只读投影。CLI/TUI/runtime 只能作为 adapter
//! 调用这里的 API，不能再维护各自独立的 session store。

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod ports;

pub use application::{
    ActivityQuery, ActivityService, ActivityServiceError, ActivitySnapshot, AppendTurnCommand,
    AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand, ForkThreadCommand,
    GoalCommandService, GoalCommandServiceError, GoalExecutionSpec, GoalQueryService,
    GoalQueryServiceError, InterruptTurnCommand, LaneQueryService, LaneSnapshot, NameThreadCommand,
    ResumeThreadCommand, StartEmptyThreadCommand, StartThreadCommand, StartTurnCommand,
    ThreadService, WorkspaceGitSnapshot, WorkspaceIdentity, WorkspaceResolver,
    WorkspaceResolverError, WorkspaceSnapshot, WorkspaceSnapshotFieldState,
    interaction_session_lane_id, session_lane_id,
};
pub use domain::WorkspaceRecord;
pub use domain::session_action::{default_action_priority, effective_action_priority};
pub use domain::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord,
    SessionRuntimeRecord,
};
pub use domain::{
    Thread, ThreadCompaction, ThreadDetail, ThreadEvent, ThreadEventRecord, ThreadHistoryFileRef,
    ThreadHistoryImageRef, ThreadHistoryItem, ThreadHistoryReceipt, ThreadHistoryStatus, ThreadId,
    ThreadListQuery, ThreadSettings, ThreadSource, ThreadStatus, ThreadSummary, ThreadSummaryPage,
    Turn, TurnId, TurnStatus,
};
pub use infrastructure::SqliteRuntimeStore;
pub use infrastructure::{
    AXON_RUNTIME_DATA_DIR, JsonlThreadEventStore, SESSION_STORE_DIR, THREAD_EVENTS_FILE,
    ThreadEventReadIssue, ThreadEventScan, session_store_path,
};
pub use ports::ExecutionSettlement;
pub use ports::ThreadActivationSnapshot;
pub use ports::ThreadEventStore;
pub use ports::{
    AgentContinuationAppendOutcome, AgentContinuationQuery, AgentContinuationRecord,
    AgentContinuationState, AgentContinuationStore, AgentContinuationStoreError,
    AgentMailboxAppendOutcome, AgentMailboxMessageKind, AgentMailboxMessageRecord,
    AgentMailboxMessageState, AgentMailboxQuery, AgentMailboxRetryPolicy, AgentMailboxStore,
    AgentMailboxStoreError, CheckpointTelemetry, ClientDeliveryStore, ClientDeliveryStoreError,
    ClientInboxEntry, ClientPresenceRecord, CompletionDeliveryRecord, CompletionDeliveryTarget,
    CompletionNotificationRecord, DecisionStore, DecisionStoreError, ExecutionStore,
    ExecutionStoreError, ExecutionTrigger, GoalCommitError, GoalCommitStore, GoalMutationStore,
    GoalPage, GoalStore, GoalStoreError, HealthStore, InteractionAppendReceipt, InteractionCursor,
    InteractionProcessingLease, InteractionQueryStore, InteractionStore, InteractionStoreError,
    LaneQueryStore, LaneSummaryRow, LedgerCommitError, LedgerCommitStore, LedgerEventStore,
    LedgerEventStoreError, NotificationDeliveryState, RESULT_ACK_SOURCE_EXPIRED,
    RESULT_ACK_SOURCE_READ, ResultStore, ResultStoreError, RuntimeHealthCounters, RuntimeStore,
    RuntimeStoreError, UserQuestionStore, UserQuestionStoreError, WorkspaceStore,
    WorkspaceStoreError,
};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
