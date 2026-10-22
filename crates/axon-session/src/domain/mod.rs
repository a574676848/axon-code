//! Session 领域模型。

mod event;
mod history;
pub mod runtime;
pub mod session_action;
pub mod session_event;
pub mod session_schedule;
mod settings;
mod thread;
mod turn;
pub mod workspace;

pub use event::{ThreadEvent, ThreadEventRecord};
pub use history::{
    ThreadHistoryFileRef, ThreadHistoryImageRef, ThreadHistoryItem, ThreadHistoryReceipt,
    ThreadHistoryStatus,
};
pub use runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord,
    SessionRuntimeRecord,
};
pub use settings::ThreadSettings;
pub use thread::{
    Thread, ThreadCompaction, ThreadDetail, ThreadId, ThreadListQuery, ThreadSource, ThreadStatus,
    ThreadSummary, ThreadSummaryPage,
};
pub use turn::{Turn, TurnId, TurnStatus};
pub use workspace::WorkspaceRecord;
