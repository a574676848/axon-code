//! Session 应用服务。

mod activity_service;
mod goal_command_service;
mod goal_query_service;
pub(crate) mod projection;
mod session_identity;
mod session_query_service;
mod thread_service;
mod thread_service_append;
mod workspace_resolver;

/// thread 事件 schema 版本。
///
/// 4：新增 `turnMetadataPatched`（治理回写只追加增量，不再复制整份轮次记录）。
/// 旧日志一律拒绝读取，不做补列与回填。
pub(crate) const THREAD_EVENT_SCHEMA_VERSION: u32 = 4;

pub use activity_service::{
    ActivityQuery, ActivityService, ActivityServiceError, ActivitySnapshot,
};
pub use goal_command_service::{GoalCommandService, GoalCommandServiceError, GoalExecutionSpec};
pub use goal_query_service::{GoalQueryService, GoalQueryServiceError};
pub use projection::project_threads;
pub use session_identity::{
    ULID_LENGTH, default_kind_for_interaction, interaction_session_id, is_ulid, new_interaction_id,
    new_session_id, new_ulid, trust_scope_of,
};
pub use session_query_service::{SessionQueryService, SessionSnapshot};
mod thread_maintenance;
mod thread_repair;
pub use thread_maintenance::AnnotateChildOriginCommand;
pub use thread_service::{
    AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand,
    ForkThreadCommand, InterruptTurnCommand, NameThreadCommand, RepairOpenTurnCommand,
    ResumeThreadCommand, StartEmptyThreadCommand, StartThreadCommand, StartTurnCommand,
    ThreadService, TurnRepairOutcome,
};
pub use workspace_resolver::{
    WorkspaceGitSnapshot, WorkspaceIdentity, WorkspaceResolver, WorkspaceResolverError,
    WorkspaceSnapshot, WorkspaceSnapshotFieldState,
};
