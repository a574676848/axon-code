//! Session 应用服务。

mod activity_service;
mod goal_command_service;
mod goal_query_service;
mod lane_query_service;
pub(crate) mod projection;
mod session_lane;
mod thread_service;
mod workspace_resolver;

pub(crate) const THREAD_EVENT_SCHEMA_VERSION: u32 = 3;

pub use activity_service::{
    ActivityQuery, ActivityService, ActivityServiceError, ActivitySnapshot,
};
pub use goal_command_service::{GoalCommandService, GoalCommandServiceError, GoalExecutionSpec};
pub use goal_query_service::{GoalQueryService, GoalQueryServiceError};
pub use lane_query_service::{LaneQueryService, LaneSnapshot};
pub use session_lane::{interaction_session_lane_id, session_lane_id};
pub use thread_service::{
    AppendTurnCommand, AppendTurnItemsCommand, CompactThreadCommand, FailTurnCommand,
    ForkThreadCommand, InterruptTurnCommand, NameThreadCommand, ResumeThreadCommand,
    StartEmptyThreadCommand, StartThreadCommand, StartTurnCommand, ThreadService,
};
pub use workspace_resolver::{
    WorkspaceGitSnapshot, WorkspaceIdentity, WorkspaceResolver, WorkspaceResolverError,
    WorkspaceSnapshot, WorkspaceSnapshotFieldState,
};
