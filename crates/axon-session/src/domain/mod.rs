//! Session 领域模型。

/// 会话级上下文快照。
pub mod context_snapshot;
mod event;
mod history;
/// Interaction 状态口径。
pub mod interaction_status;
pub mod runtime;
pub mod session_action;
pub mod session_event;
pub mod session_schedule;
mod settings;
mod thread;
mod turn;
pub mod turn_repair;
pub mod workspace;

pub use context_snapshot::ContextSnapshot;
pub use event::{ThreadEvent, ThreadEventRecord};
pub use history::{
    ThreadHistoryFileRef, ThreadHistoryImageRef, ThreadHistoryItem, ThreadHistoryReceipt,
    ThreadHistoryStatus,
};
pub use runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord, SessionKind,
    SessionRuntimeRecord, SessionTrustScope,
};
pub use settings::ThreadSettings;
pub use thread::{
    Thread, ThreadCompaction, ThreadDetail, ThreadId, ThreadListQuery, ThreadSource, ThreadStatus,
    ThreadSummary, ThreadSummaryPage,
};
pub use turn::{Turn, TurnId, TurnStatus};
pub use workspace::WorkspaceRecord;
