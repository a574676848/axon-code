//! Axon 领域层类型。
//!
//! 本 crate 只定义纯领域对象、状态和错误，不依赖 UI、数据库、网络框架或模型 SDK。

#[path = "model/activity.rs"]
mod activity;
#[path = "model/attachment.rs"]
mod attachment;
#[path = "model/audience.rs"]
mod audience;
#[path = "model/cell.rs"]
mod cell;
#[path = "model/channel_breaker.rs"]
mod channel_breaker;
#[path = "model/channel_failure.rs"]
mod channel_failure;
#[path = "model/cron.rs"]
mod cron;
#[path = "model/daemon.rs"]
mod daemon;
#[path = "model/daemon_health.rs"]
mod daemon_health;
#[path = "model/decision.rs"]
mod decision;
#[path = "model/error.rs"]
mod error;
#[path = "model/event.rs"]
mod event;
#[path = "model/execution.rs"]
mod execution;
#[path = "model/goal.rs"]
mod goal;
#[path = "model/ids.rs"]
mod ids;
#[path = "model/interaction.rs"]
mod interaction;
#[path = "model/ledger_id.rs"]
mod ledger_id;
#[path = "model/mcp.rs"]
mod mcp;
#[path = "model/memory.rs"]
mod memory;
#[cfg(test)]
#[path = "model/memory_tests.rs"]
mod memory_tests;
#[path = "model/model.rs"]
mod model;
#[path = "model/model_channel.rs"]
mod model_channel;
#[path = "model/observability.rs"]
mod observability;
#[path = "model/observability_realtime.rs"]
mod observability_realtime;
#[path = "model/observation.rs"]
mod observation;
#[path = "model/pane.rs"]
mod pane;
#[path = "model/platform.rs"]
mod platform;
#[path = "model/prediction.rs"]
mod prediction;
#[path = "model/pricing.rs"]
mod pricing;
#[path = "model/principal.rs"]
mod principal;
#[path = "model/quoted_message.rs"]
mod quoted_message;
#[path = "model/ratio.rs"]
mod ratio;
#[path = "model/remote.rs"]
mod remote;
#[path = "model/result.rs"]
mod result;
#[path = "model/failure.rs"]
mod runtime_failure;
#[path = "model/scope.rs"]
mod scope;
#[path = "model/semantic_output.rs"]
mod semantic_output;
#[path = "model/thread_spawn.rs"]
mod thread_spawn;
#[path = "model/token_budget.rs"]
mod token_budget;
#[path = "model/tool.rs"]
mod tool;
#[path = "model/user_question.rs"]
mod user_question;

pub use activity::{ActivityQuery, ActivitySnapshot};
pub use attachment::{
    AttachmentKind, AttachmentPromptPolicy, AttachmentRef, AttachmentReviewState,
    AttachmentSensitivity, AttachmentSource,
};
pub use audience::{AudienceContext, AudienceError, AudienceKind, DisclosurePolicy, ReplyRoute};
pub use cell::{
    ApplyExecutionStatus, BlockReason, CancelReason, CellClosureState, CellFailure, CellResult,
    McpServerStartupStatus, OutputDrainState, PausedCellState, RuntimeCell, RuntimeCellEvent,
    RuntimeCellSnapshot, RuntimeCellState,
};
pub use channel_breaker::BreakerPolicy;
pub use channel_failure::{ChannelFailureKind, classify_channel_failure};
pub use cron::{
    CRON_SCHEMA_VERSION, CronDeliveryAdapter, CronDeliveryTarget, CronJob, CronJobInput,
    CronJobStatus, CronJobUpdate, CronRunRecord, CronValidationError,
};
pub use daemon::{
    ConsumerCursor, DaemonDomainError, DaemonSessionId, RunSessionId, RuntimeHostId, RuntimeLease,
    SchedulePolicy, SessionActionId, SessionActionStatus, SessionRuntimeStatus,
};
pub use daemon_health::{
    DaemonHealthSnapshot, DeliveryHealth, HealthBacklog, HealthHostState, HealthStatusCounts,
};
pub use decision::{DecisionEffect, DecisionRecord};
pub use error::{
    AxonError, AxonResult, CellError, ConfigError, MemoryError, ProviderError,
    ProviderInvalidResponse, ProviderToolCallIssue, RuntimeError, ToolError,
};
pub use event::JarvisEvent;
pub use execution::{ExecutionCheckpoint, ExecutionLease, ExecutionRecord, ExecutionStatus};
pub use goal::{
    GoalActivityView, GoalCancelCommand, GoalCapabilityError, GoalCreateCommand, GoalCreationScope,
    GoalDetailQuery, GoalError, GoalListQuery, GoalPlan, GoalProgress, GoalQueryScope, GoalRecord,
    GoalStatus, GoalUpdateCommand, MAX_GOAL_TEXT_LENGTH,
};
pub use ids::{
    AttachmentId, ByteChunkId, CellId, ExecutionRequestId, LongRunningJobId, MemoryCandidateId,
    MemoryId, ModelProfileId, OutputRefId, PaneId, PermissionSnapshotId, SessionId, SuggestionId,
    SuggestionUndoTokenId, ToolCallId, ToolId, TurnId,
};
pub use interaction::{
    COMPLETION_STEER_SOURCE_PREFIX, CRON_DELIVERY_NOTIFICATION_PREFIX, CRON_DELIVERY_SOURCE_PREFIX,
    InteractionError, InteractionId, InteractionRecord, InteractionSource, InteractionStatus,
    MAX_INTERACTION_CONTENT_LENGTH, MAX_INTERACTION_ID_LENGTH, SYSTEM_SOURCE_CHANNEL,
    cron_delivery_notification_id,
};
pub use ledger_id::{DecisionId, ExecutionId, GoalId, JarvisEventId, ResultId};
pub use mcp::{MCP_CONFIG_FILE, McpServerDefinition};
pub use memory::{
    MEMORY_DEFAULT_TOMBSTONE_REASON, MEMORY_STRONG_MIN_CONFIDENCE_BASIS_POINTS,
    MEMORY_WEAK_MIN_CONFIDENCE_BASIS_POINTS, MemoryAdmissionDecision, MemoryAdmissionGate,
    MemoryAdmissionPolicy, MemoryAdmissionReason, MemoryCandidate, MemoryCandidateKind, MemoryKind,
    MemoryRecallDecision, MemoryRecallGate, MemoryRecallPath, MemoryRecord, MemoryRetentionPolicy,
    MemorySensitivity, MemorySourceOrigin, MemorySourceTrace, MemoryState, MemoryTombstone,
    MemoryVerification, MemoryWriteDecision,
};
pub use model::{
    CostClass, DEFAULT_VISION_MAX_BYTES_PER_IMAGE, DEFAULT_VISION_MAX_TOTAL_BYTES, ModelProfile,
    RouteReason, RoutingObjective, TaskClassification, TaskKind, VisionInputLimits,
};
pub use model_channel::{
    DEFAULT_CHANNEL_PRIORITY, DEFAULT_CHANNEL_WEIGHT, ModelChannelCandidate, ModelChannelPlan,
    ModelChannelSelection,
};
pub use observability::{
    HealthStatus, HealthTimelineBucket, HeatmapBucket, MetricDelta, RealtimeSummaryMetrics,
    RealtimeWindowReport, RequestStatusPoint, SLIDING_WINDOW_SLOT_COUNT,
    SLIDING_WINDOW_SLOT_DURATION_MS, SLIDING_WINDOW_TOTAL_DURATION_MS, SlidingWindowRingBuffer,
    SlotSnapshot, ThroughputPoint, TokenComposition, WindowSlot, build_activity_heatmap_buckets,
    calculate_heatmap_level, classify_health,
};
pub use observation::{
    GatewayMetrics, GenerationMetrics, LspMetricsSnapshot, METRIC_GATEWAY_ACTIVE_TURN_COUNT,
    METRIC_LLM_PROVIDER_GENERATION_SPEED, METRIC_LLM_PROVIDER_PROMPT_CACHE_HIT_RATE,
    METRIC_LLM_PROVIDER_REASONING_EFFORT, METRIC_LLM_PROVIDER_REQUEST_PATH,
    METRIC_LLM_PROVIDER_SPEED_MODE, METRIC_LLM_PROVIDER_TTFT_MS,
    METRIC_LLM_PROVIDER_UPSTREAM_MODEL, METRIC_TUI_TRANSCRIPT_RENDER_CACHE_HIT_RATE,
    RuntimeObservationFrame, TurnTimingMetrics, compute_gateway_active_turn_count,
};
pub use pane::{PaneSecurityBadge, PaneSecurityBadgeLevel};
pub use platform::PlatformKind;
pub use prediction::{
    CommandCompletionExplainabilityCopy, GHOST_TEXT_MAX_CHARS, GhostTextProjection, PredictionGate,
    PredictionSignalSnapshot, REMOTE_SUGGESTION_SUMMARY_MAX_CHARS, RemoteSuggestionSummary,
    SUGGESTION_DEFAULT_MIN_CONFIDENCE_BASIS_POINTS, SUGGESTION_OPT_OUT_COPY,
    SuggestionAcceptDecision, SuggestionAcceptOrigin, SuggestionAcceptRequest,
    SuggestionAcceptanceEffect, SuggestionBlockReason, SuggestionCandidate,
    SuggestionDisplayDecision, SuggestionDisplayMode, SuggestionFeedbackKind,
    SuggestionFeedbackMemoryAdmission, SuggestionFeedbackRecord, SuggestionForbiddenImplication,
    SuggestionKind, SuggestionPolicy, SuggestionSafetyClass, SuggestionSideEffect,
    SuggestionUndoRecord,
};
pub use pricing::{
    ModelPriceRule, PricingStyle, calculate_cost_micros, calculate_item_cost_micros,
    compare_rules_priority, glob_match, match_rule, pattern_literal_len,
    quantize_price_per_m_from_usd,
};
pub use principal::{AXON_OWNER_ID_PREFIX, AxonOwnerId, RunPrincipalContext};
pub use quoted_message::QuotedMessageContext;
pub use ratio::{FULL_RATIO_BASIS_POINTS, ratio_basis_points};
pub use remote::{
    RemoteAction, RemoteCapabilityPolicy, RemoteDenialReason, RemoteSessionProjection,
    RemoteVisibility,
};
pub use result::{GOAL_RESULT_SUMMARY_MAX_CHARS, ResultErrorKind, ResultRecord};
pub use runtime_failure::{
    RUNTIME_FAILURE_CATEGORY_INTERRUPTED, RUNTIME_FAILURE_CATEGORY_PROVIDER_PROTOCOL,
    RUNTIME_FAILURE_CATEGORY_RUNTIME, RUNTIME_FAILURE_CODE_INTERRUPTED,
    RUNTIME_FAILURE_CODE_PROVIDER_INVALID_RESPONSE,
    RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE, RUNTIME_FAILURE_CODE_RUNTIME_FAILED,
    RUNTIME_FAILURE_KIND_INCOMPLETE, RUNTIME_FAILURE_KIND_INVALID_RESPONSE,
    RUNTIME_FAILURE_TIMESTAMP_UNKNOWN, RuntimeFailure,
};
pub use scope::{MAX_WORKSPACE_ID_LENGTH, ScopeError, ScopeRef, WorkspaceId};
pub use semantic_output::{MAX_SEMANTIC_OUTPUT_LENGTH, SemanticOutput, SemanticOutputError};
pub use thread_spawn::{ThreadSpawnEdge, ThreadSpawnEdgeStatus, ThreadSpawnNode};
pub use token_budget::TokenBudget;
pub use tool::{
    OutputArchiveReference, OutputRef, ToolCallInput, ToolConcurrencyPolicy, ToolDefinition,
    ToolExecutionPermission, ToolFreeformFormat, ToolInput, ToolInputFormat, ToolOutput,
    ToolOutputPolicy, ToolPermissionPolicy, ToolResult, ToolResultMetadata, ToolSchema,
    ToolSchemaField, ToolSchemaFieldKind,
};
pub use user_question::{
    DEFAULT_SUSPENDED_HANDOFF, UserAnswerItem, UserQuestionItem, UserQuestionOption,
    UserQuestionRecord, UserQuestionState, UserQuestionValidationError,
};
