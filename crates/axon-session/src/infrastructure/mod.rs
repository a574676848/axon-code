//! Session 基础设施实现。

use std::path::{Path, PathBuf};

mod jsonl_store;
mod sqlite_runtime_store;

#[cfg(test)]
#[path = "sqlite_runtime_store_activity_tests.rs"]
mod sqlite_runtime_store_activity_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_agent_continuation_tests.rs"]
mod sqlite_runtime_store_agent_continuation_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_agent_mailbox_tests.rs"]
mod sqlite_runtime_store_agent_mailbox_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_event_batch_tests.rs"]
mod sqlite_runtime_store_event_batch_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_execution_lease_tests.rs"]
mod sqlite_runtime_store_execution_lease_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_execution_session_tests.rs"]
mod sqlite_runtime_store_execution_session_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_goal_activity_tests.rs"]
mod sqlite_runtime_store_goal_activity_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_health_tests.rs"]
mod sqlite_runtime_store_health_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_interaction_tests.rs"]
mod sqlite_runtime_store_interaction_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_result_tests.rs"]
mod sqlite_runtime_store_result_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_schema_identity_tests.rs"]
mod sqlite_runtime_store_schema_identity_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_session_tests.rs"]
mod sqlite_runtime_store_session_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_tests.rs"]
mod sqlite_runtime_store_tests;
#[cfg(test)]
#[path = "sqlite_runtime_store_user_question_tests.rs"]
mod sqlite_runtime_store_user_question_tests;

/// Axon 运行数据目录名。
pub const AXON_RUNTIME_DATA_DIR: &str = "data";
/// Session 持久化目录名。
pub const SESSION_STORE_DIR: &str = "sessions";

/// 解析 Axon 根目录下统一的 Session 持久化目录。
#[must_use]
pub fn session_store_path(root: &Path) -> PathBuf {
    root.join(AXON_RUNTIME_DATA_DIR).join(SESSION_STORE_DIR)
}

pub use jsonl_store::{
    JsonlThreadEventStore, THREAD_EVENTS_FILE, ThreadEventReadIssue, ThreadEventScan,
};
pub use sqlite_runtime_store::SqliteRuntimeStore;
pub use sqlite_runtime_store::{
    action_repository::ActionRepository, event_repository::EventRepository,
    lease_repository::LeaseRepository, runtime_repository::RuntimeRepository,
    schedule_repository::ScheduleRepository, transaction::TransactionBoundary,
};
