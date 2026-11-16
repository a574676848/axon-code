//! Session 应用服务。

mod activity_service;
mod goal_command_service;
mod goal_query_service;
pub(crate) mod projection;
mod session_identity;
mod session_query_service;
mod thread_service;
mod workspace_resolver;

pub(crate) const THREAD_EVENT_SCHEMA_VERSION: u32 = 3;

pub use activity_service::{
    ActivityQuery, ActivityService, ActivityServiceError, ActivitySnapshot,
};
pub use goal_command_service::{GoalCommandService, GoalCommandServiceError, GoalExecutionSpec};
pub use goal_query_service::{GoalQueryService, GoalQueryServiceError};
pub use session_identity::{interaction_session_id, session_id_for_route};
pub use session_query_service::{SessionQueryService, SessionSnapshot};
mod thread_repair;
pub use thread_service::{
    AnnotateChildOriginCommand, AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand,
    FailTurnCommand, ForkThreadCommand, InterruptTurnCommand, NameThreadCommand,
    RepairOpenTurnCommand, ResumeThreadCommand, StartEmptyThreadCommand, StartThreadCommand,
    StartTurnCommand, ThreadService, TurnRepairOutcome,
};
pub use workspace_resolver::{
    WorkspaceGitSnapshot, WorkspaceIdentity, WorkspaceResolver, WorkspaceResolverError,
    WorkspaceSnapshot, WorkspaceSnapshotFieldState,
};
