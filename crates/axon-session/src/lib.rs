//! Axon 统一 Session/Thread 子系统。
//!
//! 本 crate 是会话事实来源，负责 Thread、Turn、事件流、fork、resume、
//! 只读投影。CLI/TUI/runtime 只能作为 adapter
//! 调用这里的 API，不能再维护各自独立的 session store。

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod ports;

pub use application::THREAD_EVENT_SCHEMA_VERSION;
pub use application::{
    ActivityQuery, ActivityService, ActivityServiceError, ActivitySnapshot,
    AnnotateChildOriginCommand, AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand,
    FailTurnCommand, ForkThreadCommand, GoalCommandService, GoalCommandServiceError,
    GoalExecutionSpec, GoalQueryService, GoalQueryServiceError, InterruptTurnCommand,
    NameThreadCommand, RepairOpenTurnsCommand, ResumeThreadCommand, SessionQueryService,
    SessionSnapshot, StartEmptyThreadCommand, StartThreadCommand, StartTurnCommand, ThreadService,
    TurnRepairOutcome, WorkspaceGitSnapshot, WorkspaceIdentity, WorkspaceResolver,
    WorkspaceResolverError, WorkspaceSnapshot, WorkspaceSnapshotFieldState, interaction_session_id,
    is_ulid, new_interaction_id, new_session_id, new_ulid, project_threads,
};
pub use domain::WorkspaceRecord;
pub use domain::context_snapshot::ContextSnapshot;
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
    CONTROL_DATABASE_FILE_NAME, EventBodyTieringPlan, EventBodyTieringReport,
    SESSION_EVENT_ARCHIVE_SUBDIR, SESSION_EVENT_BODY_ARCHIVED_TYPE,
    SESSION_EVENT_INLINE_PAYLOAD_LIMIT_BYTES, SESSION_EVENT_MISSING_ARCHIVE_TYPE,
    SqliteRuntimeStore, ThreadTurnFacts, TurnActivity, WalCheckpoint, body_archive_path,
    body_archive_ref, conclusive_work_items_without_terminal_kind_sql,
    disable_automatic_wal_checkpoint, read_body_archive,
};
pub use ports::ExecutionSettlement;
pub use ports::ThreadActivationSnapshot;
pub use ports::ThreadEventStore;
pub use ports::requires_attribution;
pub use ports::{
    AgentContinuationAppendOutcome, AgentContinuationQuery, AgentContinuationRecord,
    AgentContinuationState, AgentContinuationStore, AgentContinuationStoreError,
    AgentMailboxAppendOutcome, AgentMailboxMessageKind, AgentMailboxMessageRecord,
    AgentMailboxMessageState, AgentMailboxQuery, AgentMailboxRetryPolicy, AgentMailboxStore,
    AgentMailboxStoreError, CheckpointTelemetry, ClientDeliveryStore, ClientDeliveryStoreError,
    ClientInboxEntry, ClientPresenceRecord, CompletionDeliveryRecord, CompletionDeliveryTarget,
    CompletionNotificationRecord, ContextSnapshotStore, DecisionStore, DecisionStoreError,
    DispatchTerminalKind, EventLogStore, ExecutionStore, ExecutionStoreError, ExecutionTrigger,
    GoalCommitError, GoalCommitStore, GoalMutationStore, GoalPage, GoalSessionRequest, GoalStore,
    GoalStoreError, HealthStore, InFlightTurnFacts, InteractionAppendReceipt, InteractionCursor,
    InteractionProcessingLease, InteractionQueryStore, InteractionSettlementStore,
    InteractionStore, InteractionStoreError, InteractionTurnHead, LedgerCommitError,
    LedgerCommitStore, LedgerEventStore, LedgerEventStoreError, NewSessionRequest,
    NotificationDeliveryState, RESULT_ACK_SOURCE_EXPIRED, RESULT_ACK_SOURCE_READ,
    RecoveringSettlement, ResultStore, ResultStoreError, RosterLastTurnStatus, RosterStatus,
    RosterTurnStatus, RuntimeHealthCounters, RuntimeStore, RuntimeStoreError,
    ScheduleSessionRequest, SessionQueryStore, SessionSummaryRow, SubagentSessionRequest,
    UserQuestionStore, UserQuestionStoreError, WorkspaceStore, WorkspaceStoreError,
};
pub use ports::{ResumePointerStore, ResumePointerStoreError};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
