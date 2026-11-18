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
    ActivityQuery, ActivityService, ActivityServiceError, ActivitySnapshot,
    AnnotateChildOriginCommand, AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand,
    FailTurnCommand, ForkThreadCommand, GoalCommandService, GoalCommandServiceError,
    GoalExecutionSpec, GoalQueryService, GoalQueryServiceError, InterruptTurnCommand,
    NameThreadCommand, RepairOpenTurnCommand, ResumeThreadCommand, SessionQueryService,
    SessionSnapshot, StartEmptyThreadCommand, StartThreadCommand, StartTurnCommand, ThreadService,
    TurnRepairOutcome, WorkspaceGitSnapshot, WorkspaceIdentity, WorkspaceResolver,
    WorkspaceResolverError, WorkspaceSnapshot, WorkspaceSnapshotFieldState, interaction_session_id,
    is_ulid, new_interaction_id, new_session_id, new_ulid, project_threads,
};
pub use domain::WorkspaceRecord;
pub use domain::session_action::{default_action_priority, effective_action_priority};
pub use domain::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord, SessionKind,
    SessionRuntimeRecord, SessionTrustScope,
};
pub use domain::{
    Thread, ThreadCompaction, ThreadDetail, ThreadEvent, ThreadEventRecord, ThreadHistoryFileRef,
    ThreadHistoryImageRef, ThreadHistoryItem, ThreadHistoryReceipt, ThreadHistoryStatus, ThreadId,
    ThreadListQuery, ThreadSettings, ThreadSource, ThreadStatus, ThreadSummary, ThreadSummaryPage,
    Turn, TurnId, TurnStatus,
};
pub use infrastructure::{
    AXON_RUNTIME_DATA_DIR, JsonlThreadEventStore, SESSION_STORE_DIR, THREAD_EVENTS_FILE,
    ThreadEventReadIssue, ThreadEventScan, ThreadIndexEntry, ThreadIndexSnapshot, ThreadTailRead,
    session_store_path,
};
pub use infrastructure::{
    SESSION_EVENT_ARCHIVE_SUBDIR, SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES,
    SESSION_EVENT_MISSING_ARCHIVE_TYPE, SqliteRuntimeStore, WalCheckpoint,
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
    CompletionNotificationRecord, DecisionStore, DecisionStoreError, EventLogStore, ExecutionStore,
    ExecutionStoreError, ExecutionTrigger, GoalCommitError, GoalCommitStore, GoalMutationStore,
    GoalPage, GoalSessionRequest, GoalStore, GoalStoreError, HealthStore, InteractionAppendReceipt,
    InteractionCursor, InteractionProcessingLease, InteractionQueryStore, InteractionStore,
    InteractionStoreError, LedgerCommitError, LedgerCommitStore, LedgerEventStore,
    LedgerEventStoreError, NewSessionRequest, NotificationDeliveryState, RESULT_ACK_SOURCE_EXPIRED,
    RESULT_ACK_SOURCE_READ, ResultStore, ResultStoreError, RuntimeHealthCounters, RuntimeStore,
    RuntimeStoreError, ScheduleSessionRequest, SessionQueryStore, SessionSummaryRow,
    SubagentSessionRequest, UserQuestionStore, UserQuestionStoreError, WorkspaceStore,
    WorkspaceStoreError,
};
pub use ports::{ResumePointerStore, ResumePointerStoreError};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
