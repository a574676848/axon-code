//! Session Runtime 应用服务。

mod agent_turn_canonical_terminal;
mod agent_turn_executor;
mod goal_execution_report;
mod provider_request_builder;
mod runtime_profile;
mod session_attach_snapshot;
mod session_checkpoint;
mod session_recovery;
mod tool_environment;
mod turn_profile;

pub use agent_turn_executor::{
    AgentTurnExecutor, ContinuationResult, ProviderTurnResult, ProviderTurnRunner,
    wire_event_to_runtime_event,
};
pub use provider_request_builder::{ProviderTurnRequest, build_provider_turn_request};
pub use runtime_profile::{RuntimeProfile, RuntimeProfileSelector};
pub use session_attach_snapshot::SnapshotChunker;
pub use session_checkpoint::SessionCheckpoint;
pub use session_recovery::SessionRecovery;
pub use tool_environment::ProviderToolEnvironment;
pub use turn_profile::{
    ORIGIN_CHANNEL_GOAL, ORIGIN_CHANNEL_KEY, PRIMARY_TURN_ACTION_KIND, TerminalAccounting,
    TerminalFailureOwner, TurnContextProfile, TurnOrigin, TurnProfile,
};
